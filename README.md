# vyges-drc

Geometric **design-rule check**: a laid-out **GDS** + a per-layer **rule deck** in,
a list of **violations** out.

> **Vyges open EDA tools.** Commercial-grade silicon sign-off capability, built on
> open standards and plain file formats — and meant to be accessible to everyone,
> not only teams who can license a six-figure tool. `vyges-drc` opens up DRC.

> **Stability: experimental (v0.1.0).** The width and spacing checks are real and
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
```

## Current state (v0.1.0)

**Working & tested:** four rule classes —

- **width** — a shape whose smaller dimension is below the layer minimum;
- **spacing** — two distinct same-layer shapes closer than the minimum (run-length
  on overlapping projections, Euclidean on corners; touching shapes treated as
  connected);
- **area** — a polygon below the layer's minimum area (dbu²);
- **density** — windowed metal coverage: each `window`-square tile (edge tiles
  clamped to the layer bbox) must stay within a `min..max` percent band, flagging
  the offending tile and its measured coverage.

GDS load + hierarchy **flatten** (via `vyges-layout`), text + `--json` reports, a
`--fail-on-violation` CI exit code.

**Depth reserved (honest):**

- shapes are taken **per input boundary, not pre-merged** — a wide wire drawn as
  abutting rectangles, or two same-net touching shapes, are measured as drawn;
  proper DRC unions same-layer geometry first (a `vyges-layout` boolean OR) and
  measures the resulting polygons (this also means density can over-count where
  same-layer shapes overlap);
- non-Manhattan polygons fall back to their **bounding box**;
- spacing/density are **brute-force** per layer (a spatial index is the scaling
  pass, as in `vyges-extract`'s coupling grid);
- **enclosure** and **antenna** (the latter needs net connectivity) are the next
  rule classes on the same engine;
- layers are keyed by **GDS number**; a named-layer mapping is a follow-up.

**Validation roadmap:** correlate against **KLayout / Magic** golden DRC on open
PDKs (sky130, gf180) — the same oracle-backed, golden-corpus discipline the rest
of Loom uses.

## Open core, certified fab decks

`vyges-drc` is open and contains **no foundry-confidential data**. The rule deck
is the plugin boundary: an open reference deck ships for the open PDKs; a
certified per-foundry deck stays private under that foundry's terms — the same
split the rest of the Vyges flow uses.
