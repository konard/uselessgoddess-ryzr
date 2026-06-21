//! The simulation lifecycle. Entering [`Mode::Run`] compiles the board to a
//! [`Sim`]; while running, the transport advances it and clicks toggle
//! switches; leaving run mode drops the sim. A compile error drops straight
//! back to edit mode with the error in the status line — no panic.

use crate::prelude::*;

/// On entering run mode, compile the board. On success, store the sim and note
/// the net count; on failure, surface the error and bounce back to edit mode.
fn enter_run(
    board: Res<BoardRes>,
    mut sim: NonSendMut<SimRes>,
    mut editor: ResMut<Editor>,
    mut transport: ResMut<Transport>,
    mut next: ResMut<NextState<Mode>>,
) {
    match Sim::new(&board) {
        Ok(s) => {
            transport.accumulator = 0.0;
            editor.status = format!("running · {} nets", s.net_count());
            sim.0 = Some(s);
        }
        Err(e) => {
            editor.status = format!("compile error: {e}");
            sim.0 = None;
            next.set(Mode::Edit);
        }
    }
}

/// On leaving run mode, drop the sim so edit mode starts clean.
fn exit_run(mut sim: NonSendMut<SimRes>) {
    sim.0 = None;
}

/// Transport keys: `P` play/pause, `.` single-step, `+`/`-` change speed.
fn transport_controls(
    keys: Res<ButtonInput<KeyCode>>,
    mut transport: ResMut<Transport>,
    mut editor: ResMut<Editor>,
) {
    if keys.just_pressed(KeyCode::KeyP) {
        transport.playing = !transport.playing;
        editor.status = if transport.playing { "running".to_owned() } else { "paused".to_owned() };
    }
    if keys.just_pressed(KeyCode::Period) {
        transport.step = true;
    }
    if keys.just_pressed(KeyCode::Equal) || keys.just_pressed(KeyCode::NumpadAdd) {
        transport.hz = (transport.hz * 2.0).min(256.0);
    }
    if keys.just_pressed(KeyCode::Minus) || keys.just_pressed(KeyCode::NumpadSubtract) {
        transport.hz = (transport.hz * 0.5).max(0.25);
    }
}

/// Advance the sim: a pending single step ticks once; otherwise, when playing,
/// accumulate `hz * dt` and tick the whole number of ticks owed (capped so a
/// long stall can't freeze the frame).
fn advance_sim(time: Res<Time>, mut sim: NonSendMut<SimRes>, mut transport: ResMut<Transport>) {
    let Some(s) = sim.0.as_mut() else { return };
    let ticks = if transport.step {
        transport.step = false;
        1
    } else if transport.playing {
        transport.accumulator = time.delta_secs().mul_add(transport.hz, transport.accumulator);
        let whole = transport.accumulator.floor();
        transport.accumulator -= whole;
        (whole as u64).min(1024)
    } else {
        0
    };
    if ticks > 0 {
        s.run(ticks);
    }
}

/// Left-click a switch to toggle it while running.
fn click_switch(
    mouse: Res<ButtonInput<MouseButton>>,
    editor: Res<Editor>,
    mut sim: NonSendMut<SimRes>,
) {
    if !mouse.just_pressed(MouseButton::Left) {
        return;
    }
    let Some(cell) = editor.hovered else { return };
    let Some(s) = sim.0.as_mut() else { return };
    if s.is_switch(cell) {
        s.toggle_switch(cell);
    }
}

pub fn plugin(app: &mut App) {
    app.add_systems(OnEnter(Mode::Run), enter_run)
        .add_systems(OnExit(Mode::Run), exit_run)
        .add_systems(
            Update,
            (transport_controls, advance_sim, click_switch).chain().run_if(in_state(Mode::Run)),
        );
}
