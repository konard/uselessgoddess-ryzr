//! The unified engine: SWAR × JIT × rayon in one tick.
//!
//! Each technique multiplies a different axis, and they compose because
//! they are orthogonal:
//!
//! - **SWAR** packs 64 independent circuit instances into every `u64`
//!   value slot, dividing the per-gate cost by 64 (same lane discipline as
//!   [`BatchEngine`](crate::BatchEngine)).
//! - **JIT** compiles the settle pass to straight-line native code with
//!   operand offsets baked in as immediates, eliminating interpreter
//!   dispatch and keeping hot intermediates in registers (same chunking as
//!   [`JitEngine`](crate::JitEngine), but on 64-bit words).
//! - **rayon** fans wide levels out across cores. Gates within a level are
//!   independent by construction, so a wide level is pre-split into
//!   per-thread native functions that run concurrently; each writes only
//!   its own slot range and reads only strictly lower levels.
//!
//! The execution plan is baked at construction: consecutive narrow levels
//! merge into serial chunks (a per-level rayon barrier costs microseconds —
//! ruinous at millions of ticks per second), and only levels wider than the
//! threshold are split for parallel execution. Per instance the results are
//! bit-for-bit identical to every other engine; there are no semantic
//! shortcuts anywhere.

use std::sync::Arc;

use cranelift_jit::JITModule;
use rayon::prelude::*;

use crate::Engine;
use crate::compile::Compiled;
use crate::jit::{CHUNK, TickFn, compile_ranges};

pub const LANES: usize = 64;

/// Minimum level width (in gates) before the level is split across threads.
const PARALLEL_THRESHOLD: usize = 1 << 15;

/// One step of the baked execution plan.
enum Step {
    /// A straight-line chunk run on the calling thread. Chunks within and
    /// across narrow levels — slots are in topo order, so any serial cut
    /// is valid.
    Serial(TickFn),
    /// Disjoint pieces of a single wide level, run concurrently.
    Parallel(Vec<TickFn>),
}

/// [`Step`] before compilation: the slot ranges each function will cover.
enum Spec {
    Serial(core::ops::Range<usize>),
    Parallel(Vec<core::ops::Range<usize>>),
}

/// Shares the value buffer's base pointer with the rayon workers.
struct SyncPtr(*mut u8);

impl SyncPtr {
    /// Borrow-based accessor so closures capture the wrapper (which is
    /// `Sync`), not the raw pointer field (which is not).
    fn get(&self) -> *mut u8 {
        self.0
    }
}

// SAFETY: every jitted piece in one parallel step writes only its own
// disjoint slot range within a single level and reads only slots at
// strictly lower levels (validated in `Compiled::new`), which no piece of
// the same step writes. Distinct steps are separated by rayon joins.
unsafe impl Sync for SyncPtr {}

pub struct HybridEngine {
    tape: Arc<Compiled>,
    values: Vec<u64>,
    reg_scratch: Vec<u64>,
    steps: Vec<Step>,
    /// Owns the executable memory behind the plan. `Some` until drop.
    module: Option<JITModule>,
}

impl HybridEngine {
    pub fn new(circuit: &ryzr_core::Circuit) -> Self {
        Self::with_tape(Arc::new(Compiled::new(circuit)))
    }

    pub fn with_tape(tape: Arc<Compiled>) -> Self {
        Self::with_tape_and_threshold(tape, PARALLEL_THRESHOLD)
    }

    /// Like [`new`](Self::new), but with a custom width at which a level is
    /// split across threads. Mostly useful for testing the parallel path on
    /// small circuits.
    pub fn with_parallel_threshold(circuit: &ryzr_core::Circuit, threshold: usize) -> Self {
        Self::with_tape_and_threshold(Arc::new(Compiled::new(circuit)), threshold)
    }

    pub fn with_tape_and_threshold(tape: Arc<Compiled>, threshold: usize) -> Self {
        let n = tape.slot_count();
        assert!(
            n.checked_mul(8).is_some_and(|bytes| i32::try_from(bytes).is_ok()),
            "circuit too large for hybrid engine (slot offsets exceed i32)"
        );
        let threshold = threshold.max(1);

        // Plan: serial chunks over narrow levels, per-thread pieces over
        // wide ones.
        let chunks = |start: usize, end: usize| {
            let mut pieces = Vec::new();
            let mut s = start;
            while s < end {
                let e = usize::min(s + CHUNK, end);
                pieces.push(s..e);
                s = e;
            }
            pieces
        };

        let mut spec: Vec<Spec> = Vec::new();
        let mut serial_start = tape.gate_start as usize;
        for level in &tape.levels {
            let width = (level.end - level.start) as usize;
            if width < threshold {
                continue;
            }
            spec.extend(chunks(serial_start, level.start as usize).into_iter().map(Spec::Serial));
            spec.push(Spec::Parallel(chunks(level.start as usize, level.end as usize)));
            serial_start = level.end as usize;
        }
        spec.extend(chunks(serial_start, n).into_iter().map(Spec::Serial));

        let flat: Vec<core::ops::Range<usize>> = spec
            .iter()
            .flat_map(|s| match s {
                Spec::Serial(range) => core::slice::from_ref(range),
                Spec::Parallel(ranges) => ranges.as_slice(),
            })
            .cloned()
            .collect();
        let (module, fns) = compile_ranges(&tape, &flat, true);

        let mut fns = fns.into_iter();
        let steps = spec
            .into_iter()
            .map(|s| match s {
                Spec::Serial(_) => Step::Serial(fns.next().unwrap()),
                Spec::Parallel(ranges) => {
                    Step::Parallel(ranges.iter().map(|_| fns.next().unwrap()).collect())
                }
            })
            .collect();

        let mut values = vec![0u64; n];
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

        Self { tape, values, reg_scratch, steps, module: Some(module) }
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

impl Engine for HybridEngine {
    fn name(&self) -> &'static str {
        "hybrid"
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
        let tape = &self.tape;
        for (r, &slot) in tape.reg_out_slots.iter().enumerate() {
            if slot != u32::MAX {
                self.values[slot as usize] = self.reg_scratch[r];
            }
        }

        let base = SyncPtr(self.values.as_mut_ptr().cast::<u8>());
        for step in &self.steps {
            match step {
                Step::Serial(f) => {
                    // SAFETY: the buffer holds `slot_count()` u64s and every
                    // 8-byte offset the jitted code touches was validated in
                    // `Compiled::new` (and bounded by the i32 assert above).
                    unsafe { f(base.get()) }
                }
                Step::Parallel(fns) => {
                    fns.par_iter().for_each(|f| {
                        // SAFETY: as above; concurrent pieces write disjoint
                        // ranges of one level and read only lower levels
                        // (see the `Sync` impl on `SyncPtr`).
                        unsafe { f(base.get()) }
                    });
                }
            }
        }

        for (r, &slot) in tape.reg_in_slots.iter().enumerate() {
            self.reg_scratch[r] = self.values[slot as usize];
        }
    }
}

impl Drop for HybridEngine {
    fn drop(&mut self) {
        self.steps.clear();
        if let Some(module) = self.module.take() {
            // SAFETY: all pointers into the module's executable memory were
            // cleared above; nothing can call into it after this point.
            unsafe { module.free_memory() };
        }
    }
}
