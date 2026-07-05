//! The check engine — flatten the GDS, collect per-layer shapes, run the rules.
//!
//! v0 measures, per layer:
//! - **width**: a shape whose smaller dimension is below the layer minimum;
//! - **spacing**: two distinct shapes on the same layer closer than the minimum;
//! - **area**: a polygon below the layer's minimum area (dbu²);
//! - **density**: windowed metal coverage outside a `min..max` percent band;
//!
//! per **net** (via union-find connectivity from `connect` rules):
//! - **antenna**: a net's conductor-layer area exceeds `max_ratio ×` its gate area;
//!
//! cross-layer:
//! - **enclosure**: an `inner` shape lacks `min` margin inside an `outer` shape.
//!
//! Plus a **metal-fill generator** (`fill_library`): tile windows below a density
//! target with clearance-respecting fill shapes and emit a filled GDS.
//!
//! Spacing, antenna connectivity, and fill clearance scale via a `vyges-layout`
//! **`RegionIndex`** (spatial index): each query returns only nearby shapes, which
//! are then rechecked with the exact predicate — near-linear, with results identical
//! to the all-pairs scan.
//!
//! Geometry handling: the **enclosure** family (`enclosure`, `venc`) measures against the
//! merged outer region — the layer's true rectilinear polygons unioned (a `vyges-layout`
//! boolean OR) — so a via enclosed across abutting rectangles, or flush against a landing
//! pad, is judged on the real merged shape rather than a bounding box. The per-shape rules
//! (width/space/area/density) still take each input `Boundary` as drawn and are **not
//! pre-merged** (depth reserved). Non-Manhattan polygons fall back to their bounding box;
//! touching/overlapping shapes are treated as connected (not a spacing violation).

use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::layout::boolean::{self, Op};
use crate::layout::contour::trace_contours;
use crate::layout::edges::{edges_not, ring_edges, separation, Axis, Edge};
use crate::layout::flatten;
use crate::layout::geom::{self, Rect};
use crate::layout::gds::{Element, Library};
use crate::layout::index::RegionIndex;
use crate::rules::Rules;

#[derive(Debug, Clone)]
pub struct Violation {
    pub rule: &'static str, // width|space|area|density|antenna|enclosure|span|venc|corner|sep|c2c|grid|track
    pub layer: i16,
    /// The bound that was violated. DB units for width/space, DB units² for area,
    /// **percent** for density, and **centi-ratio** (ratio × 100) for antenna.
    pub limit: i64,
    /// The measured value, in the same unit as `limit` for that rule.
    pub value: i64,
    pub a: Rect,            // the offending shape, spacing pair's first, or density window
    pub b: Option<Rect>,    // the second shape, for a spacing violation
}

/// Overlap area of two axis-aligned rects in DB units² (0 if disjoint).
fn overlap_area(a: &Rect, b: &Rect) -> i64 {
    let ix = (a.x1.min(b.x1) - a.x0.max(b.x0)).max(0) as i64;
    let iy = (a.y1.min(b.y1) - a.y0.max(b.y0)).max(0) as i64;
    ix * iy
}

/// Edge-to-edge spacing between two axis-aligned rects in DB units. `None` when
/// they overlap or touch (treated as connected, not a spacing case). When their
/// projections overlap on one axis it is the gap on the other (run-length
/// spacing); a corner-to-corner pair uses the Euclidean diagonal.
pub fn spacing(a: &Rect, b: &Rect) -> Option<i64> {
    let gx = (b.x0 - a.x1).max(a.x0 - b.x1); // >0 ⇒ separated in x
    let gy = (b.y0 - a.y1).max(a.y0 - b.y1); // >0 ⇒ separated in y
    if gx <= 0 && gy <= 0 {
        None // overlap / touch
    } else if gx > 0 && gy > 0 {
        Some(((gx as f64).hypot(gy as f64)).round() as i64)
    } else {
        Some(gx.max(gy) as i64) // edge case: one gap is ≤0, take the positive one
    }
}

/// Resolve the top cell to operate on (the only cell, or the named one).
fn pick_top(lib: &Library, top: Option<&str>) -> Result<String, String> {
    match top {
        Some(t) => Ok(t.to_string()),
        None if lib.cells.len() == 1 => Ok(lib.cells[0].name.clone()),
        None => Err(format!(
            "{} cells; pass a top cell ({})",
            lib.cells.len(),
            lib.cells.iter().map(|c| c.name.as_str()).collect::<Vec<_>>().join(", ")
        )),
    }
}

/// A measurable element's `(layer, rect)` — a rectangle where possible, else the
/// bounding box; `None` for elements we don't measure (Path/Text).
fn elem_rect(e: &Element) -> (i16, Option<Rect>) {
    match e {
        Element::Boundary { layer, pts, .. } | Element::Box { layer, pts, .. } => {
            (*layer, Rect::from_boundary(pts).or_else(|| geom::bbox(pts)))
        }
        _ => (0, None),
    }
}

/// Every edge of a closed polygon is axis-aligned.
fn is_manhattan(pts: &[(i32, i32)]) -> bool {
    let n = if pts.len() >= 2 && pts.first() == pts.last() { pts.len() - 1 } else { pts.len() };
    (0..n).all(|i| {
        let (x1, y1) = pts[i];
        let (x2, y2) = pts[(i + 1) % n];
        x1 == x2 || y1 == y2
    })
}

/// A measurable element's `(layer, polygon)` — the **true** rectilinear polygon (notches
/// preserved), or the bounding box only for a genuinely non-Manhattan shape. Unlike
/// `elem_rect`, a rectilinear L/notched polygon is kept whole rather than collapsed to its
/// bbox, so unioning these gives the real merged geometry (enclosure needs this).
fn elem_poly(e: &Element) -> (i16, Option<Vec<(i32, i32)>>) {
    match e {
        Element::Boundary { layer, pts, .. } | Element::Box { layer, pts, .. } => {
            if is_manhattan(pts) {
                (*layer, Some(pts.clone()))
            } else {
                (*layer, geom::bbox(pts).map(|b| b.as_boundary()))
            }
        }
        _ => (0, None),
    }
}

/// Is `q` fully covered by the union of `region` (merged tiles)? Exact: AND `q` with the
/// local tiles (fetched by the index) and compare area. An empty query is vacuously
/// covered.
fn covered(q: &Rect, region: &[Rect], idx: &RegionIndex) -> bool {
    if q.x0 >= q.x1 || q.y0 >= q.y1 {
        return true;
    }
    let local: Vec<Rect> = idx.overlaps(q).into_iter().map(|i| region[i as usize]).collect();
    if local.is_empty() {
        return false;
    }
    let inter = boolean::boolean(&[*q], &local, Op::And);
    inter.iter().map(|r| r.area()).sum::<i64>() == q.area()
}

/// Projection enclosure of `inner` by the merged `region` on one side: the largest
/// margin `d ≤ cap` for which the full side-strip of width `d` is covered. `side` is
/// 0=left, 1=right, 2=bottom, 3=top. Monotone in `d`, so a binary search is exact.
fn side_margin(inner: &Rect, side: u8, region: &[Rect], idx: &RegionIndex, cap: i64) -> i64 {
    let strip = |d: i64| match side {
        0 => Rect { x0: inner.x0 - d as i32, y0: inner.y0, x1: inner.x0, y1: inner.y1 },
        1 => Rect { x0: inner.x1, y0: inner.y0, x1: inner.x1 + d as i32, y1: inner.y1 },
        2 => Rect { x0: inner.x0, y0: inner.y0 - d as i32, x1: inner.x1, y1: inner.y0 },
        _ => Rect { x0: inner.x0, y0: inner.y1, x1: inner.x1, y1: inner.y1 + d as i32 },
    };
    let (mut lo, mut hi) = (0i64, cap);
    while lo < hi {
        let mid = (lo + hi + 1) / 2;
        if covered(&strip(mid), region, idx) {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    lo
}

/// Every layer that needs its true rectilinear polygons unioned into a merged region — the
/// `outer` of enclosure/venc/corner rules and the `layer` of sep rules.
fn merged_layers(rules: &Rules) -> BTreeSet<i16> {
    rules
        .enclosure
        .iter()
        .map(|r| r.outer)
        .chain(rules.venc.iter().map(|r| r.outer))
        .chain(rules.corner.iter().map(|r| r.outer))
        .chain(rules.sep.iter().map(|r| r.layer))
        .chain(rules.c2c.iter().map(|r| r.layer))
        .collect()
}

/// The merged region (unioned tiles) of each layer that needs one. Empty when none.
fn merged_regions(
    rules: &Rules,
    polys_by_layer: &BTreeMap<i16, Vec<Vec<(i32, i32)>>>,
) -> BTreeMap<i16, Vec<Rect>> {
    let mut merged = BTreeMap::new();
    let empty = Vec::new();
    for l in merged_layers(rules) {
        let polys = polys_by_layer.get(&l).unwrap_or(&empty);
        merged.insert(l, boolean::boolean_poly(polys, &[], Op::Or));
    }
    merged
}

/// Perpendicular gap between two parallel axis-aligned edges (their separation distance).
fn edge_gap(a: &Edge, b: &Edge) -> i64 {
    if a.axis() == Axis::Horizontal {
        (a.a.1 - b.a.1).abs() as i64
    } else {
        (a.a.0 - b.a.0).abs() as i64
    }
}

/// Collapse (ea,eb) and (eb,ea) to one unordered pair — for same-class (space) spacing.
fn dedup_edge_pairs(pairs: Vec<(Edge, Edge)>) -> Vec<(Edge, Edge)> {
    let norm = |e: &Edge| (e.a.0.min(e.b.0), e.a.1.min(e.b.1), e.a.0.max(e.b.0), e.a.1.max(e.b.1));
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for p in pairs {
        let (a, b) = (norm(&p.0), norm(&p.1));
        let key = if a <= b { (a, b) } else { (b, a) };
        if seen.insert(key) {
            out.push(p);
        }
    }
    out
}

/// Directional edge-spacing: on the merged boundary `edges`, an edge of length in the `a`
/// class facing an edge of the `b` class closer than `dist` violates. Same class ⇒ space
/// (dedup unordered pairs); different classes ⇒ separation.
fn sep_violations(rule: &crate::rules::Sep, edges: &[Edge], out: &mut Vec<Violation>) {
    let in_class = |e: &Edge, lo: i64, hi: i64| {
        let l = e.len();
        l >= lo && (hi == 0 || l <= hi)
    };
    let a: Vec<Edge> = edges.iter().copied().filter(|e| in_class(e, rule.a_min, rule.a_max)).collect();
    let b: Vec<Edge> = edges.iter().copied().filter(|e| in_class(e, rule.b_min, rule.b_max)).collect();
    let pairs = separation(&a, &b, rule.dist);
    let pairs = if (rule.a_min, rule.a_max) == (rule.b_min, rule.b_max) { dedup_edge_pairs(pairs) } else { pairs };
    for (ea, eb) in pairs {
        out.push(Violation {
            rule: "sep",
            layer: rule.layer,
            limit: rule.dist,
            value: edge_gap(&ea, &eb),
            a: bbox_edge(&ea),
            b: Some(bbox_edge(&eb)),
        });
    }
}

fn bbox_edge(e: &Edge) -> Rect {
    Rect { x0: e.a.0.min(e.b.0), y0: e.a.1.min(e.b.1), x1: e.a.0.max(e.b.0), y1: e.a.1.max(e.b.1) }
}

/// Outward normal of a directed edge (right of travel; solid on the left).
fn outward_edge(e: &Edge) -> (i64, i64) {
    ((e.b.1 - e.a.1).signum() as i64, -((e.b.0 - e.a.0).signum() as i64))
}

/// Strict projection overlap of two parallel axis-aligned edges (side-to-side, not corner).
fn projecting_edges(a: &Edge, b: &Edge) -> bool {
    if a.axis() == Axis::Horizontal {
        let (x1a, x1b) = (a.a.0.min(a.b.0), a.a.0.max(a.b.0));
        let (x2a, x2b) = (b.a.0.min(b.b.0), b.a.0.max(b.b.0));
        x1b > x2a && x2b > x1a
    } else {
        let (y1a, y1b) = (a.a.1.min(a.b.1), a.a.1.max(a.b.1));
        let (y2a, y2b) = (b.a.1.min(b.b.1), b.a.1.max(b.b.1));
        y1b > y2a && y2b > y1a
    }
}

/// The two closest points (on a, on b) between two axis-aligned segments.
fn closest_pts(a: &Edge, b: &Edge) -> ((i32, i32), (i32, i32)) {
    let clamp = |p: (i32, i32), s: &Edge| {
        let (x0, y0) = (s.a.0.min(s.b.0), s.a.1.min(s.b.1));
        let (x1, y1) = (s.a.0.max(s.b.0), s.a.1.max(s.b.1));
        (p.0.clamp(x0, x1), p.1.clamp(y0, y1))
    };
    let dsq = |p: (i32, i32), q: (i32, i32)| {
        let (dx, dy) = ((p.0 - q.0) as i64, (p.1 - q.1) as i64);
        dx * dx + dy * dy
    };
    let cands = [(a.a, clamp(a.a, b)), (a.b, clamp(a.b, b)), (clamp(b.a, a), b.a), (clamp(b.b, a), b.b)];
    *cands.iter().min_by_key(|(p, q)| dsq(*p, *q)).unwrap()
}

fn point_in_region(p: (i32, i32), tiles: &[Rect], idx: &RegionIndex) -> bool {
    let q = Rect { x0: p.0, y0: p.1, x1: p.0, y1: p.1 };
    idx.overlaps(&q)
        .into_iter()
        .any(|i| { let r = &tiles[i as usize]; r.x0 <= p.0 && p.0 <= r.x1 && r.y0 <= p.1 && p.1 <= r.y1 })
}

/// Corner-to-corner spacing: parallel non-projecting merged-boundary edges whose closest
/// approach (at corners) is a `dist`-bounded gap across empty space that both edges face.
fn c2c_violations(rule: &crate::rules::C2c, edges: &[Edge], tiles: &[Rect], out: &mut Vec<Violation>) {
    let d2 = rule.dist * rule.dist;
    let boxes: Vec<Rect> = edges.iter().map(bbox_edge).collect();
    let eidx = RegionIndex::build(&boxes);
    let tidx = RegionIndex::build(tiles);
    let mut seen = std::collections::HashSet::new();
    for i in 0..edges.len() {
        for j in eidx.within(&boxes[i], rule.dist as i32, None) {
            let j = j as usize;
            if j <= i {
                continue;
            }
            let (ea, eb) = (&edges[i], &edges[j]);
            if ea.axis() != eb.axis() {
                continue; // corner-to-corner is between parallel edges
            }
            let (ca, cb) = closest_pts(ea, eb);
            let (dx, dy) = ((ca.0 - cb.0) as i64, (ca.1 - cb.1) as i64);
            let ds = dx * dx + dy * dy;
            if ds == 0 || ds >= d2 {
                continue;
            }
            if projecting_edges(ea, eb) {
                continue; // projection-overlapping is side-to-side / tip spacing, not corner
            }
            let mid = ((ca.0 + cb.0) / 2, (ca.1 + cb.1) / 2);
            if point_in_region(mid, tiles, &tidx) {
                continue; // material between — not a space
            }
            // both edges must face the gap (outward normals point toward each other)
            let dir = ((cb.0 - ca.0) as i64, (cb.1 - ca.1) as i64);
            let (oa, ob) = (outward_edge(ea), outward_edge(eb));
            if oa.0 * dir.0 + oa.1 * dir.1 <= 0 || ob.0 * (-dir.0) + ob.1 * (-dir.1) <= 0 {
                continue;
            }
            let (na, nb) = (bbox_edge(ea), bbox_edge(eb));
            let key = if (na.x0, na.y0) <= (nb.x0, nb.y0) { (na, nb) } else { (nb, na) };
            let k = (key.0.x0, key.0.y0, key.0.x1, key.0.y1, key.1.x0, key.1.y0, key.1.x1, key.1.y1);
            if !seen.insert(k) {
                continue;
            }
            out.push(Violation {
                rule: "c2c",
                layer: rule.layer,
                limit: rule.dist,
                value: (ds as f64).sqrt() as i64,
                a: na,
                b: Some(nb),
            });
        }
    }
}

/// Run the rule deck over the flattened top cell of `lib`.
pub fn check_library(
    lib: &Library,
    top: Option<&str>,
    rules: &Rules,
) -> Result<Vec<Violation>, String> {
    let top = pick_top(lib, top)?;
    let cell = flatten::flatten(lib, &top)?;

    // shapes per layer (rectangles where possible; bbox for non-Manhattan)
    let mut by_layer: BTreeMap<i16, Vec<Rect>> = BTreeMap::new();
    for e in &cell.elements {
        let (layer, rect) = elem_rect(e);
        if let Some(r) = rect {
            by_layer.entry(layer).or_default().push(r);
        }
    }

    // True rectilinear polygons for every layer that needs a merged region (enclosure/venc/
    // corner outer, sep layer), unioned so checks measure against real geometry (a bounding
    // box would fill notches).
    let want = merged_layers(rules);
    let mut polys_by_layer: BTreeMap<i16, Vec<Vec<(i32, i32)>>> = BTreeMap::new();
    if !want.is_empty() {
        for e in &cell.elements {
            let (layer, poly) = elem_poly(e);
            if want.contains(&layer) {
                if let Some(p) = poly {
                    polys_by_layer.entry(layer).or_default().push(p);
                }
            }
        }
    }
    let merged_outer = merged_regions(rules, &polys_by_layer);

    let mut viols = Vec::new();
    for (&layer, shapes) in &by_layer {
        if let Some(&min_w) = rules.width.get(&layer) {
            for r in shapes {
                let w = ((r.x1 - r.x0).min(r.y1 - r.y0)) as i64;
                if w < min_w {
                    viols.push(Violation { rule: "width", layer, limit: min_w, value: w, a: *r, b: None });
                }
            }
        }
        if let Some(&min_s) = rules.space.get(&layer) {
            // Only shapes within `min_s` of one another can violate; a RegionIndex
            // returns those candidates so this is near-linear instead of all-pairs.
            // Each candidate is rechecked with the exact `spacing`, and `j > i` keeps
            // every unordered pair reported once — identical results to all-pairs.
            let idx = RegionIndex::build(shapes);
            for i in 0..shapes.len() {
                for jd in idx.within(&shapes[i], min_s as i32, Some(i as u32)) {
                    let j = jd as usize;
                    if j <= i {
                        continue;
                    }
                    if let Some(s) = spacing(&shapes[i], &shapes[j]) {
                        if s < min_s {
                            viols.push(Violation {
                                rule: "space",
                                layer,
                                limit: min_s,
                                value: s,
                                a: shapes[i],
                                b: Some(shapes[j]),
                            });
                        }
                    }
                }
            }
        }
        if let Some(&min_a) = rules.area.get(&layer) {
            for r in shapes {
                let area = (r.x1 - r.x0) as i64 * (r.y1 - r.y0) as i64;
                if area < min_a {
                    viols.push(Violation { rule: "area", layer, limit: min_a, value: area, a: *r, b: None });
                }
            }
        }
        if let Some(d) = rules.density.get(&layer) {
            density_violations(layer, shapes, *d, &mut viols);
        }
    }
    // antenna is a per-net (connectivity) check — needs all layers together
    if !rules.antenna.is_empty() {
        let all: Vec<(i16, Rect)> =
            by_layer.iter().flat_map(|(&l, shapes)| shapes.iter().map(move |r| (l, *r))).collect();
        antenna_violations(&all, &rules.connect, &rules.antenna, &mut viols);
    }
    // enclosure is cross-layer: inner inside the merged outer region
    let no_inner = Vec::new();
    let no_outer = Vec::new();
    for rule in &rules.enclosure {
        let inners = by_layer.get(&rule.inner).unwrap_or(&no_inner);
        let outer = merged_outer.get(&rule.outer).unwrap_or(&no_outer);
        enclosure_violations(rule, inners, outer, &mut viols);
    }
    // span is cross-layer (a cut must span the width of the metal it sits on)
    for rule in &rules.span {
        span_violations(rule, &by_layer, &mut viols);
    }
    // venc is cross-layer (asymmetric via enclosure against the merged outer region)
    for rule in &rules.venc {
        let inners = by_layer.get(&rule.inner).unwrap_or(&no_inner);
        let outer = merged_outer.get(&rule.outer).unwrap_or(&no_outer);
        venc_violations(rule, inners, outer, &mut viols);
    }
    // corner is cross-layer: an inner via's corners must lie on the merged outer boundary
    for rule in &rules.corner {
        let outer = merged_outer.get(&rule.outer).unwrap_or(&no_outer);
        let edges = outer_boundary_edges(outer);
        // de-duplicate coincident inner vias (merged semantics)
        let mut seen = std::collections::HashSet::new();
        let inners: Vec<Rect> = by_layer
            .get(&rule.inner)
            .unwrap_or(&no_inner)
            .iter()
            .copied()
            .filter(|r| seen.insert((r.x0, r.y0, r.x1, r.y1)))
            .collect();
        corner_violations(rule.inner, &inners, &edges, &mut viols);
    }
    // sep is per-layer directional edge spacing on the merged boundary
    for rule in &rules.sep {
        let tiles = merged_outer.get(&rule.layer).unwrap_or(&no_outer);
        let edges: Vec<Edge> = trace_contours(tiles).iter().flat_map(|r| ring_edges(r)).collect();
        sep_violations(rule, &edges, &mut viols);
    }
    // c2c is per-layer corner-to-corner spacing on the merged boundary
    for rule in &rules.c2c {
        let tiles = merged_outer.get(&rule.layer).unwrap_or(&no_outer);
        let edges: Vec<Edge> = trace_contours(tiles).iter().flat_map(|r| ring_edges(r)).collect();
        c2c_violations(rule, &edges, tiles, &mut viols);
    }
    // grid is per-layer (vertices must lie on a manufacturing grid)
    for rule in &rules.grid {
        if let Some(shapes) = by_layer.get(&rule.layer) {
            grid_violations(rule, shapes, &mut viols);
        }
    }
    // track is per-layer (min-width wire centerlines must lie on the routing-track grid)
    for rule in &rules.track {
        if let Some(shapes) = by_layer.get(&rule.layer) {
            track_violations(rule, shapes, &mut viols);
        }
    }
    Ok(viols)
}

/// Windowed metal-density check: tile the layer's bounding box into `window`-square
/// tiles (edge tiles clamped to the bbox), and flag any tile whose coverage —
/// `100·Σ(shape∩tile)/tile_area`, rounded — falls outside `min..=max` percent.
///
/// v0 sums per-shape overlaps **without unioning** the layer first, so overlapping
/// same-layer shapes can over-count coverage (the same not-pre-merged caveat as
/// width/space). Proper density unions the layer first; that is the depth pass.
fn density_violations(layer: i16, shapes: &[Rect], d: crate::rules::Density, out: &mut Vec<Violation>) {
    let Some(bb) = bbox(shapes) else { return };
    let w = d.window;
    let mut ty0 = bb.y0;
    while ty0 < bb.y1 {
        let ty1 = (ty0 + w as i32).min(bb.y1);
        let mut tx0 = bb.x0;
        while tx0 < bb.x1 {
            let tx1 = (tx0 + w as i32).min(bb.x1);
            let tile = Rect::new(tx0, ty0, tx1, ty1);
            let tile_area = (tx1 - tx0) as i64 * (ty1 - ty0) as i64;
            if tile_area > 0 {
                let covered: i64 = shapes.iter().map(|s| overlap_area(s, &tile)).sum();
                let pct = (100 * covered / tile_area).clamp(0, 100);
                if pct < d.min_pct {
                    out.push(Violation { rule: "density", layer, limit: d.min_pct, value: pct, a: tile, b: None });
                } else if pct > d.max_pct {
                    out.push(Violation { rule: "density", layer, limit: d.max_pct, value: pct, a: tile, b: None });
                }
            }
            tx0 = tx1.max(tx0 + 1);
        }
        ty0 = ty1.max(ty0 + 1);
    }
}

/// Area of a single rect in DB units².
fn rect_area(r: &Rect) -> i64 {
    (r.x1 - r.x0) as i64 * (r.y1 - r.y0) as i64
}

/// Enclosure: every `inner`-layer shape must sit inside an `outer`-layer shape with
/// at least `min` margin on all four sides. We take the *best* enclosing outer
/// (largest min-side margin); a `value` of `-1` flags an inner not enclosed by any
/// single outer shape.
///
/// v0 bound (honest): outer shapes are not pre-merged, so an inner enclosed only by
/// the *union* of two abutting outer rects reports as under-enclosed — same
/// not-pre-merged caveat as the geometry rules.
fn enclosure_violations(
    rule: &crate::rules::Enclosure,
    inners: &[Rect],
    outer: &[Rect],
    out: &mut Vec<Violation>,
) {
    if inners.is_empty() {
        return;
    }
    let idx = RegionIndex::build(outer);
    for inner in inners {
        // `inner` must sit inside the merged outer region; else the not-enclosed sentinel.
        if !covered(inner, outer, &idx) {
            out.push(Violation { rule: "enclosure", layer: rule.inner, limit: rule.min, value: -1, a: *inner, b: None });
            continue;
        }
        // min enclosure over the four sides (capped at `min`; adequate ⇒ no violation).
        let m = (0..4).map(|s| side_margin(inner, s, outer, &idx, rule.min)).min().unwrap_or(0);
        if m < rule.min {
            out.push(Violation { rule: "enclosure", layer: rule.inner, limit: rule.min, value: m, a: *inner, b: None });
        }
    }
}

/// Via-span: every `cut` shape sitting on a `metal` shape must span that metal's full
/// width (its shorter dimension) with edges coincident on both width-sides. Each cut is
/// matched to the metal it overlaps most; a cut on no metal is skipped (that is a
/// coverage rule, not this one). `value` is the total edge deviation in DB units — how
/// far the cut's two width-edges are from the metal's — and `limit` is 0 (must be
/// exact), so a cut that is narrower, shifted, or protruding past the metal all flag.
///
/// v0 bound (honest): metals are not pre-merged, so a wire drawn as abutting rects can
/// mis-measure the width under a cut; a square metal (no distinct width axis) uses the
/// x-axis by convention.
fn span_violations(
    rule: &crate::rules::Span,
    by_layer: &BTreeMap<i16, Vec<Rect>>,
    out: &mut Vec<Violation>,
) {
    let Some(cuts) = by_layer.get(&rule.cut) else { return };
    let empty = Vec::new();
    let metals = by_layer.get(&rule.metal).unwrap_or(&empty);
    for cut in cuts {
        // the metal this cut sits on = the metal shape it overlaps most
        let Some(m) = metals
            .iter()
            .filter(|m| overlap_area(cut, m) > 0)
            .max_by_key(|m| overlap_area(cut, m))
        else {
            continue; // no metal under this cut — not this rule's concern
        };
        // width axis = the metal's shorter dimension (x on a square, by convention);
        // the cut must span the metal edge-to-edge along it.
        let dev = if (m.x1 - m.x0) <= (m.y1 - m.y0) {
            (cut.x0 - m.x0).abs() as i64 + (cut.x1 - m.x1).abs() as i64
        } else {
            (cut.y0 - m.y0).abs() as i64 + (cut.y1 - m.y1).abs() as i64
        };
        if dev > 0 {
            out.push(Violation { rule: "span", layer: rule.cut, limit: 0, value: dev, a: *cut, b: Some(*m) });
        }
    }
}

/// Asymmetric via-enclosure: every `inner` shape must be enclosed by a single `outer`
/// shape such that, on at least one axis, the enclosure on both opposite sides is ≥
/// `minor` and on at least one side is ≥ `major` — the generic "line-end / side" via
/// enclosure (a large enclosure along the routing direction, a small one across it,
/// required on only one axis). `value` is the best major-side enclosure the inner
/// actually achieves on a `minor`-qualifying axis (`-1` when no single outer encloses
/// it), and `limit` is `major`.
///
/// Margins are projection enclosures against the **merged** outer region (its true
/// rectilinear polygons unioned), so a landing pad flush on one side flags correctly and a
/// wire drawn as abutting rectangles does not spuriously over-report — matching a
/// merged-geometry checker. `value` is the best major-side enclosure on a `minor`-qualifying
/// axis (`0` when enclosed but no axis qualifies, `-1` when the inner is not inside the
/// merged outer at all).
fn venc_violations(
    rule: &crate::rules::Venc,
    inners: &[Rect],
    outer: &[Rect],
    out: &mut Vec<Violation>,
) {
    if inners.is_empty() {
        return;
    }
    let idx = RegionIndex::build(outer);
    for inner in inners {
        if !covered(inner, outer, &idx) {
            out.push(Violation { rule: "venc", layer: rule.inner, limit: rule.major, value: -1, a: *inner, b: None });
            continue;
        }
        // per-side enclosure against the merged outer region (capped at `major`).
        let l = side_margin(inner, 0, outer, &idx, rule.major);
        let r = side_margin(inner, 1, outer, &idx, rule.major);
        let b = side_margin(inner, 2, outer, &idx, rule.major);
        let t = side_margin(inner, 3, outer, &idx, rule.major);
        // an axis "qualifies" when both its opposite margins meet `minor`; its contribution
        // is then the larger of the two (the major side).
        let mut best_major: i64 = 0;
        if l.min(r) >= rule.minor {
            best_major = best_major.max(l.max(r));
        }
        if b.min(t) >= rule.minor {
            best_major = best_major.max(b.max(t));
        }
        if best_major < rule.major {
            out.push(Violation { rule: "venc", layer: rule.inner, limit: rule.major, value: best_major, a: *inner, b: None });
        }
    }
}

/// The four boundary edges of a via rectangle (direction is irrelevant to coincidence).
fn via_edges(v: &Rect) -> [Edge; 4] {
    [
        Edge { a: (v.x0, v.y0), b: (v.x1, v.y0) },
        Edge { a: (v.x1, v.y0), b: (v.x1, v.y1) },
        Edge { a: (v.x1, v.y1), b: (v.x0, v.y1) },
        Edge { a: (v.x0, v.y1), b: (v.x0, v.y0) },
    ]
}

/// Supporting line of an axis-aligned edge: `(0, y)` horizontal or `(1, x)` vertical.
fn edge_line(e: &Edge) -> (u8, i32) {
    if e.a.1 == e.b.1 {
        (0, e.a.1)
    } else {
        (1, e.a.0)
    }
}

fn shares_endpoint(a: &Edge, b: &Edge) -> bool {
    let ae = [a.a, a.b];
    ae.contains(&b.a) || ae.contains(&b.b)
}

/// Via-corner check: an `inner` shape is flagged when it has a corner where **both**
/// incident edges depart from the merged `outer` boundary — a convex corner not backed by
/// metal on either side (the via fails to match the metal outline there). `outer_edges` are
/// the merged outer boundary edges grouped by supporting line; `inners` are de-duplicated.
fn corner_violations(
    inner_layer: i16,
    inners: &[Rect],
    outer_edges: &HashMap<(u8, i32), Vec<Edge>>,
    out: &mut Vec<Violation>,
) {
    for via in inners {
        let mut local: Vec<Edge> = Vec::new();
        for k in [(0, via.y0), (0, via.y1), (1, via.x0), (1, via.x1)] {
            if let Some(es) = outer_edges.get(&k) {
                local.extend_from_slice(es);
            }
        }
        // portions of the via's edges that are NOT on the merged outer boundary
        let nonc = edges_not(&via_edges(via), &local);
        let nh: Vec<Edge> = nonc.iter().copied().filter(|e| e.axis() == Axis::Horizontal).collect();
        let nv: Vec<Edge> = nonc.iter().copied().filter(|e| e.axis() == Axis::Vertical).collect();
        // a convex corner where a non-coincident H edge meets a non-coincident V edge
        if nh.iter().any(|h| nv.iter().any(|v| shares_endpoint(h, v))) {
            out.push(Violation { rule: "corner", layer: inner_layer, limit: 0, value: 1, a: *via, b: None });
        }
    }
}

/// The merged outer boundary edges (from the merged region tiles), grouped by supporting
/// line for per-via lookup.
fn outer_boundary_edges(tiles: &[Rect]) -> HashMap<(u8, i32), Vec<Edge>> {
    let mut by_line: HashMap<(u8, i32), Vec<Edge>> = HashMap::new();
    for ring in trace_contours(tiles) {
        for e in ring_edges(&ring) {
            by_line.entry(edge_line(&e)).or_default().push(e);
        }
    }
    by_line
}

/// Manufacturing-grid check: every layer vertex must lie on the grid — its x a multiple
/// of `xpitch`, its y a multiple of `ypitch` (a pitch of 1 leaves that axis free).
/// Collinear off-grid edges are merged first (a wire drawn as many abutting rects is one
/// edge), then each merged edge's two endpoint vertices are flagged (deduplicated) — so
/// the count matches a merged-geometry vertex check like KLayout's `ongrid`.
fn grid_violations(rule: &crate::rules::Grid, shapes: &[Rect], out: &mut Vec<Violation>) {
    // group off-grid edge segments by their fixed coordinate:
    //   vertical edges (fixed x, spanning y) checked against xpitch;
    //   horizontal edges (fixed y, spanning x) checked against ypitch.
    let mut vert: BTreeMap<i32, Vec<(i32, i32)>> = BTreeMap::new();
    let mut horiz: BTreeMap<i32, Vec<(i32, i32)>> = BTreeMap::new();
    for r in shapes {
        if r.x0 as i64 % rule.xpitch != 0 {
            vert.entry(r.x0).or_default().push((r.y0, r.y1));
        }
        if r.x1 as i64 % rule.xpitch != 0 {
            vert.entry(r.x1).or_default().push((r.y0, r.y1));
        }
        if r.y0 as i64 % rule.ypitch != 0 {
            horiz.entry(r.y0).or_default().push((r.x0, r.x1));
        }
        if r.y1 as i64 % rule.ypitch != 0 {
            horiz.entry(r.y1).or_default().push((r.x0, r.x1));
        }
    }
    let mut pts = std::collections::BTreeSet::new();
    merged_edge_endpoints(true, vert, &mut pts);
    merged_edge_endpoints(false, horiz, &mut pts);
    for (x, y) in pts {
        out.push(Violation { rule: "grid", layer: rule.layer, limit: 0, value: 0, a: Rect::new(x, y, x, y), b: None });
    }
}

/// Routing-track centerline: a **minimum-width** wire (its short dimension equal to
/// `width`) must be centered on the routing-track grid — its centerline, on the width
/// axis, a multiple of `pitch` offset by `offset`. Only min-width wires whose width-edges
/// are on the `width` grid are checked (wider wires, and off-grid edges, are other rules'
/// concern). The width axis is the shorter dimension (a tie defaults to y).
///
/// Collinear off-track min-width wire segments are merged first (a wire drawn as many
/// abutting rects is one wire), so the count matches a merged-geometry check; one violation
/// is emitted per merged wire.
fn track_violations(rule: &crate::rules::Track, shapes: &[Rect], out: &mut Vec<Violation>) {
    // off-track wires grouped by centerline: horizontal (y-centerline → x-spans) and
    // vertical (x-centerline → y-spans).
    let mut horiz: BTreeMap<i64, Vec<(i32, i32)>> = BTreeMap::new();
    let mut vert: BTreeMap<i64, Vec<(i32, i32)>> = BTreeMap::new();
    for r in shapes {
        let (w, h) = ((r.x1 - r.x0) as i64, (r.y1 - r.y0) as i64);
        if w.min(h) != rule.width {
            continue; // only 1x (minimum-width) wires
        }
        // width axis = the shorter dimension (horizontal wire → y, vertical → x; tie → y)
        let horizontal = h <= w;
        let (lo, hi) = if horizontal { (r.y0 as i64, r.y1 as i64) } else { (r.x0 as i64, r.x1 as i64) };
        if lo % rule.width != 0 || hi % rule.width != 0 {
            continue; // width-edges off the width grid → a grid rule's concern, not this
        }
        let cl = (lo + hi) / 2;
        if (cl - rule.offset).rem_euclid(rule.pitch) != 0 {
            if horizontal {
                horiz.entry(cl).or_default().push((r.x0, r.x1));
            } else {
                vert.entry(cl).or_default().push((r.y0, r.y1));
            }
        }
    }
    let half = (rule.width / 2) as i32;
    emit_merged_track(true, horiz, half, rule.layer, out);
    emit_merged_track(false, vert, half, rule.layer, out);
}

/// Merge collinear abutting/overlapping wire spans per centerline and emit one `track`
/// violation per merged wire. `horizontal` → centerline is a y value with x-spans.
fn emit_merged_track(
    horizontal: bool,
    by_cl: BTreeMap<i64, Vec<(i32, i32)>>,
    half: i32,
    layer: i16,
    out: &mut Vec<Violation>,
) {
    for (cl, mut spans) in by_cl {
        spans.sort_unstable();
        let mut cur = spans[0];
        let mut segs = Vec::new();
        for s in spans.into_iter().skip(1) {
            if s.0 <= cur.1 {
                cur.1 = cur.1.max(s.1);
            } else {
                segs.push(cur);
                cur = s;
            }
        }
        segs.push(cur);
        let cl = cl as i32;
        for (a, b) in segs {
            let rect = if horizontal {
                Rect::new(a, cl - half, b, cl + half)
            } else {
                Rect::new(cl - half, a, cl + half, b)
            };
            out.push(Violation { rule: "track", layer, limit: 0, value: 0, a: rect, b: None });
        }
    }
}

/// Merge collinear edge segments (same fixed coordinate, overlapping/abutting spans) and
/// collect the two endpoint vertices of each merged segment into `pts` (deduplicated).
/// `x_fixed` picks the axis: `true` = vertical edges keyed by x with y-spans.
fn merged_edge_endpoints(
    x_fixed: bool,
    by_coord: BTreeMap<i32, Vec<(i32, i32)>>,
    pts: &mut std::collections::BTreeSet<(i32, i32)>,
) {
    for (coord, mut spans) in by_coord {
        spans.sort_unstable();
        let mut cur = spans[0];
        let mut segs = Vec::new();
        for s in spans.into_iter().skip(1) {
            if s.0 <= cur.1 {
                cur.1 = cur.1.max(s.1); // overlap or abut -> extend
            } else {
                segs.push(cur);
                cur = s;
            }
        }
        segs.push(cur);
        for (a, b) in segs {
            if x_fixed {
                pts.insert((coord, a));
                pts.insert((coord, b));
            } else {
                pts.insert((a, coord));
                pts.insert((b, coord));
            }
        }
    }
}

/// Two rects are electrically touching if they overlap or abut (`spacing` returns
/// `None` exactly then).
fn touch_or_overlap(a: &Rect, b: &Rect) -> bool {
    spacing(a, b).is_none()
}

/// A fill candidate keeps at least `gap` clearance from `other` (strictly: they are
/// separated by ≥ `gap`, never overlapping/touching).
fn keeps_gap(a: &Rect, other: &Rect, gap: i64) -> bool {
    matches!(spacing(a, other), Some(d) if d >= gap)
}

/// **Metal-fill generator** (the fix paired with the density *check*): for each fill
/// rule, tile each `window` that is below `target%` with `size`-square fill shapes
/// that keep `gap` clearance from existing geometry and from each other, until the
/// window reaches the target (or its grid is exhausted). Returns the input library
/// with the fill added to `top`, plus the number of fill shapes placed.
///
/// v0 bounds (honest): clearance to existing geometry uses a `RegionIndex` (rechecked
/// exactly); clearance to already-placed fill stays a linear scan (a live index is
/// depth); fill goes on the rule's layer at datatype 0 (a dedicated fill datatype is a
/// follow-up); the fill region is the design bounding box.
pub fn fill_library(lib: &Library, top: Option<&str>, rules: &Rules) -> Result<(Library, usize), String> {
    let top = pick_top(lib, top)?;
    let cell = flatten::flatten(lib, &top)?;

    let mut by_layer: BTreeMap<i16, Vec<Rect>> = BTreeMap::new();
    let mut all: Vec<Rect> = Vec::new();
    for e in &cell.elements {
        if let (layer, Some(r)) = elem_rect(e) {
            by_layer.entry(layer).or_default().push(r);
            all.push(r);
        }
    }
    let region = match bbox(&all) {
        Some(b) => b,
        None => return Ok((lib.clone(), 0)), // empty cell — nothing to fill
    };

    let mut new_elems: Vec<Element> = Vec::new();
    for rule in &rules.fill {
        let empty = Vec::new();
        let existing = by_layer.get(&rule.layer).unwrap_or(&empty);
        let placed = fill_region(rule, &region, existing);
        for r in &placed {
            new_elems.push(Element::Boundary { layer: rule.layer, datatype: 0, pts: r.as_boundary() });
        }
    }

    let n = new_elems.len();
    let mut out = lib.clone();
    if let Some(c) = out.cells.iter_mut().find(|c| c.name == top) {
        c.elements.extend(new_elems);
    }
    Ok((out, n))
}

/// Place fill squares for one rule over `region`, honoring clearance to `existing`.
fn fill_region(rule: &crate::rules::Fill, region: &Rect, existing: &[Rect]) -> Vec<Rect> {
    let (size, gap, win) = (rule.size as i32, rule.gap, rule.window as i32);
    let pitch = size + gap.max(0) as i32;
    let mut placed: Vec<Rect> = Vec::new();
    // Index the static existing geometry so each candidate's clearance test touches
    // only nearby shapes (rechecked with the exact `keeps_gap`) rather than all of
    // them. `placed` grows as we go, so it stays a linear scan (a live index is depth).
    let existing_idx = RegionIndex::build(existing);

    let mut wy0 = region.y0;
    while wy0 < region.y1 {
        let wy1 = (wy0 + win).min(region.y1);
        let mut wx0 = region.x0;
        while wx0 < region.x1 {
            let wx1 = (wx0 + win).min(region.x1);
            let window = Rect::new(wx0, wy0, wx1, wy1);
            let win_area = rect_area(&window);
            let target_area = rule.target_pct * win_area / 100;
            let mut covered: i64 = existing.iter().map(|s| overlap_area(s, &window)).sum();

            let mut fy = wy0;
            while fy + size <= wy1 && covered < target_area {
                let mut fx = wx0;
                while fx + size <= wx1 && covered < target_area {
                    let cand = Rect::new(fx, fy, fx + size, fy + size);
                    // halo ≥ 1 so an abutting shape (gap 0, zero-area overlap) is still
                    // a candidate; keeps_gap then makes the exact accept/reject call.
                    let clears_existing = existing_idx
                        .within(&cand, gap.max(1) as i32, None)
                        .iter()
                        .all(|&id| keeps_gap(&cand, &existing[id as usize], gap));
                    if clears_existing && placed.iter().all(|s| keeps_gap(&cand, s, gap)) {
                        covered += rect_area(&cand);
                        placed.push(cand);
                    }
                    fx += pitch;
                }
                fy += pitch;
            }
            wx0 = wx1.max(wx0 + 1);
        }
        wy0 = wy1.max(wy0 + 1);
    }
    placed
}

/// Minimal union-find for net extraction.
struct UnionFind {
    parent: Vec<usize>,
}
impl UnionFind {
    fn new(n: usize) -> UnionFind {
        UnionFind { parent: (0..n).collect() }
    }
    fn find(&mut self, mut x: usize) -> usize {
        while self.parent[x] != x {
            self.parent[x] = self.parent[self.parent[x]]; // path halving
            x = self.parent[x];
        }
        x
    }
    fn union(&mut self, a: usize, b: usize) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            self.parent[ra] = rb;
        }
    }
}

/// Antenna check: extract nets (union-find over shapes that overlap on the same
/// layer, or on a `connect`-declared layer pair), then for each net flag any
/// conductor-layer area that exceeds `max_ratio ×` the connected gate-layer area.
///
/// v0 bounds (honest): connectivity uses a `RegionIndex` for the touch/overlap
/// candidate search (rechecked exactly); the ratio is single-conductor-layer (not the
/// cumulative per-metal-layer charge model), and a net with conductor but no gate
/// is treated as not-applicable rather than flagged.
fn antenna_violations(
    all: &[(i16, Rect)],
    connect: &[(i16, i16)],
    rules: &[crate::rules::Antenna],
    out: &mut Vec<Violation>,
) {
    let n = all.len();
    let connects = |la: i16, lb: i16| -> bool {
        la == lb || connect.iter().any(|&(x, y)| (x == la && y == lb) || (x == lb && y == la))
    };
    // Connectivity only unions shapes that touch/overlap; a RegionIndex over all
    // shapes returns the touching candidates (halo of 1 dbu catches abutment too),
    // rechecked exactly — near-linear instead of all-pairs, same union result.
    let rects: Vec<Rect> = all.iter().map(|&(_, r)| r).collect();
    let idx = RegionIndex::build(&rects);
    let mut uf = UnionFind::new(n);
    for i in 0..n {
        for jd in idx.within(&all[i].1, 1, Some(i as u32)) {
            let j = jd as usize;
            if j <= i {
                continue;
            }
            let (li, ri) = all[i];
            let (lj, rj) = all[j];
            if connects(li, lj) && touch_or_overlap(&ri, &rj) {
                uf.union(i, j);
            }
        }
    }
    // group shape indices by net root
    let mut nets: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for i in 0..n {
        let root = uf.find(i);
        nets.entry(root).or_default().push(i);
    }
    for idxs in nets.values() {
        for rule in rules {
            let gate_area: i64 =
                idxs.iter().filter(|&&i| all[i].0 == rule.gate).map(|&i| rect_area(&all[i].1)).sum();
            if gate_area <= 0 {
                continue; // no gate on this net — antenna ratio is not applicable
            }
            let cond: Vec<Rect> =
                idxs.iter().filter(|&&i| all[i].0 == rule.conductor).map(|&i| all[i].1).collect();
            let cond_area: i64 = cond.iter().map(rect_area).sum();
            if cond_area <= 0 {
                continue;
            }
            let ratio = cond_area as f64 / gate_area as f64;
            if ratio > rule.max_ratio {
                let net_bbox = bbox(&cond).unwrap_or(all[idxs[0]].1);
                out.push(Violation {
                    rule: "antenna",
                    layer: rule.conductor,
                    // value / limit carried as centi-ratio (×100) — the report divides.
                    value: (ratio * 100.0).round() as i64,
                    limit: (rule.max_ratio * 100.0).round() as i64,
                    a: net_bbox,
                    b: None,
                });
            }
        }
    }
}

/// Bounding box of a set of rects.
fn bbox(shapes: &[Rect]) -> Option<Rect> {
    let first = shapes.first()?;
    let mut b = *first;
    for r in &shapes[1..] {
        b.x0 = b.x0.min(r.x0);
        b.y0 = b.y0.min(r.y0);
        b.x1 = b.x1.max(r.x1);
        b.y1 = b.y1.max(r.y1);
    }
    Some(b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::gds::{Cell, Element};

    fn rect_elem(layer: i16, x0: i32, y0: i32, x1: i32, y1: i32) -> Element {
        Element::Boundary {
            layer,
            datatype: 0,
            pts: vec![(x0, y0), (x1, y0), (x1, y1), (x0, y1), (x0, y0)],
        }
    }

    fn lib_with(elems: Vec<Element>) -> Library {
        Library { cells: vec![Cell { name: "top".into(), elements: elems }], ..Library::default() }
    }

    #[test]
    fn catches_min_width() {
        // layer 66: a 50-wide (x) × 400-tall shape; min width 100 -> violation
        let lib = lib_with(vec![rect_elem(66, 0, 0, 50, 400)]);
        let rules = Rules::parse("width 66 100\n").unwrap();
        let v = check_library(&lib, None, &rules).unwrap();
        assert_eq!(v.len(), 1);
        assert_eq!((v[0].rule, v[0].value, v[0].limit), ("width", 50, 100));
    }

    #[test]
    fn wide_enough_passes() {
        let lib = lib_with(vec![rect_elem(66, 0, 0, 200, 400)]);
        let rules = Rules::parse("width 66 100\n").unwrap();
        assert!(check_library(&lib, None, &rules).unwrap().is_empty());
    }

    #[test]
    fn catches_min_space() {
        // two met1 (layer 68) wires 60 apart in x, overlapping in y; min space 100
        let lib = lib_with(vec![rect_elem(68, 0, 0, 100, 100), rect_elem(68, 160, 0, 260, 100)]);
        let rules = Rules::parse("space 68 100\n").unwrap();
        let v = check_library(&lib, None, &rules).unwrap();
        assert_eq!(v.len(), 1);
        assert_eq!((v[0].rule, v[0].value, v[0].limit), ("space", 60, 100));
        assert!(v[0].b.is_some());
    }

    #[test]
    fn catches_min_area() {
        // a 50×50 = 2500 dbu² pad on layer 72; min area 10000 -> violation
        let lib = lib_with(vec![rect_elem(72, 0, 0, 50, 50)]);
        let rules = Rules::parse("area 72 10000\n").unwrap();
        let v = check_library(&lib, None, &rules).unwrap();
        assert_eq!(v.len(), 1);
        assert_eq!((v[0].rule, v[0].value, v[0].limit), ("area", 2500, 10000));
    }

    #[test]
    fn density_below_min_flags_the_window() {
        // two small shapes in a wide bbox -> sparse coverage. bbox 0..1000 × 0..100,
        // one 1000-window tile, covered 20000 / 100000 = 20%. min 50% -> violation.
        let lib = lib_with(vec![rect_elem(70, 0, 0, 100, 100), rect_elem(70, 900, 0, 1000, 100)]);
        let rules = Rules::parse("density 70 50 90 1000\n").unwrap();
        let v = check_library(&lib, None, &rules).unwrap();
        assert_eq!(v.len(), 1);
        assert_eq!((v[0].rule, v[0].value, v[0].limit), ("density", 20, 50));
    }

    #[test]
    fn density_above_max_flags_the_window() {
        // a solid fill: 100% coverage in its bbox; max 70% -> violation reporting 100%.
        let lib = lib_with(vec![rect_elem(71, 0, 0, 1000, 100)]);
        let rules = Rules::parse("density 71 10 70 1000\n").unwrap();
        let v = check_library(&lib, None, &rules).unwrap();
        assert_eq!(v.len(), 1);
        assert_eq!((v[0].rule, v[0].value, v[0].limit), ("density", 100, 70));
    }

    #[test]
    fn density_in_range_passes() {
        // bbox 0..1000 × 0..100; a 500-wide fill covers 50%. A degenerate far edge at
        // x=1000 only widens the bbox (zero area). 50% is within 10–90% -> clean.
        let lib = lib_with(vec![rect_elem(70, 0, 0, 500, 100), rect_elem(70, 1000, 0, 1000, 100)]);
        let rules = Rules::parse("density 70 10 90 1000\n").unwrap();
        assert!(check_library(&lib, None, &rules).unwrap().is_empty());
    }

    #[test]
    fn catches_antenna_ratio() {
        // gate poly (layer 5, area 100) connected to a big metal (layer 68, area
        // 50000) via `connect 5 68`. ratio 500 > max 100 -> antenna violation.
        let lib = lib_with(vec![
            rect_elem(5, 0, 0, 10, 10),       // gate, area 100
            rect_elem(68, 0, 0, 500, 100),    // conductor, area 50000, overlaps the gate
        ]);
        let rules = Rules::parse("connect 5 68\nantenna 68 5 100\n").unwrap();
        let v = check_library(&lib, None, &rules).unwrap();
        assert_eq!(v.len(), 1, "{v:?}");
        assert_eq!(v[0].rule, "antenna");
        assert_eq!(v[0].layer, 68);
        assert_eq!(v[0].value, 50000); // centi-ratio: 500.00
        assert_eq!(v[0].limit, 10000); // centi-ratio: 100.00
    }

    #[test]
    fn antenna_under_ratio_passes_and_no_gate_is_skipped() {
        // (a) within ratio: metal area 4000 / gate 100 = 40 < max 100 -> clean.
        let ok = lib_with(vec![rect_elem(5, 0, 0, 10, 10), rect_elem(68, 0, 0, 400, 10)]);
        let rules = Rules::parse("connect 5 68\nantenna 68 5 100\n").unwrap();
        assert!(check_library(&ok, None, &rules).unwrap().is_empty());

        // (b) conductor but no gate on the net -> antenna not applicable, no flag.
        let nogate = lib_with(vec![rect_elem(68, 0, 0, 9000, 100)]);
        assert!(check_library(&nogate, None, &rules).unwrap().is_empty());
    }

    #[test]
    fn antenna_needs_connectivity_disconnected_net_is_safe() {
        // gate and a huge metal that do NOT overlap and have no connect path ->
        // different nets -> the metal has no gate -> not flagged.
        let lib = lib_with(vec![
            rect_elem(5, 0, 0, 10, 10),            // gate near origin
            rect_elem(68, 5000, 5000, 9000, 6000), // metal far away, disjoint
        ]);
        let rules = Rules::parse("connect 5 68\nantenna 68 5 1\n").unwrap();
        assert!(check_library(&lib, None, &rules).unwrap().is_empty(), "disjoint -> no shared net");
    }

    #[test]
    fn enclosure_margin_pass_and_fail() {
        // outer 68 box 0..200, inner 66 box 50..150 -> 50 margin on every side
        let lib = lib_with(vec![rect_elem(68, 0, 0, 200, 200), rect_elem(66, 50, 50, 150, 150)]);
        assert!(check_library(&lib, None, &Rules::parse("enclosure 68 66 40\n").unwrap()).unwrap().is_empty());
        let v = check_library(&lib, None, &Rules::parse("enclosure 68 66 60\n").unwrap()).unwrap();
        assert_eq!(v.len(), 1);
        assert_eq!((v[0].rule, v[0].value, v[0].limit), ("enclosure", 50, 60));
    }

    #[test]
    fn enclosure_inner_outside_is_not_enclosed() {
        // inner sticks out past the outer -> not enclosed by any single outer shape
        let lib = lib_with(vec![rect_elem(68, 0, 0, 100, 100), rect_elem(66, 80, 80, 160, 160)]);
        let v = check_library(&lib, None, &Rules::parse("enclosure 68 66 10\n").unwrap()).unwrap();
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].value, -1, "not-enclosed sentinel");
    }

    #[test]
    fn span_via_must_match_metal_width() {
        // a metal on layer 34 running vertically: width 100 in x, 0..600 in y.
        // a cut on layer 25 that spans the full 100-wide metal edge-to-edge passes;
        // one only 60 wide (inset) fails; one protruding past the metal edge fails;
        // a cut on no metal is skipped.
        let rules = Rules::parse("span 25 34\n").unwrap();

        // exact span 0..100 in x -> clean
        let ok = lib_with(vec![rect_elem(34, 0, 0, 100, 600), rect_elem(25, 0, 250, 100, 350)]);
        assert!(check_library(&ok, None, &rules).unwrap().is_empty());

        // 60-wide cut inset (20..80) -> 20 short on each side, dev 40
        let narrow = lib_with(vec![rect_elem(34, 0, 0, 100, 600), rect_elem(25, 20, 250, 80, 350)]);
        let v = check_library(&narrow, None, &rules).unwrap();
        assert_eq!(v.len(), 1);
        assert_eq!((v[0].rule, v[0].value, v[0].limit), ("span", 40, 0));
        assert!(v[0].b.is_some());

        // cut protruding past the metal low edge (-10..100) -> dev 10
        let over = lib_with(vec![rect_elem(34, 0, 0, 100, 600), rect_elem(25, -10, 250, 100, 350)]);
        let v = check_library(&over, None, &rules).unwrap();
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].value, 10);

        // a cut with no metal under it -> skipped (not this rule's concern)
        let orphan = lib_with(vec![rect_elem(25, 5000, 5000, 5100, 5100)]);
        assert!(check_library(&orphan, None, &rules).unwrap().is_empty());
    }

    #[test]
    fn track_min_width_wires_on_centerline_grid() {
        // width=96, pitch=192, offset=48: a 96-tall horizontal wire's y-centerline must be
        // 48 + 192N. y 0..96 -> cl 48 (on grid); y 96..192 -> cl 144 (off -> flag).
        let rules = Rules::parse("track 40 96 192 48\n").unwrap();
        let t = |lib: &Library| check_library(lib, None, &rules).unwrap().into_iter().filter(|v| v.rule == "track").count();
        assert_eq!(t(&lib_with(vec![rect_elem(40, 0, 0, 500, 96)])), 0); // on-grid wire
        assert_eq!(t(&lib_with(vec![rect_elem(40, 0, 96, 500, 192)])), 1); // off-grid (cl 144)
        // off-grid wire drawn as two abutting rects -> merged -> still one
        assert_eq!(t(&lib_with(vec![rect_elem(40, 0, 96, 500, 192), rect_elem(40, 500, 96, 900, 192)])), 1);
        // a 2x-wide (192-tall) wire is not a 1x track -> not checked
        assert_eq!(t(&lib_with(vec![rect_elem(40, 0, 96, 500, 288)])), 0);
        // a min-width wire with an off-96-grid edge -> skipped (a grid rule's concern)
        assert_eq!(t(&lib_with(vec![rect_elem(40, 0, 50, 500, 146)])), 0);
    }

    #[test]
    fn grid_flags_offgrid_vertices_merged() {
        // ypitch=100, x free: a wire off-grid in y flags its two horizontal edges'
        // endpoint vertices (4 points); a wire drawn as abutting rects merges to the same 4.
        let rules = Rules::parse("grid 40 1 100\n").unwrap();
        let grid = |lib: &Library| check_library(lib, None, &rules).unwrap().into_iter().filter(|v| v.rule == "grid").count();

        // on-grid wire (y 0..100) -> clean
        assert_eq!(grid(&lib_with(vec![rect_elem(40, 0, 0, 500, 100)])), 0);
        // off-grid wire (y 50..150): y0=50, y1=150 both off the 100-grid -> 4 endpoints
        assert_eq!(grid(&lib_with(vec![rect_elem(40, 0, 50, 500, 150)])), 4);
        // same wire as two abutting rects -> collinear edges merge -> still 4, not 8
        let split = lib_with(vec![rect_elem(40, 0, 50, 500, 150), rect_elem(40, 500, 50, 1000, 150)]);
        assert_eq!(grid(&split), 4);
    }

    #[test]
    fn venc_enclosure_satisfied_on_one_axis() {
        // major=20, minor=8: a via is enclosed adequately if, on one axis, both sides
        // are >=8 and at least one is >=20.
        let rules = Rules::parse("venc 30 25 20 8\n").unwrap();

        // via fills the wire height (y margins 0) but is deep inside along x -> the
        // x-axis qualifies (100 & 1828), so it passes despite the 0 y-margins.
        let ok = lib_with(vec![rect_elem(30, 0, 0, 2000, 136), rect_elem(25, 100, 0, 172, 136)]);
        assert!(check_library(&ok, None, &rules).unwrap().is_empty());

        // via inset only 4 dbu on every side of a small pad -> neither axis meets the
        // minor on both sides -> violation, enclosed (value 0, not -1).
        let bad = lib_with(vec![rect_elem(30, 0, 0, 100, 100), rect_elem(25, 4, 4, 96, 96)]);
        let v = check_library(&bad, None, &rules).unwrap();
        assert_eq!(v.len(), 1);
        assert_eq!((v[0].rule, v[0].layer, v[0].limit, v[0].value), ("venc", 25, 20, 0));

        // via not inside any single outer shape -> not enclosed sentinel (-1)
        let orphan = lib_with(vec![rect_elem(30, 0, 0, 100, 100), rect_elem(25, 5000, 5000, 5100, 5100)]);
        let v = check_library(&orphan, None, &rules).unwrap();
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].value, -1);
    }

    #[test]
    fn corner_via_must_match_metal_width() {
        // metal wire 100×20; a via spanning the full height sits on the top and bottom
        // metal boundary — no corner departs the metal on both of its edges. Passes.
        let ok = lib_with(vec![rect_elem(19, 0, 0, 100, 20), rect_elem(18, 40, 0, 60, 20)]);
        assert!(check_library(&ok, None, &Rules::parse("corner 19 18\n").unwrap()).unwrap().is_empty());
        // a via floating inside the metal (reaching no edge) — every corner departs the
        // merged boundary on both edges. Flagged.
        let bad = lib_with(vec![rect_elem(19, 0, 0, 100, 20), rect_elem(18, 40, 5, 60, 15)]);
        let v = check_library(&bad, None, &Rules::parse("corner 19 18\n").unwrap()).unwrap();
        assert_eq!(v.len(), 1);
        assert_eq!((v[0].rule, v[0].layer), ("corner", 18));
        // a via drawn twice (coincident) is de-duplicated → still one violation
        let dup = lib_with(vec![
            rect_elem(19, 0, 0, 100, 20),
            rect_elem(18, 40, 5, 60, 15),
            rect_elem(18, 40, 5, 60, 15),
        ]);
        assert_eq!(check_library(&dup, None, &Rules::parse("corner 19 18\n").unwrap()).unwrap().len(), 1);
    }

    #[test]
    fn sep_flags_facing_gap_and_dedups() {
        // two squares 4 dbu apart on layer 19; same-class (self-)spacing within dist 5
        // finds the single facing pair (A's right edge vs B's left edge), deduped.
        let lib = lib_with(vec![rect_elem(19, 0, 0, 10, 10), rect_elem(19, 14, 0, 24, 10)]);
        let v = check_library(&lib, None, &Rules::parse("sep 19 1 20 1 20 5\n").unwrap()).unwrap();
        assert_eq!(v.len(), 1);
        assert_eq!((v[0].rule, v[0].value, v[0].limit), ("sep", 4, 5));
        // gap == dist is not a violation
        assert!(check_library(&lib, None, &Rules::parse("sep 19 1 20 1 20 4\n").unwrap()).unwrap().is_empty());
    }

    #[test]
    fn c2c_flags_diagonal_corner_gap() {
        // two squares whose nearest corners are sqrt(32)≈5.66 dbu apart, diagonally
        let lib = lib_with(vec![rect_elem(19, 0, 0, 10, 10), rect_elem(19, 14, 14, 24, 24)]);
        // dist 6 catches it — one horizontal-edge pair and one vertical-edge pair
        let v = check_library(&lib, None, &Rules::parse("c2c 19 6\n").unwrap()).unwrap();
        assert_eq!(v.len(), 2);
        assert!(v.iter().all(|x| x.rule == "c2c"));
        // dist 5 < 5.66: clean
        assert!(check_library(&lib, None, &Rules::parse("c2c 19 5\n").unwrap()).unwrap().is_empty());
    }

    #[test]
    fn fill_raises_coverage_keeping_clearance() {
        // a die boundary (layer 100, 1000×1000) sets the fill region; one sparse
        // layer-68 shape near the origin. fill layer 68 to 10%, 50-square, 50 gap.
        let lib = lib_with(vec![rect_elem(100, 0, 0, 1000, 1000), rect_elem(68, 0, 0, 100, 100)]);
        let rules = Rules::parse("fill 68 10 1000 50 50\n").unwrap();
        let (filled, n) = fill_library(&lib, None, &rules).unwrap();
        assert!(n > 0, "should place fill shapes");
        assert_eq!(filled.cells[0].elements.len(), lib.cells[0].elements.len() + n);

        let die = Rect::new(0, 0, 1000, 1000);
        let cov = |l: &Library| -> i64 {
            flatten::flatten(l, "top")
                .unwrap()
                .elements
                .iter()
                .filter_map(|e| if elem_rect(e).0 == 68 { elem_rect(e).1 } else { None })
                .map(|r| overlap_area(&r, &die))
                .sum()
        };
        assert!(cov(&filled) > cov(&lib), "fill increases layer-68 coverage");

        // every fill shape clears the original metal by the gap
        let orig = Rect::new(0, 0, 100, 100);
        for e in &filled.cells[0].elements {
            if let (68, Some(r)) = elem_rect(e) {
                if r != orig {
                    assert!(keeps_gap(&r, &orig, 50), "fill {r:?} too close to existing");
                }
            }
        }
    }

    #[test]
    fn indexed_spacing_matches_brute_force_at_scale() {
        // A deterministic field of shapes with varied gaps; the RegionIndex-backed
        // spacing check must find exactly the same violations as an all-pairs scan.
        let min_s = 50i64;
        let mut elems = Vec::new();
        let mut shapes = Vec::new();
        for gy in 0..24i32 {
            for gx in 0..24i32 {
                // pitch jitters so some neighbours are < min_s apart and some aren't
                let x = gx * 80 + (gx * 13 + gy * 5) % 40;
                let y = gy * 80 + (gy * 11 + gx * 7) % 40;
                let r = Rect::new(x, y, x + 50, y + 50);
                shapes.push(r);
                elems.push(rect_elem(68, r.x0, r.y0, r.x1, r.y1));
            }
        }
        let lib = lib_with(elems);
        let rules = Rules::parse(&format!("space 68 {min_s}\n")).unwrap();

        // indexed result (rule=="space" pairs → comparable tuples)
        let key = |r: &Rect| (r.x0, r.y0, r.x1, r.y1);
        let mut got: Vec<_> = check_library(&lib, None, &rules)
            .unwrap()
            .into_iter()
            .filter(|v| v.rule == "space")
            .map(|v| (key(&v.a), key(&v.b.unwrap()), v.value))
            .collect();

        // brute-force reference (all-pairs)
        let mut want = Vec::new();
        for i in 0..shapes.len() {
            for j in (i + 1)..shapes.len() {
                if let Some(s) = spacing(&shapes[i], &shapes[j]) {
                    if s < min_s {
                        want.push((key(&shapes[i]), key(&shapes[j]), s));
                    }
                }
            }
        }
        got.sort();
        want.sort();
        assert_eq!(got, want, "indexed spacing must equal all-pairs");
        assert!(!want.is_empty(), "the fixture should actually contain violations");
    }

    #[test]
    fn far_enough_and_touching_pass() {
        // 200 apart (> min 100) -> ok; and a touching pair -> connected, not a space viol
        let lib = lib_with(vec![
            rect_elem(68, 0, 0, 100, 100),
            rect_elem(68, 300, 0, 400, 100), // gap 200
            rect_elem(68, 100, 0, 200, 100), // abuts the first -> merged/connected
        ]);
        let rules = Rules::parse("space 68 100\n").unwrap();
        assert!(check_library(&lib, None, &rules).unwrap().is_empty());
    }
}
