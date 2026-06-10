//! The unified engine: SWAR × rayon × JIT, with the plan picked by
//! measurement.
//!
//! Each technique multiplies a different axis, and they compose because
//! they are orthogonal:
//!
//! - **SWAR** packs 64 independent circuit instances into every `u64`
//!   value slot, dividing the per-gate cost by 64 (same lane discipline as
//!   [`BatchEngine`](crate::BatchEngine)).
//! - **rayon** fans wide levels out across cores. Gates within a level are
//!   independent by construction, so a wide level splits into disjoint
//!   pieces that run concurrently; each writes only its own slot range and
//!   reads only strictly lower levels.
//! - **JIT** compiles the settle pass to straight-line native code with
//!   operand offsets baked in as immediates, eliminating interpreter
//!   dispatch and keeping hot intermediates in registers (same chunking as
//!   [`JitEngine`](crate::JitEngine), but on 64-bit words).
//!
//! JIT is a trade, not a free win: straight-line code spends instruction
//! bytes on every gate, so past a few thousand gates the settle stops
//! fitting in instruction cache and each tick streams the whole program
//! from memory. At that point the SWAR interpreter's tiny resident loop
//! wins — its "program" is the tape's index arrays, which flow through the
//! data prefetcher instead of the front end. Where the crossover sits
//! depends on the circuit and the host, so [`Strategy::Auto`] (the
//! default) settles it the honest way: build both plans, time each on the
//! live circuit for a fraction of a millisecond, keep the faster one.
//!
//! Either plan merges consecutive narrow levels into serial sections (a
//! per-level rayon barrier costs microseconds — ruinous at millions of
//! ticks per second) and fans out only levels wider than the threshold.
//! Per instance the results are bit-for-bit identical to every other
//! engine under either plan; there are no semantic shortcuts anywhere.

use std::sync::Arc;
use std::time::{Duration, Instant};

use cranelift_jit::JITModule;
use rayon::prelude::*;

use crate::Engine;
use crate::batch::{apply_edge, capture_next, eval_gate, eval_runs};
use crate::compile::Compiled;
use crate::jit::{CHUNK, TickFn, compile_ranges};

pub const LANES: usize = 64;

/// Minimum level width (in gates) before the level is split across threads.
const PARALLEL_THRESHOLD: usize = 1 << 15;

/// How the hybrid engine executes the combinational settle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Strategy {
    /// Build both plans, time each on the live circuit, keep the faster.
    Auto,
    /// Force the jitted plan.
    Jit,
    /// Force the SWAR-interpreted plan.
    Interpret,
}

/// One step of the baked jitted plan.
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

/// The compiled settle pass and the executable memory backing it.
struct JitPlan {
    steps: Vec<Step>,
    /// Owns the executable memory behind `steps`. `Some` until drop.
    module: Option<JITModule>,
}

impl Drop for JitPlan {
    fn drop(&mut self) {
        self.steps.clear();
        if let Some(module) = self.module.take() {
            // SAFETY: all pointers into the module's executable memory were
            // cleared above; nothing can call into it after this point.
            unsafe { module.free_memory() };
        }
    }
}

/// How one tick's settle runs. [`Plan::Interp`] carries no data: it walks
/// the tape directly — narrow levels through the run interpreter, wide
/// levels fanned out per gate.
enum Plan {
    Jit(Box<JitPlan>),
    Interp,
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
    plan: Plan,
    /// Minimum level width before fanning out to rayon.
    threshold: usize,
    /// Whether any level reaches the threshold; lets the interpreted plan
    /// skip the level walk entirely on narrow circuits.
    has_wide: bool,
}

impl HybridEngine {
    pub fn new(circuit: &ryzr_core::Circuit) -> Self {
        Self::with_tape(Arc::new(Compiled::new(circuit)))
    }

    pub fn with_tape(tape: Arc<Compiled>) -> Self {
        Self::with_config(tape, PARALLEL_THRESHOLD, Strategy::Auto)
    }

    /// Like [`new`](Self::new), but with a custom width at which a level is
    /// split across threads. Mostly useful for testing the parallel path on
    /// small circuits.
    pub fn with_parallel_threshold(circuit: &ryzr_core::Circuit, threshold: usize) -> Self {
        Self::with_config(Arc::new(Compiled::new(circuit)), threshold, Strategy::Auto)
    }

    pub fn with_config(tape: Arc<Compiled>, threshold: usize, strategy: Strategy) -> Self {
        let threshold = threshold.max(1);
        let n = tape.slot_count();
        let fits_jit = n.checked_mul(8).is_some_and(|bytes| i32::try_from(bytes).is_ok());
        let has_wide =
            tape.levels.iter().any(|level| (level.end - level.start) as usize >= threshold);

        let plan = match strategy {
            Strategy::Interpret => Plan::Interp,
            Strategy::Jit => {
                assert!(fits_jit, "circuit too large for the jitted plan (offsets exceed i32)");
                Plan::Jit(Box::new(build_jit_plan(&tape, threshold)))
            }
            // Oversized circuits can't be jitted, so there is nothing to race.
            Strategy::Auto if !fits_jit => Plan::Interp,
            Strategy::Auto => Plan::Jit(Box::new(build_jit_plan(&tape, threshold))),
        };
        let race = matches!(strategy, Strategy::Auto) && matches!(plan, Plan::Jit(_));

        let mut engine = Self {
            values: vec![0; n],
            reg_scratch: vec![0; tape.register_count()],
            tape,
            plan,
            threshold,
            has_wide,
        };
        engine.reset();
        if race {
            engine.calibrate();
        }
        engine
    }

    /// Restore power-on state: constants, register initials, inputs low.
    fn reset(&mut self) {
        self.values.fill(0);
        for &(slot, value) in &self.tape.const_slots {
            self.values[slot as usize] = if value != 0 { u64::MAX } else { 0 };
        }
        for (r, &init) in self.tape.reg_initial.iter().enumerate() {
            let broadcast = if init != 0 { u64::MAX } else { 0 };
            self.reg_scratch[r] = broadcast;
            let slot = self.tape.reg_out_slots[r];
            if slot != u32::MAX {
                self.values[slot as usize] = broadcast;
            }
        }
    }

    /// Race the two plans on the live circuit and keep the winner. Called
    /// only from [`Strategy::Auto`] construction with the jitted plan built.
    fn calibrate(&mut self) {
        let jit = core::mem::replace(&mut self.plan, Plan::Interp);
        let interpreted = self.time_per_tick();
        self.plan = jit;
        let jitted = self.time_per_tick();
        if interpreted < jitted {
            // Dropping the jitted plan frees its executable memory.
            self.plan = Plan::Interp;
        }
        self.reset();
    }

    /// Average tick latency over a short timed burst: a couple of warmup
    /// ticks, then ~500µs (capped at 4096 ticks) of measurement.
    fn time_per_tick(&mut self) -> Duration {
        self.tick();
        self.tick();
        let start = Instant::now();
        let mut ticks = 0u32;
        loop {
            for _ in 0..4 {
                self.tick();
            }
            ticks += 4;
            let elapsed = start.elapsed();
            if elapsed >= Duration::from_micros(500) || ticks >= 4096 {
                return elapsed / ticks;
            }
        }
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

/// Compile the settle pass: serial chunks over narrow levels, per-thread
/// pieces over levels at least `threshold` gates wide.
fn build_jit_plan(tape: &Compiled, threshold: usize) -> JitPlan {
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

    let n = tape.slot_count();
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
    let (module, fns) = compile_ranges(tape, &flat, true);

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

    JitPlan { steps, module: Some(module) }
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
        let Self { tape, values, reg_scratch, plan, threshold, has_wide } = self;
        apply_edge(tape, values, reg_scratch);

        match plan {
            Plan::Jit(jit) => {
                let base = SyncPtr(values.as_mut_ptr().cast::<u8>());
                for step in &jit.steps {
                    match step {
                        Step::Serial(f) => {
                            // SAFETY: the buffer holds `slot_count()` u64s
                            // and every 8-byte offset the jitted code touches
                            // was validated in `Compiled::new` (and bounded
                            // by the i32 check at construction).
                            unsafe { f(base.get()) }
                        }
                        Step::Parallel(fns) => {
                            fns.par_iter().for_each(|f| {
                                // SAFETY: as above; concurrent pieces write
                                // disjoint ranges of one level and read only
                                // lower levels (see the `Sync` impl on
                                // `SyncPtr`).
                                unsafe { f(base.get()) }
                            });
                        }
                    }
                }
            }
            Plan::Interp if !*has_wide => {
                eval_runs(tape, values, 0..tape.runs.len());
            }
            Plan::Interp => {
                for level in &tape.levels {
                    let width = (level.end - level.start) as usize;
                    if width < *threshold {
                        let runs = level.run_start as usize..level.run_end as usize;
                        eval_runs(tape, values, runs);
                        continue;
                    }

                    let (lower, rest) = values.split_at_mut(level.start as usize);
                    let (current, _) = rest.split_at_mut(width);
                    let lower = &*lower;
                    let base = level.start as usize;

                    current.par_iter_mut().with_min_len(1024).enumerate().for_each(|(k, out)| {
                        *out = eval_gate(tape, lower, base + k);
                    });
                }
            }
        }

        capture_next(tape, values, reg_scratch);
    }
}
