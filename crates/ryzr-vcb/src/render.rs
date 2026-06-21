//! Procedural rendering of the board with gizmos — no sprites, no atlases. Each
//! tile is drawn from primitives (lines, circles, triangles) so the look is
//! fully generated and scales crisply at any zoom. Layers are composited by
//! depth fading: the active layer at full contrast, layers below dimmed, layers
//! above faint, so the stack reads as depth without hiding anything.

use crate::prelude::*;

const C_GRID: Color = Color::srgba(1.0, 1.0, 1.0, 0.04);
const C_WIRE_OFF: Color = Color::srgb(0.22, 0.40, 0.52);
const C_WIRE_ON: Color = Color::srgb(0.30, 0.85, 1.0);
const C_VIA_OFF: Color = Color::srgb(0.62, 0.50, 0.28);
const C_VIA_ON: Color = Color::srgb(1.0, 0.78, 0.30);
const C_GATE_AND: Color = Color::srgb(0.55, 0.75, 0.45);
const C_GATE_OR: Color = Color::srgb(0.45, 0.65, 0.85);
const C_GATE_XOR: Color = Color::srgb(0.78, 0.55, 0.85);
const C_SWITCH: Color = Color::srgb(0.70, 0.55, 0.35);
const C_SOURCE_OFF: Color = Color::srgb(0.50, 0.38, 0.38);
const C_CLOCK: Color = Color::srgb(0.60, 0.55, 0.75);
const C_LED_OFF: Color = Color::srgb(0.45, 0.20, 0.22);
const C_LED_ON: Color = Color::srgb(1.0, 0.35, 0.40);
const C_ON: Color = Color::srgb(1.0, 0.85, 0.45);
const C_CURSOR: Color = Color::srgb(0.95, 0.95, 0.98);
const C_VIA_LINK: Color = Color::srgb(0.40, 0.95, 0.55);

/// Draw the grid and every painted tile, faded by its layer's distance from the
/// active one and lit from the live sim while running.
fn draw_board(
    mut gizmos: Gizmos,
    board: Res<BoardRes>,
    editor: Res<Editor>,
    sim: NonSend<SimRes>,
    state: Res<State<Mode>>,
) {
    let running = *state.get() == Mode::Run;
    let sim_ref = sim.0.as_ref();
    let active = editor.active_layer as i32;
    draw_grid(&mut gizmos, &board);
    for (cell, tile) in board.iter() {
        let f = depth_factor(cell.layer as i32 - active);
        draw_tile(&mut gizmos, &board, cell, tile, f, running, sim_ref);
    }
}

/// Highlight the hovered cell, and when the via tool is active, mark cells
/// directly above/below that already hold a via — the cue for where a vertical
/// link would fuse.
fn draw_cursor(mut gizmos: Gizmos, board: Res<BoardRes>, editor: Res<Editor>) {
    let Some(cell) = editor.hovered else { return };
    let c = cell_center_world(cell);
    let h = TILE * 0.5;
    gizmos.rect_2d(c, Vec2::splat(TILE), C_CURSOR);
    if editor.tool == Tool::Via {
        for n in board.vertical_neighbours(cell) {
            if board.get(n) == Tile::Via {
                gizmos.rect_2d(cell_center_world(n), Vec2::splat(h * 1.2), C_VIA_LINK);
            }
        }
    }
}

/// Opacity factor for a tile whose layer is `delta` above (`> 0`) or below
/// (`< 0`) the active layer. Below fades gently, above fades faintest.
fn depth_factor(delta: i32) -> f32 {
    if delta == 0 {
        1.0
    } else if delta < 0 {
        (delta as f32).mul_add(0.12, 0.62).clamp(0.22, 0.62)
    } else {
        (delta as f32).mul_add(-0.10, 0.42).clamp(0.14, 0.42)
    }
}

/// Scale a colour's alpha by `f` (used for depth fading).
fn fade(color: Color, f: f32) -> Color {
    let c = color.to_srgba();
    Color::srgba(c.red, c.green, c.blue, c.alpha * f)
}

/// Linear interpolation between two colours in sRGB space.
fn lerp_color(a: Color, b: Color, t: f32) -> Color {
    let a = a.to_srgba();
    let b = b.to_srgba();
    Color::srgba(
        (b.red - a.red).mul_add(t, a.red),
        (b.green - a.green).mul_add(t, a.green),
        (b.blue - a.blue).mul_add(t, a.blue),
        (b.alpha - a.alpha).mul_add(t, a.alpha),
    )
}

/// The board's faint background grid, spanning the whole canvas.
fn draw_grid(gizmos: &mut Gizmos, board: &Board) {
    let w = board.width();
    let h = board.height();
    let x0 = -0.5 * TILE;
    let y0 = -0.5 * TILE;
    let x1 = (w as f32 - 0.5) * TILE;
    let y1 = (h as f32 - 0.5) * TILE;
    for x in 0..=w {
        let xx = 0.5f32.mul_add(-TILE, x as f32 * TILE);
        gizmos.line_2d(Vec2::new(xx, y0), Vec2::new(xx, y1), C_GRID);
    }
    for y in 0..=h {
        let yy = 0.5f32.mul_add(-TILE, y as f32 * TILE);
        gizmos.line_2d(Vec2::new(x0, yy), Vec2::new(x1, yy), C_GRID);
    }
}

/// Is the same-layer neighbour in `dir` something a wire should visually
/// connect to (another conductor, a driver, or a led)?
fn connects_to(board: &Board, cell: Cell, dir: Direction) -> bool {
    match board.neighbour(cell, dir) {
        Some(n) => {
            let t = board.get(n);
            t.is_conductor() || t.is_driver() || t == Tile::Led
        }
        None => false,
    }
}

/// Whether a tile reads as "high" right now, for lighting.
fn cell_lit(sim: &Sim, cell: Cell, tile: Tile) -> bool {
    match tile {
        Tile::Led => sim.led_on(cell),
        Tile::Source { on } => on,
        _ => sim.cell_on(cell),
    }
}

/// Dispatch one tile to its primitive-drawing routine.
fn draw_tile(
    gizmos: &mut Gizmos,
    board: &Board,
    cell: Cell,
    tile: Tile,
    f: f32,
    running: bool,
    sim: Option<&Sim>,
) {
    let center = cell_center_world(cell);
    let h = TILE * 0.5;
    let lit = running && sim.is_some_and(|s| cell_lit(s, cell, tile));
    match tile {
        Tile::Wire => {
            let color = fade(if lit { C_WIRE_ON } else { C_WIRE_OFF }, f);
            draw_wire(gizmos, board, cell, color, h);
        }
        Tile::Cross => {
            let color = fade(if lit { C_WIRE_ON } else { C_WIRE_OFF }, f);
            draw_cross(gizmos, center, color, h);
        }
        Tile::Via => {
            let color = fade(if lit { C_VIA_ON } else { C_VIA_OFF }, f);
            let fused = board.vertical_neighbours(cell).any(|c| board.get(c) == Tile::Via);
            draw_via(gizmos, board, cell, color, h, fused);
        }
        Tile::Gate { kind, facing } => draw_gate(gizmos, center, kind, facing, f, lit, h),
        Tile::Switch => {
            let on = sim.is_some_and(|s| s.switch_on(cell));
            draw_switch(gizmos, center, fade(if on { C_ON } else { C_SWITCH }, f), on, h);
        }
        Tile::Source { on } => {
            let color = fade(if on { C_ON } else { C_SOURCE_OFF }, f);
            draw_source(gizmos, center, color, on, h);
        }
        Tile::Clock => draw_clock(gizmos, center, fade(if lit { C_ON } else { C_CLOCK }, f), h),
        Tile::Led => {
            draw_led(gizmos, center, fade(if lit { C_LED_ON } else { C_LED_OFF }, f), lit, h)
        }
        Tile::Empty => {}
    }
}

/// A wire: short stubs toward each connecting neighbour, plus a centre node.
fn draw_wire(gizmos: &mut Gizmos, board: &Board, cell: Cell, color: Color, h: f32) {
    let c = cell_center_world(cell);
    for dir in Direction::ALL {
        if connects_to(board, cell, dir) {
            let (dx, dy) = dir.delta();
            gizmos.line_2d(c, c + Vec2::new(dx as f32, dy as f32) * h, color);
        }
    }
    gizmos.circle_2d(c, h * 0.16, color);
}

/// A crossover: full horizontal and vertical segments that don't connect.
fn draw_cross(gizmos: &mut Gizmos, center: Vec2, color: Color, h: f32) {
    gizmos.line_2d(center - Vec2::X * h, center + Vec2::X * h, color);
    gizmos.line_2d(center - Vec2::Y * h, center + Vec2::Y * h, color);
}

/// A via: planar stubs plus concentric rings, with up/down chevrons when it
/// fuses with a via on an adjacent layer.
fn draw_via(gizmos: &mut Gizmos, board: &Board, cell: Cell, color: Color, h: f32, fused: bool) {
    let c = cell_center_world(cell);
    for dir in Direction::ALL {
        if connects_to(board, cell, dir) {
            let (dx, dy) = dir.delta();
            gizmos.line_2d(c, c + Vec2::new(dx as f32, dy as f32) * h, color);
        }
    }
    gizmos.circle_2d(c, h * 0.55, color);
    gizmos.circle_2d(c, h * 0.30, color);
    if fused {
        let t = h * 0.30;
        gizmos.linestrip_2d(
            [c + Vec2::new(-t, t * 0.2), c + Vec2::new(0.0, t * 0.9), c + Vec2::new(t, t * 0.2)],
            color,
        );
        gizmos.linestrip_2d(
            [c + Vec2::new(-t, -t * 0.2), c + Vec2::new(0.0, -t * 0.9), c + Vec2::new(t, -t * 0.2)],
            color,
        );
    }
}

/// A gate: a triangle pointing the way it drives, family ridges on the back
/// edge (none/AND, one/OR, two/XOR), and an inversion bubble at the tip for the
/// inverting kinds.
fn draw_gate(
    gizmos: &mut Gizmos,
    center: Vec2,
    kind: GateKind,
    facing: Direction,
    f: f32,
    lit: bool,
    h: f32,
) {
    let base = gate_color(kind);
    let color = fade(if lit { lerp_color(base, C_ON, 0.5) } else { base }, f);
    let rot = Vec2::from_angle(facing_angle(facing));
    let perp = Vec2::new(-rot.y, rot.x);
    let r = h * 0.78;

    let apex = center + rot * r;
    let back = -rot * (r * 0.55);
    let b1 = center + back + perp * (r * 0.72);
    let b2 = center + back - perp * (r * 0.72);
    gizmos.linestrip_2d([apex, b1, b2, apex], color);

    let ridges = match kind {
        GateKind::And | GateKind::Nand | GateKind::Buf | GateKind::Not => 0,
        GateKind::Or | GateKind::Nor => 1,
        GateKind::Xor | GateKind::Xnor => 2,
    };
    for i in 0..ridges {
        let off = back - rot * (r * 0.16 * (i as f32 + 1.0));
        gizmos.line_2d(center + off + perp * (r * 0.6), center + off - perp * (r * 0.6), color);
    }

    if matches!(kind, GateKind::Nand | GateKind::Nor | GateKind::Xnor | GateKind::Not) {
        gizmos.circle_2d(center + rot * (r * 1.12), h * 0.14, color);
    }
}

/// A switch: a square with a nub that sits high when on, low when off.
fn draw_switch(gizmos: &mut Gizmos, center: Vec2, color: Color, on: bool, h: f32) {
    gizmos.rect_2d(center, Vec2::splat(h * 1.3), color);
    let nub = if on { center + Vec2::Y * (h * 0.3) } else { center - Vec2::Y * (h * 0.3) };
    gizmos.circle_2d(nub, h * 0.26, color);
}

/// A constant source: a diamond, filled at the centre when driving high.
fn draw_source(gizmos: &mut Gizmos, center: Vec2, color: Color, on: bool, h: f32) {
    let r = h * 0.7;
    gizmos.linestrip_2d(
        [
            center + Vec2::Y * r,
            center + Vec2::X * r,
            center - Vec2::Y * r,
            center - Vec2::X * r,
            center + Vec2::Y * r,
        ],
        color,
    );
    if on {
        gizmos.circle_2d(center, h * 0.2, color);
    }
}

/// A clock: a ring with two hands.
fn draw_clock(gizmos: &mut Gizmos, center: Vec2, color: Color, h: f32) {
    gizmos.circle_2d(center, h * 0.6, color);
    gizmos.line_2d(center, center + Vec2::Y * (h * 0.55), color);
    gizmos.line_2d(center, center + Vec2::X * (h * 0.4), color);
}

/// A led: a ring that fills with concentric rings when lit.
fn draw_led(gizmos: &mut Gizmos, center: Vec2, color: Color, lit: bool, h: f32) {
    gizmos.circle_2d(center, h * 0.62, color);
    if lit {
        gizmos.circle_2d(center, h * 0.42, color);
        gizmos.circle_2d(center, h * 0.2, color);
    }
}

/// Family tint for a gate kind (the inverting kind shares its base family's).
fn gate_color(kind: GateKind) -> Color {
    match kind {
        GateKind::Or | GateKind::Nor => C_GATE_OR,
        GateKind::Xor | GateKind::Xnor => C_GATE_XOR,
        GateKind::And | GateKind::Nand | GateKind::Buf | GateKind::Not => C_GATE_AND,
    }
}

/// World-space rotation (radians) for a gate facing `dir`, with East at zero.
fn facing_angle(dir: Direction) -> f32 {
    match dir {
        Direction::East => 0.0,
        Direction::North => std::f32::consts::FRAC_PI_2,
        Direction::West => std::f32::consts::PI,
        Direction::South => -std::f32::consts::FRAC_PI_2,
    }
}

pub fn plugin(app: &mut App) {
    app.add_systems(Update, (draw_board, draw_cursor));
}
