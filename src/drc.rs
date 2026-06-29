//! The check engine — flatten the GDS, collect per-layer shapes, run the rules.
//!
//! v0 measures, per layer:
//! - **width**: a shape whose smaller dimension is below the layer minimum;
//! - **spacing**: two distinct shapes on the same layer closer than the minimum;
//! - **area**: a polygon below the layer's minimum area (dbu²);
//! - **density**: windowed metal coverage outside a `min..max` percent band.
//!
//! Honest bounds (depth reserved): shapes are taken per input `Boundary` and **not
//! pre-merged** (a wide wire drawn as abutting rectangles, or two same-net touching
//! shapes, are treated as drawn) — proper DRC unions same-layer geometry first
//! (a `vyges-layout` boolean OR) and measures the resulting polygons; non-Manhattan
//! polygons fall back to their bounding box; spacing is brute-force all-pairs per
//! layer (a spatial index is the scaling pass). Touching/overlapping shapes are
//! treated as connected (not a spacing violation).

use std::collections::BTreeMap;

use crate::layout::flatten;
use crate::layout::geom::{self, Rect};
use crate::layout::gds::{Element, Library};
use crate::rules::Rules;

#[derive(Debug, Clone)]
pub struct Violation {
    pub rule: &'static str, // "width" | "space" | "area" | "density"
    pub layer: i16,
    /// The bound that was violated. DB units for width/space, DB units² for area,
    /// **percent** for density (the min or max coverage bound).
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

/// Run the rule deck over the flattened top cell of `lib`.
pub fn check_library(
    lib: &Library,
    top: Option<&str>,
    rules: &Rules,
) -> Result<Vec<Violation>, String> {
    let top = match top {
        Some(t) => t.to_string(),
        None if lib.cells.len() == 1 => lib.cells[0].name.clone(),
        None => {
            return Err(format!(
                "{} cells; pass a top cell ({})",
                lib.cells.len(),
                lib.cells.iter().map(|c| c.name.as_str()).collect::<Vec<_>>().join(", ")
            ))
        }
    };
    let cell = flatten::flatten(lib, &top)?;

    // shapes per layer (rectangles where possible; bbox for non-Manhattan)
    let mut by_layer: BTreeMap<i16, Vec<Rect>> = BTreeMap::new();
    for e in &cell.elements {
        let (layer, rect) = match e {
            Element::Boundary { layer, pts, .. } => {
                (*layer, Rect::from_boundary(pts).or_else(|| geom::bbox(pts)))
            }
            Element::Box { layer, pts, .. } => {
                (*layer, Rect::from_boundary(pts).or_else(|| geom::bbox(pts)))
            }
            _ => continue, // Path/Text not measured in v0
        };
        if let Some(r) = rect {
            by_layer.entry(layer).or_default().push(r);
        }
    }

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
            for i in 0..shapes.len() {
                for j in (i + 1)..shapes.len() {
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
