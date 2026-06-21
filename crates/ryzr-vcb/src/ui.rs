//! The HUD: a top status line rebuilt every frame from live state, and a static
//! help line of controls at the bottom. Text only — the world itself is drawn
//! procedurally in [`crate::render`].

use crate::prelude::*;

/// Marks the live status line (top-left).
#[derive(Component)]
struct HudText;

/// Marks the static controls reference (bottom-left).
#[derive(Component)]
struct HelpText;

const HELP: &str = "L-drag paint | R-drag erase | M-drag pan | scroll zoom | Space edit/run\n\
     1-8 gate kind | W/X/V/G/S/O/C/L tools | E erase | Tab cycle | R rotate | [ ] layer\n\
     Run: P play/pause | . step | +/- speed | click a switch to toggle";

fn spawn_hud(mut commands: Commands) {
    commands.spawn((
        HudText,
        Text::new("ryzr-vcb"),
        TextFont::from_font_size(15.0),
        TextColor(Color::srgb(0.85, 0.92, 1.0)),
        Node {
            position_type: PositionType::Absolute,
            top: Val::Px(8.0),
            left: Val::Px(10.0),
            ..default()
        },
    ));
    commands.spawn((
        HelpText,
        Text::new(HELP),
        TextFont::from_font_size(12.0),
        TextColor(Color::srgb(0.55, 0.62, 0.72)),
        Node {
            position_type: PositionType::Absolute,
            bottom: Val::Px(8.0),
            left: Val::Px(10.0),
            ..default()
        },
    ));
}

/// Rebuild the status line: mode, active layer, tool (with gate kind/facing),
/// tile and net counts, and — while running — ticks, speed, play state, and the
/// hovered cell's live value.
fn update_hud(
    editor: Res<Editor>,
    board: Res<BoardRes>,
    sim: NonSend<SimRes>,
    state: Res<State<Mode>>,
    transport: Res<Transport>,
    mut query: Query<&mut Text, With<HudText>>,
) {
    let Some(mut text) = query.iter_mut().next() else { return };

    let mode = match state.get() {
        Mode::Edit => "EDIT",
        Mode::Run => "RUN",
    };
    let tool = if editor.tool == Tool::Gate {
        format!("Gate:{}", gate_label(editor.gate_kind))
    } else {
        editor.tool.label().to_owned()
    };

    let run_info = if *state.get() == Mode::Run {
        let ticks = sim.0.as_ref().map_or(0, Sim::ticks);
        let play = if transport.playing { "play" } else { "pause" };
        format!(" | {ticks} ticks | {:.0} Hz | {play}", transport.hz)
    } else {
        String::new()
    };
    let probe = match (state.get(), editor.hovered, sim.0.as_ref()) {
        (Mode::Run, Some(cell), Some(s)) => {
            let v = if s.cell_on(cell) { "high" } else { "low" };
            format!(" | probe ({},{}) {v}", cell.x, cell.y)
        }
        _ => String::new(),
    };

    let mut line = format!(
        "{mode} | layer L{}/{} | {tool} | facing {} | {} tiles | {} nets{run_info}{probe}",
        editor.active_layer + 1,
        board.layers(),
        facing_label(editor.facing),
        board.tile_count(),
        sim.0.as_ref().map_or(0, Sim::net_count),
    );
    if !editor.status.is_empty() {
        line.push_str(" | ");
        line.push_str(&editor.status);
    }
    text.0 = line;
}

/// Compact gate-kind label for the HUD.
fn gate_label(kind: GateKind) -> &'static str {
    match kind {
        GateKind::And => "AND",
        GateKind::Or => "OR",
        GateKind::Xor => "XOR",
        GateKind::Nand => "NAND",
        GateKind::Nor => "NOR",
        GateKind::Xnor => "XNOR",
        GateKind::Buf => "BUF",
        GateKind::Not => "NOT",
    }
}

/// Single-letter facing label for the HUD.
fn facing_label(dir: Direction) -> &'static str {
    match dir {
        Direction::North => "N",
        Direction::East => "E",
        Direction::South => "S",
        Direction::West => "W",
    }
}

pub fn plugin(app: &mut App) {
    app.add_systems(Startup, spawn_hud).add_systems(Update, update_hud);
}
