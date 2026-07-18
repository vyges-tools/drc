//! Integration test — the "via must span its wire's full width" scenario.
//!
//! A realistic advanced-node routing case: a wire run carrying several vias, where a
//! via must land on the **full width** of the wire it connects to (its edges coincident
//! with the wire edges, perpendicular to the wire's run). A via that is inset (narrower
//! than the wire) or shifted is a design-rule violation. This is the generic shape of a
//! dominant rule family in real block-level DRC decks — here a routing wire with a mix
//! of full-width and inset vias, drawn to realistic dimensions, as a standing regression
//! for the `span` rule against non-trivial geometry.

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
fn vias_must_span_full_wire_width() {
    // Layer 30 — a horizontal wire, 2000 long × 136 wide (its width runs in y).
    //   layer-25 vias: one spans the full 136-wide wire (ok); two are 72-tall, inset
    //   32 dbu each side (fail, 64 dbu short).
    // Layer 50 — a vertical wire, 480 wide × 3000 long (its width runs in x).
    //   layer-45 vias: one spans the full 480-wide wire (ok); one is 96-wide, inset
    //   192 dbu each side (fail, 384 dbu short).
    let elements = vec![
        rect(30, 0, 0, 2000, 136),
        rect(25, 100, 0, 172, 136),  // spans full width -> ok
        rect(25, 300, 32, 372, 104), // inset -> dev 64
        rect(25, 500, 32, 572, 104), // inset -> dev 64
        rect(50, 5000, 0, 5480, 3000),
        rect(45, 5000, 100, 5480, 196), // spans full width -> ok
        rect(45, 5192, 400, 5288, 496), // inset -> dev 384
    ];
    let lib = Library {
        cells: vec![Cell {
            name: "top".into(),
            elements,
        }],
        ..Library::default()
    };
    let rules = Rules::parse("span 25 30\nspan 45 50\n").unwrap();

    let viols = check_library(&lib, None, &rules).unwrap();
    let span: Vec<_> = viols.into_iter().filter(|v| v.rule == "span").collect();

    // exactly the three inset vias flag; the two full-width vias do not
    assert_eq!(span.len(), 3, "only inset vias flag: {span:?}");
    let on = |layer: i16| span.iter().filter(|v| v.layer == layer).count();
    assert_eq!(on(25), 2, "two inset vias on the horizontal wire");
    assert_eq!(on(45), 1, "one inset via on the vertical wire");

    // `value` is the total edge shortfall from spanning the wire width
    assert!(
        span.iter().filter(|v| v.layer == 25).all(|v| v.value == 64),
        "each 72-in-136 inset via is 64 dbu short"
    );
    assert_eq!(span.iter().find(|v| v.layer == 45).unwrap().value, 384);
}
