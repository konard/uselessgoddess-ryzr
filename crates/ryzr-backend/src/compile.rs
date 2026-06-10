//! Circuit compilation: turns the builder-level [`Circuit`] into a flat,
//! cache-friendly evaluation tape shared by every engine.
//!
//! The pipeline:
//!
//! 1. **Levelize** — `level(gate) = 1 + max(level(inputs))`, sources
//!    (constants, external inputs, register outputs) sit at level 0.
//! 2. **Schedule** — signals are reordered by `(level, op)` so that gates of
//!    the same kind form long homogeneous runs. Within a level gates are
//!    independent, so any permutation is valid; sorting by op turns the
//!    interpreter's per-gate dispatch into per-run dispatch and gives
//!    threaded engines contiguous disjoint write ranges.
//! 3. **Index** — operands become plain `u32` slot indices into one dense
//!    value buffer. A CSR successor map supports event-driven evaluation.
//!
//! Every operand index is validated here once, so engines can use unchecked
//! indexing in their hot loops.

use cranelift_entity::EntityRef;
use ryzr_core::{Circuit, GateOp, InstData};

/// Gate opcode on the tape. Sources are not ops — they live in the value
/// buffer and are written directly (constants once, inputs by the caller,
/// register outputs by the sequential update).
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[repr(u8)]
pub enum Op {
    And,
    Or,
    Xor,
    Nand,
    Nor,
    Xnor,
    Not,
    Buf,
    Mux,
}

impl Op {
    fn from_gate(op: GateOp) -> Self {
        match op {
            GateOp::And => Op::And,
            GateOp::Or => Op::Or,
            GateOp::Xor => Op::Xor,
            GateOp::Nand => Op::Nand,
            GateOp::Nor => Op::Nor,
            GateOp::Xnor => Op::Xnor,
            GateOp::Not => Op::Not,
            GateOp::Buf => Op::Buf,
            GateOp::Mux => Op::Mux,
        }
    }
}

/// A maximal contiguous range of identical gates within one level.
#[derive(Copy, Clone, Debug)]
pub struct Run {
    pub op: Op,
    pub start: u32,
    pub end: u32,
}

/// One level of the schedule: `[start, end)` slot range and the runs inside.
#[derive(Copy, Clone, Debug)]
pub struct Level {
    pub start: u32,
    pub end: u32,
    pub run_start: u32,
    pub run_end: u32,
}

/// Flat compiled form of a [`Circuit`]. Struct-of-arrays: one entry per
/// signal in schedule order (sources first, then gates level by level).
pub struct Compiled {
    /// Operand slot indices; unused operands are 0. Only meaningful for
    /// slots in the gate region (`slot >= gate_start`).
    pub a: Vec<u32>,
    pub b: Vec<u32>,
    pub c: Vec<u32>,
    /// Opcode per slot; sources carry an arbitrary value, never read.
    pub ops: Vec<Op>,
    /// First gate slot; everything before it is a source.
    pub gate_start: u32,

    pub runs: Vec<Run>,
    pub levels: Vec<Level>,

    /// Slot of each external input, indexed by input index.
    pub input_slots: Vec<u32>,
    /// `(slot, value)` for every constant.
    pub const_slots: Vec<(u32, u8)>,
    /// Slot holding each register's *output*, indexed by register.
    pub reg_out_slots: Vec<u32>,
    /// Slot computing each register's *next state*, indexed by register.
    pub reg_in_slots: Vec<u32>,
    /// Initial register values.
    pub reg_initial: Vec<u8>,
    /// Slot of each declared output.
    pub output_slots: Vec<u32>,

    /// CSR successor map: slot -> dependent gate slots. Used by the
    /// event-driven engine to propagate only actual changes.
    pub succ_offsets: Vec<u32>,
    pub succ: Vec<u32>,
    /// Level of each slot (0 for sources).
    pub slot_level: Vec<u32>,
}

impl Compiled {
    pub fn new(circuit: &Circuit) -> Self {
        // Netlist optimization (const folding, CSE, DCE, mux strength
        // reduction) preserves declared outputs and register next-state
        // functions bit-for-bit; the differential suite checks every engine
        // against the naive interpreter running the *unoptimized* netlist.
        let circuit = &ryzr_core::optimize(circuit);
        let n = circuit.insts.len();

        // 1. Levelize. Instructions are already topo-ordered (a finish()
        //    invariant), so a single forward pass suffices.
        let mut level = vec![0u32; n];
        for (sig, inst) in circuit.insts.iter() {
            if let InstData::Gate { inputs, .. } = &inst.data {
                let max = inputs
                    .as_slice(&circuit.list_pool)
                    .iter()
                    .map(|s| level[s.index()])
                    .max()
                    .unwrap_or(0);
                level[sig.index()] = max + 1;
            }
        }

        // 2. Schedule: stable order by (level, op). `pos` maps old signal
        //    index -> slot on the tape.
        let op_key = |idx: usize| -> u8 {
            match &circuit.insts[ryzr_core::Signal::new(idx)].data {
                InstData::Gate { op, .. } => Op::from_gate(*op) as u8,
                _ => 0,
            }
        };
        let mut schedule: Vec<u32> = (0..n as u32).collect();
        schedule.sort_by_key(|&i| (level[i as usize], op_key(i as usize), i));

        let mut pos = vec![0u32; n];
        for (slot, &old) in schedule.iter().enumerate() {
            pos[old as usize] = slot as u32;
        }

        // 3. Tape arrays.
        let mut a = vec![0u32; n];
        let mut b = vec![0u32; n];
        let mut c = vec![0u32; n];
        let mut ops = vec![Op::Buf; n];
        let mut slot_level = vec![0u32; n];

        let mut input_slots = vec![u32::MAX; circuit.input_count as usize];
        let mut const_slots = Vec::new();
        let mut reg_out_slots = vec![u32::MAX; circuit.regs.len()];
        let mut gate_start = n as u32;

        for (slot, &old) in schedule.iter().enumerate() {
            let slot_u = slot as u32;
            let sig = ryzr_core::Signal::new(old as usize);
            slot_level[slot] = level[old as usize];

            match &circuit.insts[sig].data {
                InstData::Const { value } => const_slots.push((slot_u, u8::from(*value))),
                InstData::Input { index } => input_slots[*index as usize] = slot_u,
                InstData::RegisterOutput { reg } => reg_out_slots[reg.index()] = slot_u,
                InstData::Gate { op, inputs } => {
                    gate_start = gate_start.min(slot_u);
                    ops[slot] = Op::from_gate(*op);
                    let ins = inputs.as_slice(&circuit.list_pool);
                    a[slot] = pos[ins[0].index()];
                    if ins.len() > 1 {
                        b[slot] = pos[ins[1].index()];
                    }
                    if ins.len() > 2 {
                        c[slot] = pos[ins[2].index()];
                    }
                }
            }
        }
        if gate_start == n as u32 && n > 0 {
            // No gates at all; keep gate_start = n (empty gate region).
        }

        // 4. Runs and levels over the gate region.
        let mut runs = Vec::new();
        let mut levels = Vec::new();
        let mut slot = gate_start as usize;
        while slot < n {
            let lvl = slot_level[slot];
            let level_start = slot as u32;
            let run_start = runs.len() as u32;
            while slot < n && slot_level[slot] == lvl {
                let op = ops[slot];
                let start = slot as u32;
                while slot < n && slot_level[slot] == lvl && ops[slot] == op {
                    slot += 1;
                }
                runs.push(Run { op, start, end: slot as u32 });
            }
            levels.push(Level {
                start: level_start,
                end: slot as u32,
                run_start,
                run_end: runs.len() as u32,
            });
        }

        // 5. Register wiring and outputs.
        let mut reg_in_slots = vec![0u32; circuit.regs.len()];
        let mut reg_initial = vec![0u8; circuit.regs.len()];
        for (reg, data) in circuit.regs.iter() {
            reg_in_slots[reg.index()] = pos[data.data_input.index()];
            reg_initial[reg.index()] = u8::from(data.initial);
        }
        let output_slots: Vec<u32> =
            circuit.output_signals.iter().map(|s| pos[s.index()]).collect();

        // 6. CSR successors (slot -> dependent gate slots).
        let mut counts = vec![0u32; n + 1];
        for slot in gate_start as usize..n {
            counts[a[slot] as usize + 1] += 1;
            if arity(ops[slot]) > 1 {
                counts[b[slot] as usize + 1] += 1;
            }
            if arity(ops[slot]) > 2 {
                counts[c[slot] as usize + 1] += 1;
            }
        }
        for i in 1..=n {
            counts[i] += counts[i - 1];
        }
        let succ_offsets = counts.clone();
        let mut cursor = counts;
        let mut succ = vec![0u32; *succ_offsets.last().unwrap_or(&0) as usize];
        for slot in gate_start as usize..n {
            let mut push = |src: u32| {
                succ[cursor[src as usize] as usize] = slot as u32;
                cursor[src as usize] += 1;
            };
            push(a[slot]);
            if arity(ops[slot]) > 1 {
                push(b[slot]);
            }
            if arity(ops[slot]) > 2 {
                push(c[slot]);
            }
        }

        // Operand validation: every engine relies on `a/b/c < slot` for gate
        // slots (strict topo order) to use unchecked reads, and the threaded
        // engine additionally relies on operands living at *strictly lower
        // levels* (so a level's slots can be written while only reading the
        // region below `level.start`).
        for slot in gate_start as usize..n {
            let ar = arity(ops[slot]);
            let lvl = slot_level[slot];
            let check = |operand: u32, name: &str| {
                assert!(
                    (operand as usize) < slot && slot_level[operand as usize] < lvl,
                    "operand {name} out of order at slot {slot}"
                );
            };
            check(a[slot], "a");
            if ar > 1 {
                check(b[slot], "b");
            }
            if ar > 2 {
                check(c[slot], "c");
            }
        }

        Self {
            a,
            b,
            c,
            ops,
            gate_start,
            runs,
            levels,
            input_slots,
            const_slots,
            reg_out_slots,
            reg_in_slots,
            reg_initial,
            output_slots,
            succ_offsets,
            succ,
            slot_level,
        }
    }

    pub fn slot_count(&self) -> usize {
        self.ops.len()
    }

    pub fn input_count(&self) -> usize {
        self.input_slots.len()
    }

    pub fn output_count(&self) -> usize {
        self.output_slots.len()
    }

    pub fn register_count(&self) -> usize {
        self.reg_out_slots.len()
    }

    pub fn successors(&self, slot: u32) -> &[u32] {
        let lo = self.succ_offsets[slot as usize] as usize;
        let hi = self.succ_offsets[slot as usize + 1] as usize;
        &self.succ[lo..hi]
    }

    /// Fresh value buffer with constants and register initial values applied.
    pub fn initial_values(&self) -> Vec<u8> {
        let mut values = vec![0u8; self.slot_count()];
        for &(slot, value) in &self.const_slots {
            values[slot as usize] = value;
        }
        for (reg, &slot) in self.reg_out_slots.iter().enumerate() {
            if slot != u32::MAX {
                values[slot as usize] = self.reg_initial[reg];
            }
        }
        values
    }
}

pub fn arity(op: Op) -> usize {
    match op {
        Op::Not | Op::Buf => 1,
        Op::Mux => 3,
        _ => 2,
    }
}
