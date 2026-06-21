//! Optional headless capture. When the `RYZR_VCB_CAPTURE` environment variable
//! names a path, the app lets the scene settle for a few frames, saves a PNG of
//! the primary window there, and exits. Setting `RYZR_VCB_CAPTURE_RUN` as well
//! first enters run mode and ticks the demo a few times, so the shot shows live
//! signals instead of the idle editor. A normal run sets neither variable and
//! never enters this path — it exists for CI smoke tests and documentation shots.

use bevy::render::view::screenshot::{Capturing, Screenshot, save_to_disk};

use crate::prelude::*;

/// The one-shot capture sequence. `Copy` so the driver can read the current
/// phase out of the resource and reassign it within the same frame.
#[derive(Clone, Copy)]
enum Phase {
    /// Let the first frame's layout and the starter board settle. If a run-mode
    /// shot was requested, enter [`Mode::Run`] afterwards; otherwise shoot.
    Settle,
    /// Force one tick per frame so the demo's oscillator and latch light up,
    /// then request the screenshot.
    Tick,
    /// Screenshot requested. `started` flips once the in-flight `Capturing`
    /// marker has been seen, so we only exit after the readback has finished and
    /// the file is on disk.
    Flush { started: bool },
}

/// Drives the capture sequence across frames.
#[derive(Resource)]
struct Capture {
    /// Where to write the PNG.
    path: String,
    /// Whether to enter run mode and tick before shooting.
    run: bool,
    /// Frames left in the current phase.
    frames: u32,
    /// Current phase.
    phase: Phase,
}

/// Spawn a primary-window screenshot that saves itself to `path` when captured.
/// Takes the path by value because the save observer must own it (`'static`).
fn shoot(commands: &mut Commands, path: String) {
    commands.spawn(Screenshot::primary_window()).observe(save_to_disk(path));
}

/// Step the capture state machine: settle, optionally run-and-tick, shoot, and
/// exit once the capture has flushed to disk.
fn drive_capture(
    mut commands: Commands,
    mut capture: ResMut<Capture>,
    mut next: ResMut<NextState<Mode>>,
    mut transport: ResMut<Transport>,
    capturing: Query<(), With<Capturing>>,
) {
    match capture.phase {
        Phase::Settle => {
            if capture.frames > 0 {
                capture.frames -= 1;
            } else if capture.run {
                next.set(Mode::Run);
                capture.phase = Phase::Tick;
                capture.frames = 24;
            } else {
                let path = capture.path.clone();
                shoot(&mut commands, path);
                capture.phase = Phase::Flush { started: false };
            }
        }
        Phase::Tick => {
            if capture.frames > 0 {
                capture.frames -= 1;
                transport.step = true;
            } else {
                let path = capture.path.clone();
                shoot(&mut commands, path);
                capture.phase = Phase::Flush { started: false };
            }
        }
        Phase::Flush { started } => {
            if capturing.iter().next().is_some() {
                capture.phase = Phase::Flush { started: true };
            } else if started {
                // The capture started and has now finished: the file is on disk.
                commands.write_message(AppExit::Success);
            }
        }
    }
}

pub fn plugin(app: &mut App) {
    let Ok(path) = std::env::var("RYZR_VCB_CAPTURE") else {
        return;
    };
    let run = std::env::var("RYZR_VCB_CAPTURE_RUN").is_ok();
    app.insert_resource(Capture { path, run, frames: 16, phase: Phase::Settle })
        .add_systems(Update, drive_capture);
}
