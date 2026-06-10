//! Dense forward-pass engine.
//!
//! The whole tick is a linear sweep over the gate region of the tape. Gates
//! were sorted by `(level, op)` at compile time, so dispatch happens once
//! per *run* instead of once per gate, the instruction stream is a tight
//! gather-op-store loop, and the value buffer (1 byte per signal) is walked
//! strictly forward — the access pattern the prefetcher likes most.

use std::sync::Arc;

use crate::Engine;
use crate::compile::{Compiled, Op};

pub struct ScalarEngine {
    tape: Arc<Compiled>,
    values: Vec<u8>,
    reg_scratch: Vec<u8>,
}

impl ScalarEngine {
    pub fn new(circuit: &ryzr_core::Circuit) -> Self {
        Self::with_tape(Arc::new(Compiled::new(circuit)))
    }

    pub fn with_tape(tape: Arc<Compiled>) -> Self {
        let values = tape.initial_values();
        let reg_scratch = tape.reg_initial.clone();
        Self { tape, values, reg_scratch }
    }

    pub fn tape(&self) -> &Arc<Compiled> {
        &self.tape
    }
}

/// Evaluate one homogeneous run of `AR`-ary gates.
///
/// SAFETY contract (established by `Compiled::new` validation): for every
/// gate slot `i`, operand indices `a[i]`, `b[i]`, `c[i]` are `< i`, and the
/// run range lies within the value buffer.
#[inline(always)]
fn eval_run<const AR: usize, F: Fn(u8, u8, u8) -> u8>(
    tape: &Compiled,
    values: &mut [u8],
    start: usize,
    end: usize,
    f: F,
) {
    debug_assert!(end <= values.len() && end <= tape.a.len());
    let v = values.as_mut_ptr();
    for i in start..end {
        // SAFETY: see function-level contract; indices were validated once
        // at compile time, removing per-gate bounds checks from the hot loop.
        unsafe {
            let av = *v.add(*tape.a.get_unchecked(i) as usize);
            let bv = if AR > 1 { *v.add(*tape.b.get_unchecked(i) as usize) } else { 0 };
            let cv = if AR > 2 { *v.add(*tape.c.get_unchecked(i) as usize) } else { 0 };
            *v.add(i) = f(av, bv, cv);
        }
    }
}

/// Dispatch a run to its monomorphized loop. Shared by scalar and threaded
/// engines.
#[inline(always)]
pub(crate) fn eval_runs(tape: &Compiled, values: &mut [u8], run_range: core::ops::Range<usize>) {
    for run in &tape.runs[run_range] {
        let (s, e) = (run.start as usize, run.end as usize);
        match run.op {
            Op::And => eval_run::<2, _>(tape, values, s, e, |a, b, _| a & b),
            Op::Or => eval_run::<2, _>(tape, values, s, e, |a, b, _| a | b),
            Op::Xor => eval_run::<2, _>(tape, values, s, e, |a, b, _| a ^ b),
            Op::Nand => eval_run::<2, _>(tape, values, s, e, |a, b, _| (a & b) ^ 1),
            Op::Nor => eval_run::<2, _>(tape, values, s, e, |a, b, _| (a | b) ^ 1),
            Op::Xnor => eval_run::<2, _>(tape, values, s, e, |a, b, _| a ^ b ^ 1),
            Op::Not => eval_run::<1, _>(tape, values, s, e, |a, _, _| a ^ 1),
            Op::Buf => eval_run::<1, _>(tape, values, s, e, |a, _, _| a),
            Op::Mux => eval_run::<3, _>(tape, values, s, e, |s_, t, e_| (s_ & t) | ((s_ ^ 1) & e_)),
        }
    }
}

/// Apply the clock edge captured at the end of the previous tick: scatter
/// the latched next-state into the register-output slots.
///
/// The latch is split across the tick boundary (capture at tick end, apply
/// at next tick start) so that after `tick()` returns, the value buffer
/// holds the settled *pre-edge* combinational values — exactly what
/// `output()` must observe.
#[inline(always)]
pub(crate) fn apply_edge(tape: &Compiled, values: &mut [u8], scratch: &[u8]) {
    for (r, &slot) in tape.reg_out_slots.iter().enumerate() {
        if slot != u32::MAX {
            values[slot as usize] = scratch[r];
        }
    }
}

/// Capture every register's next state from the settled values.
#[inline(always)]
pub(crate) fn capture_next(tape: &Compiled, values: &[u8], scratch: &mut [u8]) {
    for (r, &slot) in tape.reg_in_slots.iter().enumerate() {
        scratch[r] = values[slot as usize];
    }
}

impl Engine for ScalarEngine {
    fn name(&self) -> &'static str {
        "scalar"
    }

    fn input_count(&self) -> usize {
        self.tape.input_count()
    }

    fn output_count(&self) -> usize {
        self.tape.output_count()
    }

    fn set_input(&mut self, index: usize, value: bool) {
        self.values[self.tape.input_slots[index] as usize] = u8::from(value);
    }

    fn output(&self, index: usize) -> bool {
        self.values[self.tape.output_slots[index] as usize] != 0
    }

    fn tick(&mut self) {
        apply_edge(&self.tape, &mut self.values, &self.reg_scratch);
        eval_runs(&self.tape, &mut self.values, 0..self.tape.runs.len());
        capture_next(&self.tape, &self.values, &mut self.reg_scratch);
    }
}
