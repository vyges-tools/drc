//! Datatype isolation: rules target a `(layer, datatype)` pair, so shapes on the same
//! GDS layer number but a different datatype are never unioned or measured together.
//!
//! This guards the sky130 failure mode where drawn poly (`66/20`) and the licon1
//! contact (`66/44`) share layer number 66: a datatype-blind check unions them and the
//! contact-to-poly gaps read as thousands of phantom poly-spacing violations.

use vyges_drc::drc::check_library;
use vyges_drc::layout::gds::{Cell, Element, Library};
use vyges_drc::rules::Rules;

fn boundary(layer: i16, datatype: i16, x0: i32, y0: i32, x1: i32, y1: i32) -> Element {
    Element::Boundary { layer, datatype, pts: vec![(x0, y0), (x1, y0), (x1, y1), (x0, y1), (x0, y0)] }
}

fn lib(elements: Vec<Element>) -> Library {
    Library { cells: vec![Cell { name: "top".into(), elements }], ..Library::default() }
}

#[test]
fn space_rule_ignores_a_different_datatype_on_the_same_layer() {
    // Two wide, clean poly wires (66/20) 300 apart — no space violation at min 210.
    // Between them sit two small 66/44 "contacts" 50 apart. A datatype-blind union of
    // layer 66 would report that 50 gap (and the contact-to-poly gaps) as poly spacing.
    let elements = vec![
        boundary(66, 20, 0, 0, 100, 1000),   // poly wire A
        boundary(66, 20, 400, 0, 500, 1000), // poly wire B (gap 300 > 210: clean)
        boundary(66, 44, 180, 400, 220, 440), // contact 1
        boundary(66, 44, 270, 400, 310, 440), // contact 2 (gap 50 to contact 1)
    ];
    let l = lib(elements);

    // A poly-space rule on 66/20 sees only the two clean wires → no violation.
    let r = Rules::parse("space 66/20 210\n").unwrap();
    let v = check_library(&l, Some("top"), &r).unwrap();
    assert!(v.is_empty(), "66/20 spacing must not see the 66/44 contacts: {v:?}");

    // The same rule targeting 66/44 does see the contacts (closest gap 50) → violations.
    let r44 = Rules::parse("space 66/44 210\n").unwrap();
    let v44 = check_library(&l, Some("top"), &r44).unwrap();
    assert!(v44.iter().all(|v| v.rule == "space" && v.layer == 66));
    assert!(v44.iter().any(|v| v.value == 50), "66/44 spacing sees the 50-gap contact pair: {v44:?}");

    // A bare layer number means datatype 0 — which has no shapes here, so nothing.
    let r0 = Rules::parse("space 66 210\n").unwrap();
    assert!(check_library(&l, Some("top"), &r0).unwrap().is_empty(), "bare 66 == 66/0, empty");
}

#[test]
fn width_rule_is_datatype_scoped() {
    // A 40-wide contact on 66/44 and a 100-wide poly on 66/20. A width rule of 80 on
    // 66/20 must not flag the narrow 66/44 contact (only the poly, which is >= 80: clean).
    let l = lib(vec![
        boundary(66, 20, 0, 0, 100, 1000), // 100 wide poly — ok at width 80
        boundary(66, 44, 200, 200, 240, 240), // 40 wide contact — would fail width 80
    ]);
    let r = Rules::parse("width 66/20 80\n").unwrap();
    assert!(check_library(&l, Some("top"), &r).unwrap().is_empty(), "width 66/20 ignores the 66/44 contact");
    // Targeting the contact's datatype flags it (a 40-wide square trips width 80).
    let r44 = Rules::parse("width 66/44 80\n").unwrap();
    assert!(!check_library(&l, Some("top"), &r44).unwrap().is_empty());
}
