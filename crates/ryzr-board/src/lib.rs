//! A multi-layer grid editor model that compiles to a [`ryzr_core`] circuit.
//!
//! This crate is the pure-logic half of the editor: no rendering, no Bevy, no
//! windowing — just the data model and the lowering, so it builds in seconds and
//! is exhaustively unit-testable headless. The Bevy front-end (`ryzr-vcb`) is a
//! thin layer over the types re-exported here.
//!
//! # Pipeline
//!
//! ```text
//!   Board ──extract──▶ Nets ──compile──▶ Circuit ──Sim──▶ readable state
//!   (tiles)            (union-find)      (ryzr-core)        (cell_on / led_on)
//! ```
//!
//! 1. A [`Board`] is a stack of grids of [`Tile`]s — the document you paint.
//! 2. [`compile`] runs net extraction and lowers the board to a runnable
//!    [`ryzr_core::Circuit`], turning every driver into a register so feedback
//!    loops (latches, oscillators) are legal under ryzr's no-combinational-cycle
//!    rule while still advancing one tile per tick — the Virtual Circuit Board
//!    semantics, expressed natively.
//! 3. [`Sim`] runs that circuit and answers per-cell queries the renderer needs.
//!
//! # Example
//!
//! ```
//! use ryzr_board::{demos, Sim};
//!
//! // Two switches into an AND gate into an LED.
//! let board = demos::and_gate();
//! let (a, b) = demos::two_input_switches();
//! let led = demos::two_input_led();
//!
//! let mut sim = Sim::new(&board).expect("board compiles");
//! sim.set_switch(a, true);
//! sim.set_switch(b, true);
//! sim.run(4); // let the registered gate settle
//! assert!(sim.led_on(led));
//! ```

mod board;
mod compile;
pub mod demos;
mod net;
mod sim;
mod tile;

pub use board::{Board, Cell};
pub use compile::{BoardMap, CompileError, Compiled, compile};
pub use net::{NetId, Nets};
#[cfg(feature = "engines")]
pub use sim::EngineKind;
pub use sim::Sim;
pub use tile::{Direction, GateKind, Tile};
