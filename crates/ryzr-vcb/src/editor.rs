//! Input → board edits: cursor tracking, painting/erasing, tool and layer
//! selection, and camera pan/zoom. Everything here runs only in [`Mode::Edit`]
//! except the cursor tracking, mode toggle and camera controls, which are live
//! in both modes.

use bevy::input::mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll};
use bevy::window::PrimaryWindow;

use crate::prelude::*;

/// Project the OS cursor into world space and resolve the hovered cell on the
/// active layer. Runs in `PreUpdate` so every later system sees a fresh hover.
fn update_cursor(
    mut editor: ResMut<Editor>,
    board: Res<BoardRes>,
    windows: Query<&Window, With<PrimaryWindow>>,
    cameras: Query<(&Camera, &GlobalTransform)>,
) {
    let layer = editor.active_layer;
    editor.cursor_world = None;
    editor.hovered = None;

    let Some(window) = windows.iter().next() else { return };
    let Some((camera, cam_tf)) = cameras.iter().next() else { return };
    let Some(cursor) = window.cursor_position() else { return };
    let Ok(world) = camera.viewport_to_world_2d(cam_tf, cursor) else { return };

    editor.cursor_world = Some(world);
    editor.hovered = world_to_cell(world, layer, &board);
}

/// Left-drag paints the active tool; right-drag erases. A drag fills the line
/// between the previous and current cell so fast motion leaves no gaps.
fn paint(
    mouse: Res<ButtonInput<MouseButton>>,
    mut editor: ResMut<Editor>,
    mut board: ResMut<BoardRes>,
) {
    let Some(hovered) = editor.hovered else {
        editor.last_paint = None;
        return;
    };
    if mouse.pressed(MouseButton::Left) {
        let tile = editor.tool.tile(editor.gate_kind, editor.facing);
        paint_line(&mut board, editor.last_paint, hovered, tile);
        editor.last_paint = Some(hovered);
    } else if mouse.pressed(MouseButton::Right) {
        paint_line(&mut board, editor.last_paint, hovered, Tile::Empty);
        editor.last_paint = Some(hovered);
    } else {
        editor.last_paint = None;
    }
}

/// Set every cell on the segment from `from` (if any, same layer) to `to`.
fn paint_line(board: &mut Board, from: Option<Cell>, to: Cell, tile: Tile) {
    if let Some(from) = from
        && from.layer == to.layer
    {
        for (x, y) in bresenham(from.x as i32, from.y as i32, to.x as i32, to.y as i32) {
            if x >= 0 && y >= 0 {
                board.set(Cell::new(to.layer, x as u32, y as u32), tile);
            }
        }
        return;
    }
    board.set(to, tile);
}

/// Integer line rasterization (Bresenham) between two grid points, inclusive.
fn bresenham(x0: i32, y0: i32, x1: i32, y1: i32) -> Vec<(i32, i32)> {
    let dx = (x1 - x0).abs();
    let dy = -(y1 - y0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut err = dx + dy;
    let (mut x, mut y) = (x0, y0);
    let mut pts = Vec::new();
    loop {
        pts.push((x, y));
        if x == x1 && y == y1 {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x += sx;
        }
        if e2 <= dx {
            err += dx;
            y += sy;
        }
    }
    pts
}

/// `Space` flips between edit and run.
fn mode_toggle(
    keys: Res<ButtonInput<KeyCode>>,
    state: Res<State<Mode>>,
    mut next: ResMut<NextState<Mode>>,
) {
    if keys.just_pressed(KeyCode::Space) {
        next.set(match state.get() {
            Mode::Edit => Mode::Run,
            Mode::Run => Mode::Edit,
        });
    }
}

/// Tool and gate-kind selection: `1`–`8` pick a gate kind (and the Gate tool),
/// letter keys pick tools, `Tab` cycles, `R` rotates the stamp facing.
fn tool_keys(keys: Res<ButtonInput<KeyCode>>, mut editor: ResMut<Editor>) {
    const KINDS: [(KeyCode, GateKind); 8] = [
        (KeyCode::Digit1, GateKind::And),
        (KeyCode::Digit2, GateKind::Or),
        (KeyCode::Digit3, GateKind::Xor),
        (KeyCode::Digit4, GateKind::Nand),
        (KeyCode::Digit5, GateKind::Nor),
        (KeyCode::Digit6, GateKind::Xnor),
        (KeyCode::Digit7, GateKind::Buf),
        (KeyCode::Digit8, GateKind::Not),
    ];
    const TOOLS: [(KeyCode, Tool); 8] = [
        (KeyCode::KeyW, Tool::Wire),
        (KeyCode::KeyX, Tool::Cross),
        (KeyCode::KeyV, Tool::Via),
        (KeyCode::KeyG, Tool::Gate),
        (KeyCode::KeyS, Tool::Switch),
        (KeyCode::KeyO, Tool::Source),
        (KeyCode::KeyC, Tool::Clock),
        (KeyCode::KeyL, Tool::Led),
    ];

    for (key, kind) in KINDS {
        if keys.just_pressed(key) {
            editor.gate_kind = kind;
            editor.tool = Tool::Gate;
        }
    }
    for (key, tool) in TOOLS {
        if keys.just_pressed(key) {
            editor.tool = tool;
        }
    }
    if keys.just_pressed(KeyCode::KeyE) {
        editor.tool = Tool::Erase;
    }
    if keys.just_pressed(KeyCode::Tab) {
        editor.tool = editor.tool.next();
    }
    if keys.just_pressed(KeyCode::KeyR) {
        editor.facing = editor.facing.rotate_cw();
    }
}

/// `]` moves up a layer, `[` moves down, clamped to the board's stack.
fn layer_keys(keys: Res<ButtonInput<KeyCode>>, board: Res<BoardRes>, mut editor: ResMut<Editor>) {
    if keys.just_pressed(KeyCode::BracketRight) && editor.active_layer + 1 < board.layers() {
        editor.active_layer += 1;
    }
    if keys.just_pressed(KeyCode::BracketLeft) && editor.active_layer > 0 {
        editor.active_layer -= 1;
    }
}

/// Middle-drag pans the camera, scaling screen motion by the zoom level.
fn camera_pan(
    mouse: Res<ButtonInput<MouseButton>>,
    motion: Res<AccumulatedMouseMotion>,
    mut cameras: Query<(&mut Transform, &Projection), With<Camera2d>>,
) {
    if !mouse.pressed(MouseButton::Middle) {
        return;
    }
    let Some((mut tf, projection)) = cameras.iter_mut().next() else { return };
    let scale = match projection {
        Projection::Orthographic(o) => o.scale,
        _ => 1.0,
    };
    let d = motion.delta;
    tf.translation.x = d.x.mul_add(-scale, tf.translation.x);
    tf.translation.y = d.y.mul_add(scale, tf.translation.y);
}

/// Scroll zooms the orthographic camera, clamped to a sane range.
fn camera_zoom(
    scroll: Res<AccumulatedMouseScroll>,
    mut cameras: Query<&mut Projection, With<Camera2d>>,
) {
    let dy = scroll.delta.y;
    if dy.abs() < f32::EPSILON {
        return;
    }
    let Some(mut projection) = cameras.iter_mut().next() else { return };
    if let Projection::Orthographic(o) = &mut *projection {
        let factor = dy.mul_add(-0.1, 1.0).clamp(0.5, 2.0);
        o.scale = (o.scale * factor).clamp(0.4, 8.0);
    }
}

pub fn plugin(app: &mut App) {
    app.add_systems(PreUpdate, update_cursor)
        .add_systems(Update, (mode_toggle, camera_pan, camera_zoom))
        .add_systems(Update, (paint, tool_keys, layer_keys).run_if(in_state(Mode::Edit)));
}
