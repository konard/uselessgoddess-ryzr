//! A multi-layer circuit editor on the [`ryzr_board`] model, rendered with Bevy.
//!
//! This crate is the thin view half of the editor: it owns the window, the
//! camera, input, and procedural rendering, and drives the pure-logic model in
//! [`ryzr_board`] (which compiles a painted board to a real `ryzr` circuit and
//! runs it). All the correctness-critical logic lives there and is tested
//! headless; everything here is presentation.
//!
//! Following the per-module-plugin convention, every module exposes a
//! `pub fn plugin(app: &mut App)` and [`plugin`] (this crate's root) wires them
//! together on top of a curated, x11-only [`DefaultPlugins`].

// Bevy systems take their parameters (`Res`, `Query`, `Gizmos`, …) by value
// because that is the `SystemParam` contract; the pedantic `needless_pass_by_value`
// lint fights the framework on every system here, so we silence it crate-wide.
#![allow(clippy::needless_pass_by_value)]

use bevy::prelude::*;

mod capture;
mod core;
mod editor;
mod prelude;
mod render;
mod sim;
mod ui;

/// Build the whole editor onto `app`: a curated [`DefaultPlugins`] with a named
/// window, then each subsystem's plugin (state and resources, input, sim,
/// rendering, HUD).
pub fn plugin(app: &mut App) {
    app.add_plugins(DefaultPlugins.set(WindowPlugin {
        primary_window: Some(Window {
            title: "ryzr-vcb — multi-layer circuit editor".into(),
            ..default()
        }),
        ..default()
    }))
    .add_plugins((
        crate::core::plugin,
        crate::editor::plugin,
        crate::sim::plugin,
        crate::render::plugin,
        crate::ui::plugin,
        crate::capture::plugin,
    ));
}
