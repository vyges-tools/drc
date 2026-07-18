//! Integration test — asymmetric via enclosure ("line-end / side").
//!
//! An advanced-node via-enclosure case: a via lands on a routing wire and must be
//! enclosed by the metal by a large margin along the routing direction and a small one
//! across it — but only on **one** axis (a via that fills the wire width still passes on
//! the length axis). A via inset on both axes, or off the wire entirely, violates.
//! Generic fixture, realistic dimensions, as a standing regression for the `venc` rule.

use vyges_drc::drc::check_library;
use vyges_drc::layout::gds::{Cell, Element, Library};
use vyges_drc::rules::Rules;

fn rect(layer: i16, x0: i32, y0: i32, x1: i32, y1: i32) -> Element {
    Element::Boundary {
        layer,
        datatype: 0,
        pts: vec![(x0, y0), (x1, y0), (x1, y1), (x0, y1), (x0, y0)],
    }
}

#[test]
fn via_enclosure_satisfied_on_one_axis() {
    // Vias (layer 25) on wires (layer 30) must be enclosed by >=20 on one axis, with
    // both that axis's sides >=8. major=20, minor=8.
    let rules = Rules::parse("venc 30 25 20 8\n").unwrap();
    let elements = vec![
        rect(30, 0, 0, 2000, 80),         // a long horizontal wire
        rect(25, 200, 0, 260, 80), // fills the wire height (y margins 0) but deep in x -> ok (x axis)
        rect(25, 900, 8, 960, 72), // y margins 8 & 8 (< major) but deep in x -> ok (x axis)
        rect(25, 1500, 30, 1560, 50), // enclosed on both axes -> ok
        rect(30, 5000, 0, 5080, 80), // a short square stub
        rect(25, 5004, 4, 5076, 76), // inset 4 on every side -> neither axis qualifies -> FAIL (enclosed)
        rect(25, 9000, 9000, 9060, 9040), // off any wire -> FAIL (not enclosed)
    ];
    let lib = Library {
        cells: vec![Cell {
            name: "top".into(),
            elements,
        }],
        ..Library::default()
    };

    let viols = check_library(&lib, None, &rules).unwrap();
    let venc: Vec<_> = viols.into_iter().filter(|v| v.rule == "venc").collect();

    // only the stub-inset via and the off-wire via flag; the three well-enclosed vias pass
    assert_eq!(
        venc.len(),
        2,
        "only inadequately-enclosed vias flag: {venc:?}"
    );
    assert!(
        venc.iter().any(|v| v.value == -1),
        "the off-wire via is not enclosed (-1)"
    );
    assert!(
        venc.iter().any(|v| v.value >= 0 && v.limit == 20),
        "the stub-inset via is enclosed but under the required major margin"
    );
}
