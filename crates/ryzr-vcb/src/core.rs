//! Shared state and the world-space coordinate system.
//!
//! This module owns the editor's resources — the painted [`Board`], the
//! [`Editor`] tool state, the optional running [`Sim`], the [`Transport`] clock
//! and the edit/run [`Mode`] — plus the two pure functions that map between
//! board cells and Bevy world coordinates. It also spawns the camera and seeds
//! the canvas with a small starter board so the app opens on something real.

use bevy::prelude::*;
use ryzr_board::{Board, Cell, Direction, GateKind, Sim, Tile, demos};

/// Side length of one tile in world units. Cells sit on a `TILE`-spaced grid
/// with cell `(x, y)` centred at `(x * TILE, y * TILE)` — `+y` is up, matching
/// [`Direction::North`].
pub const TILE: f32 = 34.0;

/// The painted board. Wraps [`Board`] so it can live in the ECS as a resource
/// while still being used through its normal API via `Deref`.
#[derive(Resource, Deref, DerefMut)]
pub struct BoardRes(pub Board);

/// The currently running simulation, if any. `None` in edit mode; `Some` while
/// in [`Mode::Run`] (cleared again on exit).
///
/// Held as a Bevy **non-send** resource: a [`Sim`] owns a `Box<dyn Simulator>`
/// that is `Send` but not `Sync`, so it can't be a normal parallel-access
/// resource. The editor only ever drives one small circuit, so pinning the sim
/// (and the few systems that touch it) to the main thread costs nothing.
pub struct SimRes(pub Option<Sim>);

/// Whether the editor is painting tiles or running the circuit.
#[derive(States, Default, Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Mode {
    /// Paint, erase, rotate, change layers — the board is editable.
    #[default]
    Edit,
    /// The board is compiled to a [`Sim`] and ticking; switches are clickable.
    Run,
}

/// The clock that drives the simulation in [`Mode::Run`].
#[derive(Resource)]
pub struct Transport {
    /// Free-running when `true`; frozen (except single steps) when `false`.
    pub playing: bool,
    /// Target tick rate in hertz.
    pub hz: f32,
    /// Fractional-tick carry so non-integer `hz` advances smoothly.
    pub accumulator: f32,
    /// Set by the `.` key to request exactly one tick on the next frame.
    pub step: bool,
}

impl Default for Transport {
    fn default() -> Self {
        Self { playing: true, hz: 4.0, accumulator: 0.0, step: false }
    }
}

/// A tile-placing tool. [`Tool::Gate`] additionally carries the gate kind and
/// facing held in [`Editor`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tool {
    Wire,
    Cross,
    Via,
    Gate,
    Switch,
    Source,
    Clock,
    Led,
    Erase,
}

impl Tool {
    /// Palette order, used by [`Tool::next`] for `Tab` cycling.
    const ROW: [Tool; 9] = [
        Tool::Wire,
        Tool::Cross,
        Tool::Via,
        Tool::Gate,
        Tool::Switch,
        Tool::Source,
        Tool::Clock,
        Tool::Led,
        Tool::Erase,
    ];

    /// Short HUD label for this tool.
    pub fn label(self) -> &'static str {
        match self {
            Tool::Wire => "Wire",
            Tool::Cross => "Cross",
            Tool::Via => "Via",
            Tool::Gate => "Gate",
            Tool::Switch => "Switch",
            Tool::Source => "Source",
            Tool::Clock => "Clock",
            Tool::Led => "Led",
            Tool::Erase => "Erase",
        }
    }

    /// The next tool in palette order, wrapping around.
    pub fn next(self) -> Tool {
        let i = Tool::ROW.iter().position(|&t| t == self).unwrap_or(0);
        Tool::ROW[(i + 1) % Tool::ROW.len()]
    }

    /// The [`Tile`] this tool paints, given the active gate kind and facing.
    /// [`Tool::Erase`] paints [`Tile::Empty`].
    pub fn tile(self, kind: GateKind, facing: Direction) -> Tile {
        match self {
            Tool::Wire => Tile::Wire,
            Tool::Cross => Tile::Cross,
            Tool::Via => Tile::Via,
            Tool::Gate => Tile::Gate { kind, facing },
            Tool::Switch => Tile::Switch,
            Tool::Source => Tile::Source { on: true },
            Tool::Clock => Tile::Clock,
            Tool::Led => Tile::Led,
            Tool::Erase => Tile::Empty,
        }
    }
}

/// All the editor's tool state: the selected tool, the gate kind and facing it
/// will stamp, the active layer, and the live cursor/paint bookkeeping plus a
/// transient status line for the HUD.
#[derive(Resource)]
pub struct Editor {
    pub tool: Tool,
    pub gate_kind: GateKind,
    pub facing: Direction,
    pub active_layer: u32,
    /// The cell under the cursor on the active layer, if the cursor is on the
    /// board.
    pub hovered: Option<Cell>,
    /// The cursor position in world space, if the cursor is in the window.
    pub cursor_world: Option<Vec2>,
    /// The last cell painted this drag, so fast drags fill a continuous line.
    pub last_paint: Option<Cell>,
    /// A short message shown at the end of the HUD line (errors, mode notes).
    pub status: String,
}

impl Default for Editor {
    fn default() -> Self {
        Self {
            tool: Tool::Wire,
            gate_kind: GateKind::And,
            facing: Direction::East,
            active_layer: 0,
            hovered: None,
            cursor_world: None,
            last_paint: None,
            status: String::new(),
        }
    }
}

/// World-space centre of a cell. Only the planar `(x, y)` matters for drawing;
/// layers are composited by fading, not by translation.
pub fn cell_center_world(cell: Cell) -> Vec2 {
    Vec2::new(cell.x as f32 * TILE, cell.y as f32 * TILE)
}

/// The cell on `layer` whose square contains `world`, or `None` if that falls
/// outside the board.
pub fn world_to_cell(world: Vec2, layer: u32, board: &Board) -> Option<Cell> {
    let xf = (world.x / TILE).round();
    let yf = (world.y / TILE).round();
    if xf < 0.0 || yf < 0.0 {
        return None;
    }
    let cell = Cell::new(layer, xf as u32, yf as u32);
    board.contains(cell).then_some(cell)
}

/// A four-layer canvas seeded with three demos from [`ryzr_board::demos`],
/// stamped at non-overlapping offsets that frame the board's centre: the SR
/// latch (9×9, interactive via its set/reset switches) on the right, with the
/// cross-layer via bridge (the killer feature, immediately runnable) and the
/// clock blinker (visible motion on run) stacked down the left.
pub fn starter_board() -> Board {
    let mut board = Board::new(4, 30, 18);
    stamp(&mut board, &demos::sr_latch(), 0, 15, 5);
    stamp(&mut board, &demos::via_bridge(), 0, 5, 6);
    stamp(&mut board, &demos::clock_blinker(), 0, 5, 11);
    board
}

/// Copy every non-empty tile of `src` into `dst`, translated by
/// `(dl, dx, dy)`. Adjacency is preserved because the translation is rigid.
fn stamp(dst: &mut Board, src: &Board, dl: u32, dx: u32, dy: u32) {
    for (cell, tile) in src.iter() {
        dst.set(Cell::new(cell.layer + dl, cell.x + dx, cell.y + dy), tile);
    }
}

/// Spawn a 2D camera centred on the board and zoomed out enough to frame it.
fn setup_camera(mut commands: Commands, board: Res<BoardRes>) {
    let center = Vec2::new(
        (board.width() as f32 - 1.0) * 0.5 * TILE,
        (board.height() as f32 - 1.0) * 0.5 * TILE,
    );
    let mut projection = OrthographicProjection::default_2d();
    projection.scale = 1.4;
    commands.spawn((
        Camera2d,
        Projection::Orthographic(projection),
        Transform::from_translation(center.extend(0.0)),
    ));
}

pub fn plugin(app: &mut App) {
    app.insert_resource(ClearColor(Color::srgb(0.05, 0.06, 0.09)))
        .insert_resource(BoardRes(starter_board()))
        .init_resource::<Editor>()
        .insert_non_send(SimRes(None))
        .init_resource::<Transport>()
        .init_state::<Mode>()
        .add_systems(Startup, setup_camera);
}
