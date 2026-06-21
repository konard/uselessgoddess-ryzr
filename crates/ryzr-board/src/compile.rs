//! Lower a [`Board`] into a [`ryzr_core::Circuit`].
//!
//! # The unit-delay trick
//!
//! Virtual Circuit Board propagates one tile per tick, which lets latches and
//! oscillators exist. ryzr forbids combinational cycles outright. We reconcile
//! the two by giving **every driver its own register**: a gate computes
//! `next <= op(inputs)` and exposes the register's *previous* value as its
//! output. Feedback then always passes through a register, so no combinational
//! cycle is ever constructed, yet a signal still advances exactly one driver per
//! tick — VCB's model, expressed natively in ryzr.
//!
//! # Shape of the lowering
//!
//! * Each net becomes the wired-OR of its drivers (low when undriven).
//! * Each gate folds its op across the distinct nets on its input sides
//!   ([`build_gate`]); the empty case is defined, never floating.
//! * Every net is exported as an output so the simulator can read it back, and
//!   each LED gets one more output for its lit state.

use std::collections::HashMap;
use std::fmt;

use ryzr_core::{Circuit, CircuitBuilder, Reg, Signal};

use crate::board::{Board, Cell};
use crate::net::{self, NetId, Nets};
use crate::tile::{Direction, GateKind, Tile};

/// Everything the simulator needs besides the circuit itself.
#[derive(Clone, Debug)]
pub struct BoardMap {
    /// Number of nets. Net `n` is exported as output index `n`.
    pub net_count: usize,
    /// Switch cell → its circuit input index.
    pub switch_inputs: HashMap<Cell, usize>,
    /// LED cell → its circuit output index.
    pub leds: Vec<(Cell, usize)>,
    /// Net assignment, kept for rendering and queries.
    pub nets: Nets,
}

/// A compiled board: the runnable circuit plus the maps that tie it back to
/// cells.
pub struct Compiled {
    pub circuit: Circuit,
    pub map: BoardMap,
}

/// Why a board failed to compile. In practice unreachable — the lowering never
/// emits a cycle or leaves a register undriven — but surfaced rather than
/// panicked, so a malformed board degrades gracefully.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CompileError {
    Build(String),
}

impl fmt::Display for CompileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CompileError::Build(msg) => write!(f, "circuit construction failed: {msg}"),
        }
    }
}

impl std::error::Error for CompileError {}

/// OR a list of signals together, yielding `low` when the list is empty.
fn or_fold(b: &mut CircuitBuilder, ins: &[Signal], low: Signal) -> Signal {
    let mut it = ins.iter().copied();
    match it.next() {
        None => low,
        Some(first) => it.fold(first, |acc, s| b.or(acc, s)),
    }
}

/// AND a list of signals together, yielding `low` when the list is empty.
fn and_fold(b: &mut CircuitBuilder, ins: &[Signal], low: Signal) -> Signal {
    let mut it = ins.iter().copied();
    match it.next() {
        None => low,
        Some(first) => it.fold(first, |acc, s| b.and(acc, s)),
    }
}

/// XOR a list of signals together (parity), yielding `low` when empty.
fn xor_fold(b: &mut CircuitBuilder, ins: &[Signal], low: Signal) -> Signal {
    let mut it = ins.iter().copied();
    match it.next() {
        None => low,
        Some(first) => it.fold(first, |acc, s| b.xor(acc, s)),
    }
}

/// Combine a gate's input nets according to its kind.
///
/// `Not` folds as a multi-input NOR (negated OR), which is what a one-input case
/// reduces to anyway and keeps a floating inverter reading high.
fn build_gate(b: &mut CircuitBuilder, kind: GateKind, ins: &[Signal], low: Signal) -> Signal {
    match kind {
        GateKind::And => and_fold(b, ins, low),
        GateKind::Or | GateKind::Buf => or_fold(b, ins, low),
        GateKind::Xor => xor_fold(b, ins, low),
        GateKind::Nand => {
            let v = and_fold(b, ins, low);
            b.not(v)
        }
        GateKind::Nor | GateKind::Not => {
            let v = or_fold(b, ins, low);
            b.not(v)
        }
        GateKind::Xnor => {
            let v = xor_fold(b, ins, low);
            b.not(v)
        }
    }
}

/// Distinct input nets feeding a device at `cell`, skipping `exclude` (a gate's
/// output side). Order follows [`Direction::ALL`] for reproducibility.
fn input_nets(nets: &Nets, board: &Board, cell: Cell, exclude: Option<Direction>) -> Vec<NetId> {
    let mut seen: Vec<NetId> = Vec::new();
    for side in Direction::ALL {
        if Some(side) == exclude {
            continue;
        }
        if let Some(net) = nets.neighbour_net(board, cell, side)
            && !seen.contains(&net)
        {
            seen.push(net);
        }
    }
    seen
}

/// Lower `board` into a runnable circuit.
pub fn compile(board: &Board) -> Result<Compiled, CompileError> {
    let nets = net::extract(board);
    let mut builder = CircuitBuilder::new();
    let low = builder.const_val(false);

    // Pass A: one output signal per driver. Gates and clocks get a register and
    // are driven in pass C; switches and sources resolve immediately.
    let mut driver_sig: HashMap<Cell, Signal> = HashMap::new();
    let mut switch_inputs: HashMap<Cell, usize> = HashMap::new();
    let mut gate_regs: Vec<(Reg, Cell, GateKind, Direction)> = Vec::new();
    let mut clock_regs: Vec<(Reg, Signal)> = Vec::new();

    for (cell, tile) in board.iter() {
        match tile {
            Tile::Gate { kind, facing } => {
                let (reg, out) = builder.reg(format!("g{}", gate_regs.len()), false);
                driver_sig.insert(cell, out);
                gate_regs.push((reg, cell, kind, facing));
            }
            Tile::Clock => {
                let (reg, out) = builder.reg(format!("clk{}", clock_regs.len()), false);
                driver_sig.insert(cell, out);
                clock_regs.push((reg, out));
            }
            Tile::Switch => {
                let index = switch_inputs.len();
                let sig = builder.input(format!("sw{index}"));
                driver_sig.insert(cell, sig);
                switch_inputs.insert(cell, index);
            }
            Tile::Source { on } => {
                let sig = if on { builder.const_val(true) } else { low };
                driver_sig.insert(cell, sig);
            }
            _ => {}
        }
    }

    // Pass B: each net is the wired-OR of its drivers. Iterate the board (not the
    // hash map) so the circuit is byte-for-byte reproducible.
    let mut net_drivers: Vec<Vec<Signal>> = vec![Vec::new(); nets.count()];
    for (cell, _) in board.iter() {
        if let (Some(&sig), Some(net)) = (driver_sig.get(&cell), nets.driver_net(cell)) {
            net_drivers[net].push(sig);
        }
    }
    let net_sig: Vec<Signal> =
        net_drivers.iter().map(|drivers| or_fold(&mut builder, drivers, low)).collect();

    // Pass C: wire every register's next-state input.
    for &(reg, cell, kind, facing) in &gate_regs {
        let ins: Vec<Signal> = input_nets(&nets, board, cell, Some(facing))
            .into_iter()
            .map(|net| net_sig[net])
            .collect();
        let next = build_gate(&mut builder, kind, &ins, low);
        builder.drive(reg, next);
    }
    for &(reg, out) in &clock_regs {
        let next = builder.not(out);
        builder.drive(reg, next);
    }

    // Pass D: export every net (index == net id), then one output per LED.
    for (net, &sig) in net_sig.iter().enumerate() {
        builder.output(format!("net{net}"), sig);
    }
    let mut leds: Vec<(Cell, usize)> = Vec::new();
    for (cell, tile) in board.iter() {
        if !matches!(tile, Tile::Led) {
            continue;
        }
        let ins: Vec<Signal> =
            input_nets(&nets, board, cell, None).into_iter().map(|net| net_sig[net]).collect();
        let sig = or_fold(&mut builder, &ins, low);
        let output_index = nets.count() + leds.len();
        builder.output(format!("led{}", leds.len()), sig);
        leds.push((cell, output_index));
    }

    let circuit = builder.finish().map_err(|e| CompileError::Build(e.to_string()))?;
    Ok(Compiled { circuit, map: BoardMap { net_count: nets.count(), switch_inputs, leds, nets } })
}
