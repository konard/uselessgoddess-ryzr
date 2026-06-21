# Design: a blazingly fast multi-layer circuit editor

This is the design study for the game that sits on top of the `ryzr` engine —
a Virtual Circuit Board-like sandbox where you paint digital logic and watch it
run, but with one dimension VCB does not have: **literal depth**. The board is a
stack of logic planes wired together by vias, the way a real PCB or chip is, and
every gate of it is simulated honestly by the engine the rest of this repository
is about.

The same rule the engine obeys applies here: **simulation is honest**. The
editor never fakes a result. Every tile a player paints becomes a real gate in a
real `ryzr_core::Circuit`, and the value shown on a wire is the value the engine
computed for it that tick. The design below is built around keeping that true
while still feeling like a game.

Two questions drive the whole document: *where does this beat VCB*, and *what is
the one feature worth building the game around*. The short answers — true
multi-layer routing, and treating the extra layers as a silicon-style place-and-
route target instead of shipping yet another bespoke assembler — are argued in
§3 and §5. What is already built and tested is marked as such (§6); what is
designed but not yet implemented is marked too. Nothing here is aspirational
hand-waving dressed as a feature.

## 1. What VCB actually is (so we know what to beat)

Virtual Circuit Board is a sandbox where you paint coloured "inks" onto a 2D
canvas and a high-performance engine ticks the result. The pieces, from its own
documentation:

| ink / tool | role |
|---|---|
| `TRACE` | the wires; carries a signal, merges with touching trace |
| `READ` | a gate's input side |
| `WRITE` | a gate's output side |
| `CROSS` | lets two traces cross without connecting — **one pixel wide only** |
| `TUNNEL` | jumps a signal under other ink to a matching tunnel elsewhere |
| `MESH` / `BUS` | space-optimisation; a bus bundles up to 16 like-typed lines into one |
| 8 gates | `BUFFER`, `AND`, `OR`, `XOR` and their inverted twins `NOT`, `NAND`, `NOR`, `XNOR` |

Two facts about VCB matter for this design:

* **It is a single logic plane.** VCB has "decoration layers", but those are
  *purely visual* — they override a component's colour so you can draw buttons
  and labels; they carry no signal. All logic lives on one flat canvas, and the
  only ways to get one wire past another are `CROSS` (a one-pixel crossing) and
  `TUNNEL` (an under-jump). Both are 2D dodges around a 2D constraint.
* **It already has an assembler.** VCB ships an assembly editor for programming
  the soft CPUs you build, and the open-source `openVCB` reimplementation is
  "100% compatible" with VCB assembly. An assembler is therefore *parity* with
  VCB, not a way to beat it — a fact §5 turns on.

Its tick model is the part `ryzr` already matches: a signal propagates roughly
one component per tick, and the engine runs that at up to millions of ticks per
second. The README's RISC-V section covers the speed comparison in detail; this
document is about the game, not the benchmark.

## 2. Where this beats VCB

| dimension | VCB | this editor |
|---|---|---|
| **depth** | one logic plane + visual-only decoration layers | **N stacked logic planes**, connected by vias |
| **crossing** | `CROSS` (1px) and `TUNNEL` on the single plane | cross *and* via — or just route on another layer |
| **engine** | event-based sim | the full `ryzr` engine set, packed-JIT one-instance, every engine bit-identical to the reference oracle |
| **semantics** | implicit unit delay | explicit: every driver is a register, so feedback is always legal and the delay is exactly defined (§4) |
| **graphics** | painted sprite inks | procedural — gizmos and signed-distance shapes, minimal baked assets (§7, and `EDITOR.md`) |
| **programming a CPU** | bespoke ISA + bespoke assembler | a real gate-level RV32I core as a blueprint, targetable by *any* RISC-V toolchain (§5) |

The depth row is the headline and the next section is entirely about it. The
engine row is the substrate the rest of the repo already delivers. The rest are
consequences.

## 3. The killer feature: literal depth

**The real constraint in a 2D circuit painter is routing, not logic.** Anyone
who has built something large in VCB knows the gates are the easy part; the work
is getting signals *past each other*. A bus that has to cross a control matrix is
dozens of individual `CROSS` pixels; a signal that has to reach the far side of a
dense block either snakes around it or tunnels under it. This is exactly why real
hardware stopped being single-layer: PCBs went to 4, 8, 16 copper layers and
chips to a dozen-plus metal layers, *purely to have somewhere to route*. Logic is
cheap; wires fighting for space is the cost.

So the feature worth building the game around is the one VCB's architecture can't
retrofit cleanly: **stacked logic planes joined by vias.** Not decoration layers
— real ones, where a signal can climb to an empty plane above, cross the
congestion freely, and drop back down.

The model is already implemented and tested in `ryzr-board`:

```text
  layer 1   . . . . [via] ──wire── [led]      ← the signal arrives on a clean plane
                       │
                     (fuse)                     ← Via fuses with the Via directly below
                       │
  layer 0  [switch] ─wire─ [via] . . . . .     ← and crosses the congested plane underneath
```

* A `Cell` is `{ layer, x, y }` — the grid is genuinely 3D.
* A `Tile::Via` conducts in-plane like a wire **and** fuses with a `Via`
  directly above or below it (`net.rs`, `vertical_neighbours`). That vertical
  fuse is the via; stack three vias and a net spans three layers.
* The `via_bridge` demo and its `via_bridges_layers` test prove the end to end
  case: a switch on layer 0 lights an LED on layer 1, *only* through the via
  stack, and goes dark when the switch opens. It is a reproducing test, not a
  promise.

Why this is more than a gimmick, concretely:

* **Routing becomes a 3D problem with a 2D cost.** Every layer you add is a fresh
  uncongested plane. The classic VCB pain — a 16-bit bus that must cross a
  decoder — becomes: via the bus up one layer, run all sixteen lines straight
  across empty space, via it back down. No per-line crossing, no tunnel
  bookkeeping.
* **It composes with the engine for free.** The compiler treats a via stack as
  one net; the via adds no gate and no delay. So depth costs nothing at
  simulation time — a 100×100×4 board is the same kind of circuit as a
  200×200×1 board, only easier to wire. The honesty contract is untouched: a via
  is a conductor, it computes nothing, it just connects.
* **It is the natural home for §5's synthesis.** When a logic description is
  compiled and auto-placed, the spare layers are where the router puts the wires
  — exactly as a silicon place-and-route tool uses metal layers. The killer
  feature and the "more insane than an assembler" feature are the same
  feature seen twice.

## 4. The honest semantics: every driver is a register

This is the load-bearing design decision, and it is what lets a VCB-style game
run on `ryzr` at all.

`ryzr` forbids combinational cycles — a circuit must be a DAG, because the engines
levelize it and evaluate each level once. But every interesting VCB build *is* a
cycle: an SR latch is two NOR gates feeding each other, a ring oscillator is an
inverter feeding itself. If a gate were lowered as pure combinational logic,
those would be illegal circuits.

The reconciliation: **every driver lowers to a register.** A gate computes
`next <= op(inputs)` and exposes the register's *previous* value as its output.
Concretely (see `compile.rs`):

* Gates and clocks each get a `ryzr_core` register (Pass A); switches and sources
  resolve to an input or constant immediately.
* Each net is the wired-OR of its drivers (Pass B).
* Every register's next-state is driven from its input nets (Pass C). Because the
  output is the *old* register value, any feedback path passes through a register
  — so the constructed circuit is always a DAG, and no combinational cycle is
  ever emitted.
* Every net and every LED is exported as a circuit output so the simulator can
  read the board back (Pass D).

The payoff is that the unit of delay is now *exactly* defined and matches VCB:
**a signal advances exactly one driver per tick.** A chain of three gates takes
three ticks to settle; a latch holds because its registers hold; an oscillator
flips because its inverter's register flips. The tests pin all three:
`ring_oscillator_has_period_two`, `sr_latch_sets_resets_and_holds`,
`clock_blinks`.

One subtlety the renderer must respect: a register shows its settled,
*pre-edge* value, so a freshly driven gate's output appears on the next tick, not
the same one. The simulator's `run(n)` helper exists so tests and the UI can let
a board settle before reading it. This is the same one-tick latency every
register-transfer system has, and it is the honest behaviour, not a bug to paper
over.

## 5. The assembler decision: don't write one

The issue asks directly whether to build an assembler or "something more insane."
The research answer makes the choice for us.

**VCB already has an assembler.** Building one is parity, not a differentiator —
the most polished outcome is to be as good as the thing we are trying to beat.
Worse, a bespoke assembler is bound to a bespoke ISA, which is a large amount of
design work whose ceiling is "VCB, again."

The stack already contains something stronger: **`ryzr-riscv`, a real gate-level
RV32I core** — ripple-carry ALU, barrel shifters, register file and RAM as flip-
flops behind mux trees, lockstep-verified against an instruction-level emulator,
running at ~1.49 M instructions/s on the packed JIT. It is not a toy ISA. It is
the actual RISC-V base integer instruction set, built from the same honest gates
a player would paint.

So the decision is:

> **Defer the bespoke assembler. Ship the gate-level RV32I core as an importable
> blueprint, and invest the "insane" budget in synthesis — compiling a logic
> description and auto-placing it across the multi-layer board, the way a silicon
> tool place-and-routes a netlist onto metal layers.**

This is better than an assembler on both axes the repo cares about:

* **More honest.** The CPU you drop on the board is real gates, ticking under the
  same engine as everything else — not an interpreter bolted onto the side. You
  can probe its internal register file with an LED.
* **More insane, and more useful.** Because it is genuine RV32I, *any* RISC-V
  toolchain already targets it — `gas`, LLVM, Rust. "Program the CPU you built"
  comes for free and is strictly more powerful than a bespoke assembler, without
  writing an assembler at all. And the synthesis path means a player can describe
  logic above the gate level and watch it become a placed, routed, *running*
  circuit — using the spare layers (§3) as routing planes. The killer feature
  feeds the insane feature directly.

Staging, honest about what exists:

| stage | what | status |
|---|---|---|
| 0 | hand-built demo boards (`demos.rs`) as starter content and fixtures | **implemented, tested** |
| 1 | the RV32I core importable as a board blueprint | core exists in `ryzr-riscv`; import path designed, not built |
| 2 | expression / truth-table → gates, greedy multi-layer routing | designed (§3 + this section) |
| 3 | external netlist / RISC-V image import, full place-and-route | planned |

The assembler is not abandoned forever — once a soft-CPU blueprint is placeable
(stage 1), an assembler is a small convenience on top, and for RV32I we get it
from existing tools. It is simply not the thing to lead with.

## 6. Architecture: where the logic lives

The game is split so that all the logic is headless, fast to build, and
exhaustively testable, and only the thin top layer needs Bevy, a window, or a
GPU.

```text
  ryzr-core ──► ryzr-backend ──► ryzr-riscv      the engine substrate (unchanged)
      │              │
      └──────┬───────┘
             ▼
        ryzr-board        pure logic: Board/Cell/Tile, net extraction,
        (this PR)         compile→Circuit, Sim façade, demo gallery.
             │            No Bevy. Builds in seconds. Headless tests.
             ▼
        ryzr-vcb          thin Bevy 0.19 front-end: plugin-per-module,
        (next PR)         procedural rendering, input/painting/layers,
                          play-pause-step. Drives ryzr-board.
```

* **`ryzr-board`** is implemented in this PR. It owns the `Board` model, net
  extraction (`net.rs`), the register-based lowering (`compile.rs`, §4), the
  `Sim` façade that runs a board on the interpreter by default or any
  `ryzr-backend` engine behind the `engines` feature, and a `demos` gallery that
  doubles as starter content and the test corpus. It pulls in no rendering
  dependency at all and its behavioural test suite runs in seconds.
* **`ryzr-vcb`** is the Bevy front-end, specified in `EDITOR.md`. It follows the
  one-`plugin`-function-per-module convention, renders procedurally, and is a
  thin shell over the types `ryzr-board` re-exports. Keeping it thin is what
  keeps rebuilds fast (the heavy logic recompiles rarely; only the view churns).

This split is the reason the design can claim honesty with a straight face: the
part that has to be *correct* is the part that has no UI, and it is tested in
isolation against the same reference interpreter the engines are validated
against.

## 7. Graphics: procedural, minimal assets

The issue asks for "minimum assets, maximum procedural beauty," and the
architecture above makes that cheap. Every tile has a known shape and a known
electrical state each tick, so the renderer is a pure function of board + sim
state, drawn with Bevy gizmos and signed-distance shapes rather than sprite
sheets:

* **Wires and vias** as rounded strokes; lit nets glow by driving emissive colour
  from `Sim::cell_on`, dark nets sit at a low base — the same trick VCB uses to
  show signal flow, done procedurally.
* **Gates** as procedural glyphs (the classic AND/OR/XOR silhouettes) generated
  from the `GateKind`, oriented by `facing`.
* **Layers** distinguished by depth cue: the active layer at full contrast, the
  ones below dimmed and tinted, so the stack reads as a stack. Vias draw a short
  vertical connector between planes.

The full editor interaction, tool set, layer UX, and control scheme are the
subject of `EDITOR.md`. The renderer details (shapes, palettes, the depth cue)
land with the `ryzr-vcb` crate.

## 8. Status summary

| component | state |
|---|---|
| multi-layer board model (`Board`/`Cell`/`Tile`, vias) | implemented, tested |
| net extraction (union-find, cross-axis isolation, vertical via fuse) | implemented, tested |
| register-based unit-delay lowering to `ryzr_core::Circuit` | implemented, tested |
| `Sim` façade (interpreter default, optional `ryzr-backend` engines) | implemented, tested |
| demo gallery (AND, oscillator, clock, SR latch, via bridge) | implemented, tested |
| Bevy front-end `ryzr-vcb` (editor, rendering, controls) | designed (`EDITOR.md`), not yet built |
| RV32I core as importable blueprint | core exists; import designed |
| synthesis + multi-layer place-and-route | designed (§5), planned |

Sources for the VCB facts above: the official
[Steam basics guide](https://steamcommunity.com/sharedfiles/filedetails/?id=2814335261),
the [Virtual Circuit Board store page](https://store.steampowered.com/app/1885690/Virtual_Circuit_Board/),
and the [openVCB](https://github.com/kittybupu/openVCB) open-source engine and
assembler.
