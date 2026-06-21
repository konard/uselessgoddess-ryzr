//! Run a compiled board and read it back in cell terms.
//!
//! [`Sim`] hides the choice of execution engine behind one façade. By default it
//! uses [`ryzr_core::Interpreter`], which needs no codegen and so keeps compile
//! times (and this crate's dependency footprint) tiny. With the `engines`
//! feature it can instead drive any [`ryzr_backend`] engine for raw throughput.

use std::collections::HashMap;

use ryzr_core::{Backend, Circuit, Interpreter};

use crate::board::{Board, Cell};
use crate::compile::{self, BoardMap, CompileError, Compiled};
use crate::net::NetId;

/// The minimum a backend must offer the façade: set inputs, advance, read
/// outputs. Both the interpreter wrapper and the optional engine wrapper
/// implement it, so [`Sim`] is engine-agnostic.
trait Simulator: Send {
    fn set_input(&mut self, index: usize, value: bool);
    fn output(&self, index: usize) -> bool;
    fn tick(&mut self);
}

/// The default backend: the reference interpreter, which owns the state and
/// output buffers the stateless [`Backend`] trait expects.
struct InterpreterSim {
    circuit: Circuit,
    inputs: Vec<bool>,
    state: Vec<bool>,
    outputs: Vec<bool>,
}

impl InterpreterSim {
    fn new(circuit: Circuit) -> Self {
        let inputs = vec![false; circuit.input_count as usize];
        let outputs = vec![false; circuit.output_count as usize];
        let state = circuit.initial_state();
        Self { circuit, inputs, state, outputs }
    }
}

impl Simulator for InterpreterSim {
    fn set_input(&mut self, index: usize, value: bool) {
        if let Some(slot) = self.inputs.get_mut(index) {
            *slot = value;
        }
    }

    fn output(&self, index: usize) -> bool {
        self.outputs.get(index).copied().unwrap_or(false)
    }

    fn tick(&mut self) {
        Interpreter.tick(&self.circuit, &self.inputs, &mut self.state, &mut self.outputs);
    }
}

/// Which [`ryzr_backend`] engine to run a board on. Available only with the
/// `engines` feature; the default [`Sim::new`] never needs it.
#[cfg(feature = "engines")]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EngineKind {
    Scalar,
    Event,
    Packed,
    PackedJit,
    Threaded,
    Hybrid,
}

#[cfg(feature = "engines")]
struct EngineSim(Box<dyn ryzr_backend::Engine>);

#[cfg(feature = "engines")]
impl Simulator for EngineSim {
    fn set_input(&mut self, index: usize, value: bool) {
        self.0.set_input(index, value);
    }

    fn output(&self, index: usize) -> bool {
        self.0.output(index)
    }

    fn tick(&mut self) {
        self.0.tick();
    }
}

/// A running board: a compiled circuit plus the maps that turn ticks into cell
/// readouts and cell clicks into inputs.
pub struct Sim {
    inner: Box<dyn Simulator>,
    map: BoardMap,
    /// Precomputed nets to sample for each cell's lit state.
    display: HashMap<Cell, Vec<NetId>>,
    /// Current switch positions, so the UI can show them without a tick.
    switch_state: HashMap<Cell, bool>,
    /// LED cell → output index, for direct lookup.
    led_index: HashMap<Cell, usize>,
    ticks: u64,
}

impl Sim {
    /// Compile `board` and run it on the reference interpreter.
    pub fn new(board: &Board) -> Result<Self, CompileError> {
        let Compiled { circuit, map } = compile::compile(board)?;
        Ok(Self::assemble(board, Box::new(InterpreterSim::new(circuit)), map))
    }

    /// Compile `board` and run it on a specific [`ryzr_backend`] engine.
    #[cfg(feature = "engines")]
    pub fn with_engine(board: &Board, kind: EngineKind) -> Result<Self, CompileError> {
        use ryzr_backend::{
            Engine, EventEngine, HybridEngine, PackedEngine, PackedJitEngine, ScalarEngine,
            ThreadedEngine,
        };
        let Compiled { circuit, map } = compile::compile(board)?;
        let engine: Box<dyn Engine> = match kind {
            EngineKind::Scalar => Box::new(ScalarEngine::new(&circuit)),
            EngineKind::Event => Box::new(EventEngine::new(&circuit)),
            EngineKind::Packed => Box::new(PackedEngine::new(&circuit)),
            EngineKind::PackedJit => Box::new(PackedJitEngine::new(&circuit)),
            EngineKind::Threaded => Box::new(ThreadedEngine::new(&circuit)),
            EngineKind::Hybrid => Box::new(HybridEngine::new(&circuit)),
        };
        Ok(Self::assemble(board, Box::new(EngineSim(engine)), map))
    }

    /// Build the cell-lookup tables shared by every constructor.
    fn assemble(board: &Board, inner: Box<dyn Simulator>, map: BoardMap) -> Self {
        let display =
            board.iter().map(|(cell, _)| (cell, map.nets.display_nets(board, cell))).collect();
        let switch_state = map.switch_inputs.keys().map(|&cell| (cell, false)).collect();
        let led_index = map.leds.iter().copied().collect();
        Self { inner, map, display, switch_state, led_index, ticks: 0 }
    }

    /// Advance the board by one tick.
    pub fn tick(&mut self) {
        self.inner.tick();
        self.ticks += 1;
    }

    /// Advance the board by `ticks` ticks.
    pub fn run(&mut self, ticks: u64) {
        for _ in 0..ticks {
            self.inner.tick();
        }
        self.ticks += ticks;
    }

    /// Ticks elapsed since construction.
    pub fn ticks(&self) -> u64 {
        self.ticks
    }

    /// Number of nets in the compiled board.
    pub fn net_count(&self) -> usize {
        self.map.net_count
    }

    /// Whether net `net` is currently high. Net `n` lives at output index `n`.
    pub fn net_value(&self, net: NetId) -> bool {
        self.inner.output(net)
    }

    /// Whether `cell` should render as energised — true if any net it displays
    /// is high.
    pub fn cell_on(&self, cell: Cell) -> bool {
        self.display.get(&cell).is_some_and(|nets| nets.iter().any(|&n| self.net_value(n)))
    }

    /// Whether the LED at `cell` is lit.
    pub fn led_on(&self, cell: Cell) -> bool {
        self.led_index.get(&cell).is_some_and(|&i| self.inner.output(i))
    }

    /// Whether `cell` holds a toggleable switch.
    pub fn is_switch(&self, cell: Cell) -> bool {
        self.switch_state.contains_key(&cell)
    }

    /// The current position of the switch at `cell` (false if it isn't one).
    pub fn switch_on(&self, cell: Cell) -> bool {
        self.switch_state.get(&cell).copied().unwrap_or(false)
    }

    /// Drive the switch at `cell` to `on`. No-op if `cell` isn't a switch.
    pub fn set_switch(&mut self, cell: Cell, on: bool) {
        if let Some(&index) = self.map.switch_inputs.get(&cell) {
            self.switch_state.insert(cell, on);
            self.inner.set_input(index, on);
        }
    }

    /// Flip the switch at `cell`.
    pub fn toggle_switch(&mut self, cell: Cell) {
        let now = self.switch_on(cell);
        self.set_switch(cell, !now);
    }
}
