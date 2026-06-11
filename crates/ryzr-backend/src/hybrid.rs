//! The unified engine: every single-instance technique in the crate behind
//! one type, with the plan picked by measurement instead of guesswork.
//!
//! [`HybridEngine::new`] simulates **one** circuit instance — the mode that
//! answers "how fast can a single simulated CPU run?". No formula predicts
//! which strategy wins that race: it depends on circuit shape, switching
//! activity, cache sizes, and core count. So the constructor settles it the
//! honest way — it builds every single-instance candidate, times each on
//! the live circuit for a fraction of a millisecond, keeps the winner, and
//! drops the rest:
//!
//! - **packed JIT** ([`PackedJitEngine`]) compiles the word-level packed
//!   plan to native code; wins on dense, always-active logic like CPU
//!   datapaths — by a wide margin.
//! - **packed SWAR** ([`PackedEngine`]) interprets the same plan; the
//!   cheap-to-construct fallback when compile time matters.
//! - **event-driven** ([`EventEngine`]) pays per *changed* gate, not per
//!   gate; wins on mostly-idle circuits.
//! - **level-parallel** ([`ThreadedEngine`]) fans wide levels across cores
//!   via rayon; wins when individual levels are tens of thousands of gates
//!   wide.
//!
//! Calibration runs the circuit from power-on state with inputs held low,
//! so activity-dependent plans are measured under that workload; whatever
//! wins, the results stay bit-for-bit identical to every other engine.

use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::compile::Compiled;
use crate::{Engine, EventEngine, PackedEngine, PackedJitEngine, ThreadedEngine};

/// Minimum level width (in gates) before the level is split across threads.
const PARALLEL_THRESHOLD: usize = 1 << 15;

/// Average tick latency over a short timed burst: a couple of warmup
/// ticks, then ~500µs (capped at 4096 ticks) of measurement.
fn time_per_tick(mut tick: impl FnMut()) -> Duration {
    tick();
    tick();
    let start = Instant::now();
    let mut ticks = 0u32;
    loop {
        for _ in 0..4 {
            tick();
        }
        ticks += 4;
        let elapsed = start.elapsed();
        if elapsed >= Duration::from_micros(500) || ticks >= 4096 {
            return elapsed / ticks;
        }
    }
}

/// The single-instance candidates. Every variant produces bit-for-bit
/// identical results; only speed differs, so the constructor races them on
/// the live circuit and keeps exactly one.
enum Single {
    Packed(PackedEngine),
    Event(EventEngine),
    Threaded(ThreadedEngine),
    PackedJit(Box<PackedJitEngine>),
}

impl Single {
    fn engine(&self) -> &dyn Engine {
        match self {
            Self::Packed(e) => e,
            Self::Event(e) => e,
            Self::Threaded(e) => e,
            Self::PackedJit(e) => e.as_ref(),
        }
    }

    fn engine_mut(&mut self) -> &mut dyn Engine {
        match self {
            Self::Packed(e) => e,
            Self::Event(e) => e,
            Self::Threaded(e) => e,
            Self::PackedJit(e) => e.as_mut(),
        }
    }

    fn reset(&mut self) {
        match self {
            Self::Packed(e) => e.reset(),
            Self::Event(e) => e.reset(),
            Self::Threaded(e) => e.reset(),
            Self::PackedJit(e) => e.reset(),
        }
    }
}

/// Race every single-instance candidate on the live circuit and return the
/// winner, restored to power-on state (racing advances the simulation, so
/// the winner is reset before use).
fn race_single(tape: Arc<Compiled>, threshold: usize) -> Single {
    // The packed JIT has no size cliff: its offsets are word-granular, so
    // any circuit whose plan fits the u32 bit arena compiles fine.
    let candidates = vec![
        Single::Packed(PackedEngine::with_tape(&tape)),
        Single::PackedJit(Box::new(PackedJitEngine::with_tape(&tape))),
        Single::Event(EventEngine::with_tape(tape.clone())),
        Single::Threaded(ThreadedEngine::with_tape(tape).with_threshold(threshold)),
    ];

    let mut best: Option<(Duration, Single)> = None;
    for mut candidate in candidates {
        let t = time_per_tick(|| candidate.engine_mut().tick());
        if best.as_ref().is_none_or(|&(b, _)| t < b) {
            best = Some((t, candidate));
        }
    }
    let (_, mut winner) = best.expect("candidate list is never empty");
    winner.reset();
    winner
}

pub struct HybridEngine {
    single: Single,
}

impl HybridEngine {
    /// One simulated instance, run by the fastest single-instance plan —
    /// picked by racing every candidate on the live circuit.
    pub fn new(circuit: &ryzr_core::Circuit) -> Self {
        Self::with_tape(Arc::new(Compiled::new(circuit)))
    }

    /// Single-instance mode from an already-compiled tape.
    pub fn with_tape(tape: Arc<Compiled>) -> Self {
        Self { single: race_single(tape, PARALLEL_THRESHOLD) }
    }

    /// Like [`new`](Self::new), but with a custom width at which the
    /// level-parallel candidate splits a level across threads. Mostly
    /// useful for exercising the parallel path on small circuits.
    pub fn with_parallel_threshold(circuit: &ryzr_core::Circuit, threshold: usize) -> Self {
        let tape = Arc::new(Compiled::new(circuit));
        Self { single: race_single(tape, threshold.max(1)) }
    }
}

impl Engine for HybridEngine {
    fn name(&self) -> &'static str {
        "hybrid"
    }

    fn input_count(&self) -> usize {
        self.single.engine().input_count()
    }

    fn output_count(&self) -> usize {
        self.single.engine().output_count()
    }

    fn set_input(&mut self, index: usize, value: bool) {
        self.single.engine_mut().set_input(index, value);
    }

    fn output(&self, index: usize) -> bool {
        self.single.engine().output(index)
    }

    fn tick(&mut self) {
        self.single.engine_mut().tick();
    }
}
