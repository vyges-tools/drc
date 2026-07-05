# vyges-drc

Geometric **design-rule check**: a laid-out **GDS** + a per-layer **rule deck** in,
a list of **violations** out.

> **Vyges open EDA tools.** Commercial-grade silicon sign-off capability, built on
> open standards and plain file formats — and meant to be accessible to everyone,
> not only teams who can license a six-figure tool. `vyges-drc` opens up DRC.

> **Stability: experimental (v0.1.2).** The width and spacing checks are real and
> tested, but this is an early engine — see **Current state** for exactly what is
> and isn't covered. Treat it as an inner-loop checker, not tape-out sign-off.

## Why this exists

A layout is only manufacturable if its geometry obeys the foundry's rules —
minimum wire width, minimum spacing, enclosure, density. DRC proves that. It is
the geometric sibling of LVS: **LVS** asks *does the layout implement the
schematic?*, **DRC** asks *does the layout obey the rules?* Both are required to
tape out, and both are pure geometry — exactly the deterministic, graph/geometry
work an open Rust engine can own.

## How this is solved today

In production, DRC is a **commercial physical-verification tool** (Calibre,
Pegasus, …) — gated behind major licenses. The open options are **Magic** and
**KLayout** — capable, but C++/Tcl/Ruby and awkward to embed. `vyges-drc` is a
clean-room **Rust** engine on the same **vyges-layout** GDS/boolean kernel that
`vyges-lvs` rides, so the physical-verification pair (DRC + LVS) is one toolset,
one language, one install.

**Describe the rules, not a script.** A small whitespace **rule deck** (`.drc`)
instead of a tool-specific rule language — readable, diffable, schema-checkable.

## Use it

```sh
cargo build --release            # std-only beyond the vyges-layout kernel

vyges-drc check block.gds --rules sky130.drc            # -> violations report
vyges-drc check block.gds --rules sky130.drc --top block --json
vyges-drc check block.gds --rules sky130.drc --fail-on-violation   # exit 3 (CI gate)
vyges-drc fill  block.gds --rules sky130.drc -o filled.gds         # metal-fill generator
vyges-drc demo                                          # built-in layout with violations
# flags: --rules DECK · --top CELL · -o FILE · --json · --fail-on-violation · -h · -V
```

A rule deck keys on the **GDS layer number**; each rule takes its own arguments in
DB units (`#` starts a comment):

```text
# rule     layer  args
width      66     170             # min width on layer 66
space      68     140             # min spacing on layer 68
area       68     20000           # min polygon area (dbu²) on layer 68
density    68     20 70 100000    # coverage on 68 must be 20–70% per 100000-dbu window
connect    5      68              # layers 5 & 68 connect where they overlap (via/contact)
antenna    68     5  400          # per net: layer-68 area ≤ 400 × its layer-5 gate area
enclosure  68     66 40           # every layer-66 shape enclosed by layer-68 with ≥40 margin
span       25     34              # every cut on 25 must span the full width of the metal on 34
venc       19     21 20 8         # via 21 enclosed by metal 19 by ≥20 on one axis (≥8 both sides)
grid       40     1 96            # layer-40 vertices on a 96-dbu y-grid (x free)
track      40     96 192 48       # 96-wide layer-40 wires centered on a 192-dbu track grid (offset 48)
corner     19     21              # every via 21 corner must lie on the merged metal-19 boundary
sep        19     1 143 145 0 100  # tip (edge ≤143) within 100 of a side (edge ≥145) on layer 19
fill       70     30 100000 600 400   # top layer 70 to 30% per window (600-fill, 400-gap)
```

The `fill` rule drives the **`fill` generator** (`vyges-drc fill … -o out.gds`), not
the checker — the checker ignores it.

## Current state (v0.1.2)

**Working & tested:** twelve rule classes plus a fill generator —

- **width** — a shape whose smaller dimension is below the layer minimum;
- **spacing** — two distinct same-layer shapes closer than the minimum (run-length
  on overlapping projections, Euclidean on corners; touching shapes treated as
  connected);
- **area** — a polygon below the layer's minimum area (dbu²);
- **density** — windowed metal coverage: each `window`-square tile (edge tiles
  clamped to the layer bbox) must stay within a `min..max` percent band, flagging
  the offending tile and its measured coverage;
- **antenna** — a per-**net** check: extract nets (union-find over shapes that
  overlap on the same layer, or on a `connect`-declared layer pair), then flag any
  net whose conductor-layer area exceeds `max_ratio ×` its connected gate-layer
  area (process-antenna / plasma-damage protection). The one rule class that needs
  connectivity, not just per-layer geometry;
- **enclosure** — every `inner`-layer shape must sit inside the **merged** `outer`-layer
  region with at least `min` margin on all four sides (e.g. metal must enclose a via);
  reports the worst margin, or "not enclosed" when the inner is not inside the outer.
  Measured against the unioned outer polygons, so enclosure across abutting rectangles is
  seen correctly and a bounding box does not fill in notches.
- **span** — every `cut`-layer shape sitting on a `metal`-layer shape must span that
  metal's full width (its shorter dimension) with edges coincident on both sides;
  flags a cut that is narrower, shifted, or protruding past the metal edge. The
  generic form of an advanced-node "via lands on the full wire width" rule.
- **venc** — asymmetric via enclosure: every `inner` shape must be enclosed by the
  **merged** `outer` region so that, on **at least one axis**, both opposite margins are ≥
  `minor` and at least one is ≥ `major` (the generic advanced-node via **line-end / side**
  enclosure — a large enclosure along the routing direction, a small one across it, on
  only one axis). Margins are projection enclosures against the merged outer geometry, so a
  via flush against a landing pad flags and one enclosed across abutting rectangles does not.
- **grid** — every layer vertex must lie on a manufacturing grid: a vertical edge's x a
  multiple of `xpitch`, a horizontal edge's y a multiple of `ypitch` (a pitch of 1 leaves
  that axis free). Collinear off-grid edges are merged first (a wire drawn as many rects
  counts once), then each off-grid vertex is flagged.
- **track** — a **minimum-width** wire (short dimension equal to `width`) must be centered
  on the routing-track grid: its centerline, on the width axis, a multiple of `pitch`
  offset by `offset`. Collinear wire segments are merged first, so a wire drawn as many
  rects counts once. The generic advanced-node "min-width tracks lie on the routing grid" rule.
- **corner** — every `inner` shape (a via) must match the **merged** `outer` (metal) outline
  at its corners: a convex corner where **both** incident edges depart from the merged outer
  boundary is flagged. The generic advanced-node "a via must be the metal width across the
  routing direction" rule. Built on merged-boundary contour tracing and edge-set booleans.
- **sep** — directional edge-to-edge spacing: on `layer`'s merged boundary, an edge of length
  in `[a_min, a_max]` that **faces** an edge of length in `[b_min, b_max]` across empty space,
  overlapping in projection and closer than `dist`, is flagged (`_max` of `0` = unbounded).
  Classifying boundary edges by length gives the generic advanced-node **tip-to-side** and
  **tip-to-tip** spacing family (tip / wide-tip / narrow-tip / side). Same length class on both
  sides is same-layer spacing; different classes is separation between the two.

Plus the **`fill` generator** (`vyges-drc fill`): for each `fill` rule it tiles every
window below the target with clearance-respecting fill shapes and writes a **filled
GDS** — the fix paired with the density *check*.

GDS load + hierarchy **flatten** (via `vyges-layout`), text + `--json` reports, a
`--fail-on-violation` CI exit code.

**Depth reserved (honest):**

- shapes are taken **per input boundary, not pre-merged** — a wide wire drawn as
  abutting rectangles, or two same-net touching shapes, are measured as drawn;
  proper DRC unions same-layer geometry first (a `vyges-layout` boolean OR) and
  measures the resulting polygons (this also means density can over-count where
  same-layer shapes overlap);
- **edge-coincidence** rules wait on an oriented-edge layer — the merged boundary as
  *directioned* edges plus **local** edge-vs-edge operations — which a bounding-box model
  doesn't carry; a rule that asks "does this edge lie on the merged wire boundary" (a via
  flush to the wire edge, tip-to-side spacing) needs it. Established DRC engineering (every
  production checker does it), on the build list — not open research;
- non-Manhattan polygons fall back to their **bounding box**;
- spacing, antenna-connectivity, and fill clearance scale via a `vyges-layout`
  **`RegionIndex`** (spatial index) — nearby-shape queries rechecked with the exact
  predicate, results identical to all-pairs; density windowing is still a direct scan;
- antenna is a **single-conductor-layer ratio** with a simple overlap-based connect
  model — not yet the cumulative per-metal-layer charge model or a diode-discharge
  credit, and a net with conductor but no gate is treated as not-applicable;
- enclosure takes outer shapes **un-merged**, so an inner enclosed only by the
  *union* of two abutting outer rects reports as under-enclosed;
- `fill` tiles the **design bounding box** (include a die/boundary layer for a sparse
  layout), places fill on the rule's layer at **datatype 0** (a dedicated fill
  datatype is a follow-up), and reaches the size/gap **geometric ceiling** rather
  than exceeding it;
- layers are keyed by **GDS number**; a named-layer mapping is a follow-up.

**Validation roadmap:** correlate against **KLayout / Magic** golden DRC on open
PDKs (sky130, gf180) — the same oracle-backed, golden-corpus discipline the rest
of Loom uses.

## For researchers — open problems

`vyges-drc` is a clean, std-only, fully open codebase (geometry checks over the shared
`vyges-layout` / `vyges-geom` kernel) with honest baselines — a good substrate for
student research. Each item below is a self-contained, publishable direction; the
engine's file-in/file-out boundary means a new method can be dropped in behind the same
violation report and measured against the existing baseline. Each names a **code anchor**
(the file / function that is the natural drop-in point).

1. **Independent validation & differential oracles.** Geometric sign-off is a domain
   where no single open tool is a definitive reference, so cross-checking against
   *independent* implementations and self-consistency invariants is itself the validation
   method. *Open question:* build a reusable **golden-corpus + differential-oracle harness**
   for geometry checks — independent reference implementations, format-invariance
   (a check must give the same verdict on a design and its GDS↔OASIS round-trip), and
   correlation to the established open checkers on open PDKs — that quantifies agreement and
   surfaces every disagreement. *Start from:* the self-consistency oracles already in the
   suite — this engine's brute-force-equivalence spacing test (`indexed_spacing_matches_brute_force_at_scale`
   in `drc.rs`) and `vyges-layout`'s boolean-vs-rasterizer and GDS↔OASIS round-trip tests —
   and generalize them into a corpus-driven harness.
2. **General-angle / non-Manhattan geometry.** Non-rectangular polygons currently fall back
   to their bounding box. *Open question:* exact width, spacing, and area on rectilinear —
   and eventually all-angle — polygons. *Start from:* the bbox fallback in `drc.rs::elem_rect`
   / `polys_on`, and general clipping in `vyges-layout`.
3. **Cumulative antenna charge model.** The antenna check is a single-conductor-layer area
   ratio. *Open question:* the cumulative **per-metal-layer charge** model with a
   diode-discharge credit, still deck-driven and open-PDK-validated. *Start from:*
   `drc.rs::antenna_violations` and the union-find net extraction beside it.
4. **Full-block scaling & spatial indexing.** Spacing, antenna-connectivity, and fill
   clearance ride a uniform-grid spatial index (`vyges-geom::RegionIndex`). *Open question:*
   characterize on full blocks and study index choice (grid vs. R-tree vs. hierarchical) and
   parallelism vs. layout density. *Start from:* `RegionIndex` in `vyges-geom` and its call
   sites in `drc.rs`.

**Working on one of these — or want to?** We're pursuing several ourselves, but the open
frontier is bigger than any one team, and we'd rather build it in the open than wait. If an
item here fits your research, a student project, a thesis, or just an itch, we'd genuinely
like to hear from you — open an issue, send a PR, or reach us at <https://vyges.com/contact>.

## Open core, certified fab decks

`vyges-drc` is open and contains **no foundry-confidential data**. The rule deck
is the plugin boundary: an open reference deck ships for the open PDKs; a
certified per-foundry deck stays private under that foundry's terms — the same
split the rest of the Vyges flow uses.
