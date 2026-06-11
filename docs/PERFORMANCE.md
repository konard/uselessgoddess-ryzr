# Performance analysis: how close can an honest gate simulator get to Verilog?

This document is the step-by-step profiling and feasibility study behind the
single-instance engines. The question it answers: the gate-level RV32I core
ran at **~1.34 M instructions/s** on the packed JIT (one simulated CPU on a
6-core desktop); Verilator-class RTL simulators reach **~5 M instructions/s**
on a comparable single-cycle core. Is that gap closable while keeping the
honesty contract — *every gate computed every tick* — and if so, how?

It also records a result the profiling produced and the first guess got wrong,
because the wrong guess is the useful part: the obvious bottleneck was not the
real one. The current throughput, after the fusion this branch lands, is
**~1.49 M instructions/s** (best of 5).

All numbers below come from the `plan_report` and `where_splats` examples in
`ryzr-riscv` (run them yourself: `cargo run -p ryzr-riscv --release --example
plan_report`).

## 1. Where the time goes (baseline)

The packed JIT lowers the circuit to a straight-line *word program*: each task
evaluates up to 64 same-op gates as one 64-bit word op, after gathering its
operands from scattered arena bits. On the RV32I core (256-word RAM), one tick
*before* register-file fusion was:

| metric | count |
|---|---|
| word tasks | 220 |
| — of which muxes | 110 (50%) |
| settle gather segments | 328 funnels + **1219 splats** |
| capture gather segments | 167 funnels + 330 splats |
| fused carry chains | 4 |
| fused RAM banks | 1 (256×32) |

A **funnel** is a contiguous run of source bits moved with one shift (~6 ops
for up to 64 bits). A **splat** broadcasts one scattered source bit to a
destination mask (~5 ops). Splats are the expensive primitive, and the largest
share of them are emitted by muxes:

| op | tasks | splats |
|---|---|---|
| **mux** | **110** | **874** |
| and | 66 | 213 |
| or | 13 | 82 |
| xor | 9 | 21 |
| not | 17 | 20 |
| add (fused) | 4 | 9 |
| memread (fused) | 1 | 0 |

Muxes produce 874 of the 1219 settle splats. The obvious reading — *fuse the
biggest mux structure and the splats fall with it* — is what the next section
tests, and it is wrong.

## 2. The biggest mux structure, and what fusing it actually did

Counting the unfused muxes by schedule level pointed straight at one structure:

```
unfused muxes (gate level): 3176
  level   4: 960   level   5: 512   level   6: 256   level   7: 128   level 8: 76
  level  10..17: ~460   (ALU/branch/immediate selection)
  level  33..37: ~250   (writeback / next-state selection)
  level  41..81: 16 each (barrel-shifter stages, ~320 total)
```

Levels 4–8 are a 32→16→8→4→2 reduction (≈1932 muxes) — the **register file's
two read ports** (`rs1`, `rs2`), each a 32-way mux tree selecting `regs[rs]`.
It is the RAM's twin, and it was *not* fused, for three structural reasons the
detector had to be taught to see past:

1. **31 stored words, not 32.** `x0` is hardwired to zero (no storage), so the
   write-cell array is X1–X31 — not a power of two, which the bank detector
   requires.
2. **A constant leaf.** The read tree's leaf 0 is a constant-zero word, so the
   bottom-up reconstruction in `find_banks` does not match.
3. **Two read ports share the leaves.** The bottom-level muxes of `rs1` and
   `rs2` have identical `(then, else)` operands (`(X1, zero)`), so they collide
   in the reverse-mux map and only one port reconstructs.

This branch fuses both read ports (`find_regfiles` in `mem.rs`): each becomes a
single dynamic-index gather, the `x0`-as-zero handled by reading index
`addr − (addr ≠ 0)` and masking the result for `addr == 0`. The logical word
count is recovered from the read *tree* (the largest power-of-two that
reconstructs fully), because the write side fragments under mux strength
reduction and under-reports it. Both ports are now fused (`992` muxes each).

The result, measured:

| metric | before | after |
|---|---|---|
| unfused gate-muxes | 3176 | **1256** |
| mux word tasks | 110 | **80** |
| settle **funnels** | 328 | **206** |
| settle **splats** | 1219 | **1184** |
| mux splats | 874 | **843** |
| throughput (best of 5) | ~1.34 MIPS | **~1.49 MIPS** |

Read the splat columns. Removing ~1932 gate-muxes — the single biggest
structure in the netlist — removed **35 settle splats** (1219→1184), and only
**31 of the mux splats** (874→843). The ~10% throughput gain is real, but it
came from elsewhere: **122 fewer funnels**, 30 fewer word tasks, and ~1932
fewer gate evaluations folded into two indexed loads.

**Why the splats barely moved.** The register file stores its 31 words as
contiguous 32-bit regions in the arena. A mux tree over *contiguous words*
gathers its operands as **funnels**, not splats — the very primitive that is
cheap. The read-port trees were never the splat bottleneck; they were a funnel
and gate-count cost, and that is exactly the part that dropped. The first guess
conflated "most muxes" with "most splats." They are not the same gates.

## 3. Where the splats actually are

The 843 remaining mux splats come from muxes whose `then`/`else` operands are
**individual, scattered control signals**, not aligned word regions — so each
operand bit must be splatted into place. Those are the *uniform-select* control
muxes, and `plan_report` locates them by level:

```
level 10..17  ~460 muxes   ALU-result / branch / immediate selection
level 33..39  ~300 muxes   writeback and next-state (PC, CSR) selection
level 41..81  ~340 muxes   barrel shifter (SLL/SRL/SRA), 16 per stage × ~20
```

These are the honest residue: a `mux(sel, a, b)` where `a` and `b` are two
unrelated nets computed far apart in the arena. No placement makes both
contiguous with the destination at once, so the gather splats. This — not the
register file — is what an honest simulator must attack to approach RTL speed.

## 4. Is 5 MIPS reachable, and what does it take?

**The gap is structural, not constant-factor.** Verilator does not simulate
gates: it lowers the *RTL* — `regs[rs1]` is an array index, `a + b` is a machine
add — and lets a C++ compiler optimise the datapath. `ryzr` is contractually
honest: it computes all 22,679 gates every tick. The only way an honest
simulator approaches RTL speed is to *recognise the structures the gates spell
out and execute them as their RTL equivalent*, bit-for-bit. Carry-chain fusion
(ripple adder → native add), RAM fusion (mux-tree → indexed gather) and now
register-file read fusion already do this. The path forward is **more fusion of
the splat-heavy structures specifically**, since (§2) gate count and splat count
are different costs and the splats are what dominate the gather.

In descending order of expected payoff, re-ranked by the §2/§3 measurement:

1. **Barrel-shifter recognition** (levels 41–81, ~320 muxes, ~16 splats/stage).
   These are `mux(sh[k], x << 2^k, x)` — a *uniform-select* shift-or-keep at
   each stage, the splattiest structure left. Recognised, the whole shifter
   becomes a handful of native shifts + selects instead of 20 levels of per-bit
   muxes. Highest splat-per-gate payoff of anything remaining.
2. **ALU-result / writeback select fusion** (levels 10–17, 33–39, ~760 muxes).
   The one-hot funnel that picks the retired result and next PC is a wide
   uniform-select mux over scattered datapath nets — the bulk of the surviving
   843 mux splats. Lowering it to a select over already-computed words removes
   them at the source.
3. **ROM / instruction-fetch fusion.** The fetch is `rom[pc]` — a mux-tree over
   constants. Fusing it to an indexed load over a constant table removes the
   fetch tree (a gate-count win like the register file, modest on splats).
4. **Operand placement to convert splats → funnels.** Where a mux's operands
   *can* be made contiguous (e.g. same-width sibling results), a placement pass
   that numbers slots for contiguity trades the 5-op splat for the amortised
   funnel — a constant-factor win on whatever 1–2 do not fuse.
5. **Word-level SIMD (AVX-512).** The arena is bit-packed `u64`s; the gather and
   boolean ops are embarrassingly vectorisable across the ~336-word arena.

**Verdict.** 5 MIPS on this core *while staying honest* is plausible but not a
single change — and not the change the level histogram first suggested. The
register file was the biggest *structure*; the barrel shifter and the writeback
select are the biggest *splat sources*. Fusing those two (steps 1–2) targets the
843 mux splats directly and should move throughput far more than another
gate-count win would; closing the final gap to 5 MIPS likely needs step 5 as
well. None of it abandons the contract: every fused structure computes the exact
boolean function its gates declare, and the differential + lockstep suites prove
it on every tick.

## 5. What this branch changes

See the commit history: profiling tooling (`plan_report`), then register-file
read-port fusion (`find_regfiles`). Each fusion step is gated behind the same
exhaustive structural verification as RAM fusion and checked by the differential
and RISC-V lockstep suites (full architectural state compared against an
instruction-level emulator after every retired instruction). The §2 result — a
landed, measured fusion whose splat reduction was an order of magnitude smaller
than predicted — is the reason §3/§4 re-rank the remaining work around splat
sources rather than gate counts.
