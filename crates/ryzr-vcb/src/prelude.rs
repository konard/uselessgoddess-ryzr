//! Internal prelude: the Bevy and `ryzr_board` items every module pulls in,
//! plus the editor's own shared types. Imported as `use crate::prelude::*;`.
//!
//! The `ryzr_board` and `crate::core` re-exports are listed by name (not
//! globbed) so they shadow any same-named item from `bevy::prelude` cleanly and
//! never drag a module's `plugin` function into scope.

pub use bevy::prelude::*;

pub use ryzr_board::{Board, Cell, Direction, GateKind, Sim, Tile};

pub use crate::core::{
    BoardRes, Editor, Mode, SimRes, TILE, Tool, Transport, cell_center_world, world_to_cell,
};
