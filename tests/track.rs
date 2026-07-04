//! Integration test — routing-track centerline check.
//!
//! On advanced nodes, minimum-width ("1x") wires must be centered on the routing-track
//! grid — their centerline, on the width axis, at a fixed pitch and offset. Wider wires
//! aren't 1x tracks and aren't checked; a min-width wire drawn as many abutting rects is
//! one wire. Generic fixture as a standing regression for the `track` rule.

use vyges_drc::drc::check_library;
use vyges_drc::layout::gds::{Cell, Element, Library};
use vyges_drc::rules::Rules;

fn rect(layer: i16, x0: i32, y0: i32, x1: i32, y1: i32) -> Element {
    Element::Boundary { layer, datatype: 0, pts: vec![(x0, y0), (x1, y0), (x1, y1), (x0, y1), (x0, y0)] }
}

#[test]
fn min_width_wires_must_be_on_track_grid() {
    // 96-wide (1x) wires on layer 40, track pitch 192, offset 48: the y-centerline must be
    // 48 + 192N. Rows at y 0..96 (cl 48) and 192..288 (cl 240) are on-grid; 96..192 (cl 144)
    // is off-grid. A 2x-wide wire is exempt.
    let rules = Rules::parse("track 40 96 192 48\n").unwrap();
    let elements = vec![
        rect(40, 0, 0, 3000, 96), // on-grid (cl 48)
        rect(40, 0, 192, 3000, 288), // on-grid (cl 240)
        // off-grid wire (cl 144), drawn as three abutting rects -> one wire
        rect(40, 0, 96, 1000, 192),
        rect(40, 1000, 96, 2000, 192),
        rect(40, 2000, 96, 3000, 192),
        // a separate off-grid wire elsewhere (cl 528 = 96*5+48? -> off) -> its own violation
        rect(40, 0, 480, 800, 576), // cl 528; (528-48)%192 = 480%192 = 96 -> off-grid
        // a 2x-wide wire straddling an off-grid centerline -> not a 1x track -> exempt
        rect(40, 0, 672, 3000, 864),
    ];
    let lib = Library { cells: vec![Cell { name: "top".into(), elements }], ..Library::default() };

    let track: Vec<_> =
        check_library(&lib, None, &rules).unwrap().into_iter().filter(|v| v.rule == "track").collect();

    // the merged 3-rect off-grid wire counts once + the separate off-grid wire = 2;
    // the two on-grid wires and the 2x wire don't flag. Without the merge it would be 4.
    assert_eq!(track.len(), 2, "off-grid min-width wires, merged per wire: {track:?}");
}
