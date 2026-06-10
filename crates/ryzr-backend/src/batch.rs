//! Bit-parallel (SWAR) engine: 64 independent circuit instances per word.
//!
//! Each signal is one `u64`; bit *k* belongs to instance *k*. Every gate
//! evaluates all 64 instances with a single ALU op, so the amortized cost
//! is ~1/64th of a scalar gate evaluation. This is the classic
//! bit-parallel logic simulation technique: exhaustive truth-table checks,
//! test-vector sweeps, or many independent worlds at once — with zero loss
//! of fidelity per instance.

use std::sync::Arc;

use crate::Engine;
use crate::compile::{Compiled, Op};

pub const LANES: usize = 64;

pub struct BatchEngine {
    tape: Arc<Compiled>,
    values: Vec<u64>,
    reg_scratch: Vec<u64>,
}

impl BatchEngine {
    pub fn new(circuit: &ryzr_core::Circuit) -> Self {
        Self::with_tape(Arc::new(Compiled::new(circuit)))
    }

    pub fn with_tape(tape: Arc<Compiled>) -> Self {
        let mut values = vec![0u64; tape.slot_count()];
        for &(slot, value) in &tape.const_slots {
            values[slot as usize] = if value != 0 { u64::MAX } else { 0 };
        }
        let mut reg_scratch = vec![0u64; tape.register_count()];
        for (r, &init) in tape.reg_initial.iter().enumerate() {
            let broadcast = if init != 0 { u64::MAX } else { 0 };
            reg_scratch[r] = broadcast;
            let slot = tape.reg_out_slots[r];
            if slot != u32::MAX {
                values[slot as usize] = broadcast;
            }
        }
        Self { tape, values, reg_scratch }
    }

    /// Set one input across all 64 instances at once (bit k = instance k).
    pub fn set_input_mask(&mut self, index: usize, mask: u64) {
        self.values[self.tape.input_slots[index] as usize] = mask;
    }

    pub fn set_input_lane(&mut self, index: usize, lane: usize, value: bool) {
        debug_assert!(lane < LANES);
        let slot = self.tape.input_slots[index] as usize;
        let bit = 1u64 << lane;
        if value {
            self.values[slot] |= bit;
        } else {
            self.values[slot] &= !bit;
        }
    }

    pub fn output_mask(&self, index: usize) -> u64 {
        self.values[self.tape.output_slots[index] as usize]
    }

    pub fn output_lane(&self, index: usize, lane: usize) -> bool {
        debug_assert!(lane < LANES);
        self.output_mask(index) >> lane & 1 != 0
    }
}

/// SAFETY contract identical to the scalar engine: operand indices were
/// validated `< i` for every gate slot `i` in `Compiled::new`.
#[inline(always)]
fn eval_run<const AR: usize, F: Fn(u64, u64, u64) -> u64>(
    tape: &Compiled,
    values: &mut [u64],
    start: usize,
    end: usize,
    f: F,
) {
    debug_assert!(end <= values.len() && end <= tape.a.len());
    let v = values.as_mut_ptr();
    for i in start..end {
        // SAFETY: see function-level contract.
        unsafe {
            let av = *v.add(*tape.a.get_unchecked(i) as usize);
            let bv = if AR > 1 { *v.add(*tape.b.get_unchecked(i) as usize) } else { 0 };
            let cv = if AR > 2 { *v.add(*tape.c.get_unchecked(i) as usize) } else { 0 };
            *v.add(i) = f(av, bv, cv);
        }
    }
}

/// Dispatch a run range to its monomorphized loop. The 64-lane twin of
/// `scalar::eval_runs`; shared with the hybrid engine's interpreted plan.
#[inline(always)]
pub(crate) fn eval_runs(tape: &Compiled, values: &mut [u64], run_range: core::ops::Range<usize>) {
    for run in &tape.runs[run_range] {
        let (s, e) = (run.start as usize, run.end as usize);
        match run.op {
            Op::And => eval_run::<2, _>(tape, values, s, e, |a, b, _| a & b),
            Op::Or => eval_run::<2, _>(tape, values, s, e, |a, b, _| a | b),
            Op::Xor => eval_run::<2, _>(tape, values, s, e, |a, b, _| a ^ b),
            Op::Nand => eval_run::<2, _>(tape, values, s, e, |a, b, _| !(a & b)),
            Op::Nor => eval_run::<2, _>(tape, values, s, e, |a, b, _| !(a | b)),
            Op::Xnor => eval_run::<2, _>(tape, values, s, e, |a, b, _| !(a ^ b)),
            Op::Not => eval_run::<1, _>(tape, values, s, e, |a, _, _| !a),
            Op::Buf => eval_run::<1, _>(tape, values, s, e, |a, _, _| a),
            Op::Mux => eval_run::<3, _>(tape, values, s, e, |sel, t, e_| (sel & t) | (!sel & e_)),
        }
    }
}

/// Evaluate a single gate against settled lower levels. The 64-lane twin of
/// the threaded engine's per-gate kernel; used for wide levels in the
/// hybrid engine's interpreted plan.
#[cfg(all(feature = "jit", feature = "rayon"))]
#[inline(always)]
pub(crate) fn eval_gate(tape: &Compiled, lower: &[u64], i: usize) -> u64 {
    // SAFETY: operands of a gate live at strictly lower levels, hence at
    // slots `< level.start == lower.len()`; validated in `Compiled::new`.
    let (a, b, c) = unsafe {
        (
            *lower.get_unchecked(tape.a[i] as usize),
            *lower.get_unchecked(tape.b[i] as usize),
            *lower.get_unchecked(tape.c[i] as usize),
        )
    };
    match tape.ops[i] {
        Op::And => a & b,
        Op::Or => a | b,
        Op::Xor => a ^ b,
        Op::Nand => !(a & b),
        Op::Nor => !(a | b),
        Op::Xnor => !(a ^ b),
        Op::Not => !a,
        Op::Buf => a,
        Op::Mux => (a & b) | (!a & c),
    }
}

/// Scatter the latched next-state into the register-output slots (the
/// clock edge), 64 lanes at a time.
#[inline(always)]
pub(crate) fn apply_edge(tape: &Compiled, values: &mut [u64], scratch: &[u64]) {
    for (r, &slot) in tape.reg_out_slots.iter().enumerate() {
        if slot != u32::MAX {
            values[slot as usize] = scratch[r];
        }
    }
}

/// Capture every register's next state from the settled values.
#[inline(always)]
pub(crate) fn capture_next(tape: &Compiled, values: &[u64], scratch: &mut [u64]) {
    for (r, &slot) in tape.reg_in_slots.iter().enumerate() {
        scratch[r] = values[slot as usize];
    }
}

impl Engine for BatchEngine {
    fn name(&self) -> &'static str {
        "batch64"
    }

    fn input_count(&self) -> usize {
        self.tape.input_count()
    }

    fn output_count(&self) -> usize {
        self.tape.output_count()
    }

    /// Broadcasts to all 64 lanes; use [`set_input_lane`](Self::set_input_lane)
    /// for per-instance control.
    fn set_input(&mut self, index: usize, value: bool) {
        self.set_input_mask(index, if value { u64::MAX } else { 0 });
    }

    /// Reads lane 0.
    fn output(&self, index: usize) -> bool {
        self.output_lane(index, 0)
    }

    fn tick(&mut self) {
        apply_edge(&self.tape, &mut self.values, &self.reg_scratch);
        eval_runs(&self.tape, &mut self.values, 0..self.tape.runs.len());
        capture_next(&self.tape, &self.values, &mut self.reg_scratch);
    }
}
