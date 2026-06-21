# Editor design: a typed, multi-layer board editor

This is the design spec for the editor — the half of the game a player spends
their time in — and the implementation brief for the `ryzr-vcb` Bevy crate. It
assumes the model and semantics from `DESIGN.md`: a `Board` is a 3D stack of
`Tile`s, every driver is a register (one tick of delay), and `ryzr-board`
already compiles and runs all of it headless.

The goal the issue sets is "the editor, in full, at production quality" — and to
ask whether it can be something more than VCB's editor. The answer this spec
commits to is: a **typed, tile-based, genuinely 3D editor**, where VCB's editor
is an untyped, pixel-based, flat one. That difference runs through every section
below.

## 1. The model VCB uses, and why we diverge

VCB's editor is a **paint program**. You choose an ink colour and paint pixels;
the meaning of a region is its colour, and connectivity is "same-colour pixels
that touch." It is enormously flexible, and that is also its cost:

* **Untyped.** A wire and a gate's input are both just coloured pixels; the
  editor doesn't know a "gate" exists as an object, only that some pixels are
  `READ` ink next to some `WRITE` ink. Selecting, moving, or rotating a gate as a
  unit is awkward because there is no unit.
* **Pixel-exact.** `CROSS` "can only be one pixel wide or it will not work."
  Routing is fiddly precisely because the substrate has no higher-level notion
  than the pixel.
* **Flat.** Everything is on one plane (`DESIGN.md` §1).

We diverge on all three, deliberately:

| axis | VCB | this editor |
|---|---|---|
| unit of editing | a pixel of ink | a **typed `Tile`** in a cell |
| what the editor knows | colour adjacency | the actual object: gate, kind, facing, layer |
| crossing | 1px `CROSS` / `TUNNEL` | `Cross` tile, `Via` tile, or another **layer** |
| dimensionality | one plane | a **stack of planes** |

Being typed is what makes the rest cheap: rotation is "change `facing`", a probe
is "show this cell's net value", and a via is a first-class object rather than a
painting convention. The whole editor is ultimately a structured way to call
`Board::set(cell, tile)` and, in run mode, `Sim::set_switch`.

## 2. Two modes: Edit and Run

Like VCB, the editor has two modes, and the boundary between them is exactly the
`ryzr-board` compile step.

```text
        ┌────────── Edit ──────────┐         ┌────────── Run ───────────┐
        │ paint Tiles into Board   │  ──▶    │ Sim::new(&board) compiles │
        │ no ticking, board mutable │  Space  │ ticks advance; board frozen│
        │ palette, layers, rotate   │  ◀──    │ switches live; probe nets  │
        └───────────────────────────┘  Space  └────────────────────────────┘
```

* **Edit mode.** The `Board` is the source of truth and is freely mutable. No
  simulation runs. The palette, layer controls, and painting are all active.
  Nets may be previewed statically (the editor can run net extraction without a
  full `Sim` to colour connected conductors), but nothing ticks.
* **Run mode.** Entering Run calls `Sim::new(&board)`. From then on the `Board`
  is frozen (edits are disabled or deferred) and the `Sim` is the source of
  truth for *state*. Switches become interactive (`toggle_switch`), the
  transport controls (§5) drive `tick`/`run`, and rendering samples `cell_on` /
  `led_on`. Leaving Run drops the `Sim` and unfreezes the board.

A compile error (in practice unreachable — the lowering can't emit a cycle, see
`DESIGN.md` §4) surfaces in the HUD as a message and keeps the editor in Edit
mode rather than crashing. Honest failure, not a panic.

## 3. The tool palette

Each tool maps to a `Tile` (or to erase). This is the entire vocabulary, and it
is intentionally small — the depth dimension does the work VCB needs many inks
for.

| tool | places | notes |
|---|---|---|
| **Wire** | `Tile::Wire` | conductor, all four sides; drag to draw a run |
| **Cross** | `Tile::Cross` | in-plane crossing; N–S isolated from E–W |
| **Via** | `Tile::Via` | conductor that also fuses up/down — the depth primitive |
| **Gate** | `Tile::Gate { kind, facing }` | kind chosen from a sub-palette; `facing` rotatable |
| **Switch** | `Tile::Switch` | player input; interactive in Run mode |
| **Source** | `Tile::Source { on }` | constant high/low; toggle which on place |
| **Clock** | `Tile::Clock` | free-running, two-tick period |
| **LED** | `Tile::Led` | read-only indicator |
| **Erase** | `Tile::Empty` | clears a cell |

The eight gate kinds (`And`, `Or`, `Xor`, `Nand`, `Nor`, `Xnor`, `Buf`, `Not`)
are a sub-selection under the Gate tool — number keys `1`–`8`, matching VCB's own
eight-gate set so the muscle memory transfers. The currently selected gate kind
and `facing` are shown as a ghost under the cursor before placement.

## 4. The multi-layer interaction model

This is the part with no VCB equivalent, and the part most of the editor's
design effort goes into: **how you see and edit a 3D structure on a 2D screen.**

The approach is *one active layer, the rest as depth cue*:

* **Active layer.** Exactly one layer is active at a time. Painting, erasing, and
  rotating affect only it. It renders at full contrast.
* **Layers below** render dimmed and slightly cool-tinted, so a deep stack reads
  as receding into the screen. Layers *above* the active one render fainter still
  and desaturated, as "ceiling" context you can see through.
* **Vias** draw a short vertical connector linking the planes they fuse, so a via
  stack is visibly a column, not four unrelated tiles that happen to share an
  `(x, y)`.
* **Alignment is the whole game.** Because a via only fuses with a via *directly*
  above or below (`net.rs`, `vertical_neighbours`), the editor must make vertical
  alignment obvious: when the Via tool is selected, the cells directly above and
  below the cursor on adjacent layers are highlighted, showing exactly where a
  fuse will or won't happen.

Switching layers is a first-class, frequent action, so it gets dedicated keys and
a visible layer indicator (`L2/4`) in the HUD. The mental model we want is a
stack of glass sheets: you work on one sheet, you can see through to the others,
and vias are the posts that pin a signal through the stack.

A deliberately-not-chosen alternative: an isometric/3D camera. It looks
impressive in a screenshot and is worse to edit in — occlusion fights you, and
pixel-accurate placement on a tilted grid is exactly the fiddliness we're trying
to escape. A flat top-down view with a strong depth cue keeps placement trivial
while still communicating the stack. We optimise for building, not for the
trailer.

## 5. Controls

A concrete keymap, so the implementation has a target. Mouse does placement;
keyboard does mode, tool, layer, and transport.

| input | action |
|---|---|
| **left-drag** | paint the current tool (a drag draws a straight run for wires) |
| **right-drag** | erase |
| **middle-drag** | pan the camera |
| **scroll** | zoom |
| `Space` | toggle Edit ⇄ Run |
| `R` | rotate the tool's `facing` clockwise (`Direction::rotate_cw`) |
| `1`–`8` | select gate kind (or pick tool, in a tool-row layout) |
| `[` / `]` | active layer down / up |
| `Tab` | cycle tool |
| **Run only** | |
| `.` (period) | single-step one tick |
| `Space`-hold / `P` | play / pause continuous ticking |
| `+` / `-` | sim speed (ticks per frame) up / down |
| **left-click a switch** | toggle it (`Sim::toggle_switch`) |
| **hover any cell** | probe: HUD shows its net id and live value |

Painting wires by dragging draws a straight orthogonal run between the drag's
start and current cell (an L-shaped or Bresenham path), so laying a bus is one
gesture, not one click per cell — directly addressing the routing tedium that is
VCB's main time sink.

## 6. HUD and the probe

The on-screen readout, procedurally drawn with `bevy_ui` / `bevy_text`:

* **Mode** (`EDIT` / `RUN`) and, in Run, play/pause state and speed.
* **Active layer** (`L1/4`) and **current tool** (with gate kind + facing).
* **Counts**: tiles, nets (`Sim::net_count`), ticks elapsed (`Sim::ticks`).
* **Probe**: the cell under the cursor — its tile, the net id its conductor
  belongs to, and in Run its live boolean value (`Sim::net_value` /
  `cell_on`). This is the debugging primitive: point at a wire, see its signal.

The probe is the honest counterpart to VCB's wire-highlighting: it doesn't infer
or animate a guess, it reads the exact value the engine computed for that net
this tick.

## 7. Procedural rendering

No sprite sheets. Every tile is a function of its type and its live state, drawn
with gizmos and signed-distance shapes (`DESIGN.md` §7 sketches the look; this is
the spec):

| element | how it's drawn |
|---|---|
| **wire** | rounded stroke along the cell's connections; emissive when its net is high, dim base when low |
| **cross** | two short strokes, one N–S one E–W, coloured independently by their two nets |
| **via** | a wire stroke plus a filled disc, with a vertical connector to the via above/below |
| **gate** | a procedural glyph per `GateKind` (the AND/OR/XOR silhouettes), oriented by `facing`, with a notch on the output side |
| **switch** | a square that fills when on; clickable hit-target in Run |
| **source** | a small constant marker, high/low distinguished by fill |
| **clock** | a pulsing ring (its two-tick period made visible) |
| **LED** | a disc, dark→bright by `led_on` |
| **layer depth** | active full-contrast; below dimmed/cool; above faint/desaturated |

Colour encodes signal the way VCB does — lit nets glow, dark nets recede — but
the geometry is generated, so the only baked asset is a font (and even the
default font ships with Bevy). This satisfies the "minimum assets, maximum
procedural beauty" requirement directly: the repository carries essentially no
art, and the visuals scale crisply at any zoom because they are vector shapes,
not bitmaps.

## 8. The `ryzr-vcb` crate: ECS structure

The crate follows the `clientela` convention the issue points at: a thin
`main.rs` and a `lib.rs` root plugin, a `prelude.rs` of grouped re-exports, and
one `pub fn plugin(app: &mut App)` per module that the root plugin wires
together. Concretely:

```text
  main.rs       App::new().add_plugins(ryzr_vcb::plugin).run()
  lib.rs        root plugin: adds DefaultPlugins (curated, x11) + each module's plugin
  prelude.rs    grouped `pub use` for the crate's own types

  core/         the Board resource, EditorState (mode, active layer, tool),
                the camera; the shared state every other module reads
  editor/       input → painting: mouse/keyboard systems that mutate the Board
                in Edit mode, layer + tool + rotate handling
  sim/          owns the optional Sim; compiles on entering Run, ticks on the
                transport schedule, applies switch toggles, drops on leaving Run
  render/       procedural drawing of tiles, the depth cue, lit/dark nets
  ui/           HUD, palette, probe readout (bevy_ui + bevy_text)
```

Resources, roughly:

* `Board` — the document (from `ryzr-board`).
* `EditorState { mode: Edit | Run, active_layer, tool, gate_kind, facing }`.
* `Option<Sim>` — present only in Run mode; `None` in Edit.
* transport state (playing, ticks-per-frame) for Run.

The data flow is the §2 mode boundary made literal: `editor/` writes `Board` in
Edit; entering Run, `sim/` does `Sim::new(&board)` and from then on owns state;
`render/` and `ui/` read whichever is authoritative (`Board` shape always, `Sim`
values in Run). Keeping `sim/` the sole owner of the `Sim` and `editor/` the sole
writer of the `Board` is what keeps the two modes from fighting over state.

This is also what keeps rebuilds fast (an explicit issue requirement, with mold
configured in the build setup, see the `.cargo` config that ships with the
crate): the expensive-to-compile logic lives in `ryzr-board` and changes rarely;
the Bevy view modules are small and are the only thing that recompiles during the
tight edit-run-iterate loop.

## 9. What "production quality" means here

The issue asks for the editor "in full, at production quality." Concretely, the
bar this spec sets:

* **No mode is half-built.** Edit can place every tool on every layer; Run can
  play, pause, step, change speed, and toggle switches. Both are wired, not
  stubbed.
* **Every tool round-trips.** Anything you can paint compiles and simulates,
  because the tools *are* the `Tile`s `ryzr-board` already lowers and tests.
* **The starter content is real.** The editor opens on a demo from the gallery
  (`demos.rs`) — an actual working circuit a player can immediately run, not a
  blank grid — so the first five seconds demonstrate the engine.
* **It degrades honestly.** A compile failure is a HUD message; an out-of-bounds
  edit is a no-op (`Board::set` already ignores those); nothing panics on bad
  input.

The gameplay-and-killer-feature rationale lives in `DESIGN.md`; this document is
the contract the `ryzr-vcb` implementation is checked against.
