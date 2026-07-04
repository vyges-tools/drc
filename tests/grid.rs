//! Integration test — manufacturing-grid (routing-track) check.
//!
//! On advanced nodes a metal layer's edges must land on a fixed grid (a routing-track
//! pitch on one axis, free on the other). A wire whose edge is off that grid is flagged.
//! Because a long wire is often drawn as many abutting rectangles, collinear off-grid
//! edges are merged first, so one off-grid wire counts once (not once per rect). Generic
//! fixture as a standing regression for the `grid` rule.

use vyges_drc::drc::check_library;
use vyges_drc::layout::gds::{Cell, Element, Library};
use vyges_drc::rules::Rules;

fn rect(layer: i16, x0: i32, y0: i32, x1: i32, y1: i32) -> Element {
    Element::Boundary { layer, datatype: 0, pts: vec![(x0, y0), (x1, y0), (x1, y1), (x0, y1), (x0, y0)] }
}

#[test]
fn offgrid_edges_flag_once_per_merged_wire() {
    // Layer 40 vertices must sit on a 100-dbu y-grid (x is free): a horizontal wire is
    // fine at y 0..100, off-grid at y 50..150 (both edges off the grid).
    let rules = Rules::parse("grid 40 1 100\n").unwrap();
    let elements = vec![
        rect(40, 0, 0, 3000, 100), // on-grid wire -> no violations
        // an off-grid wire (y 50..150) drawn as three abutting rects -> merges to one wire,
        // so its two horizontal edges contribute 4 endpoint vertices, not 12.
        rect(40, 0, 50, 1000, 150),
        rect(40, 1000, 50, 2000, 150),
        rect(40, 2000, 50, 3000, 150),
        // a separate off-grid wire elsewhere -> its own 4 endpoints
        rect(40, 0, 350, 800, 450),
    ];
    let lib = Library { cells: vec![Cell { name: "top".into(), elements }], ..Library::default() };

    let grid: Vec<_> =
        check_library(&lib, None, &rules).unwrap().into_iter().filter(|v| v.rule == "grid").collect();

    // 4 endpoints for the merged 3-rect wire + 4 for the separate wire = 8; the on-grid
    // wire contributes none. If the merge were missing, the split wire would report 12.
    assert_eq!(grid.len(), 8, "off-grid vertices, merged per wire: {grid:?}");
}
