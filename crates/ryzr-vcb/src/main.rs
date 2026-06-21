//! Thin entry point: build the app from the crate's root plugin and run it.

fn main() {
    bevy::prelude::App::new().add_plugins(ryzr_vcb::plugin).run();
}
