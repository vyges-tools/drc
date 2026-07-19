//! Render ranked violation views — the part an agent cannot get from a text report.
//!
//! A violation is a measurement plus a coordinate. Neither tells you *what went wrong*: that
//! this via is unlanded, that these two wires converge at a corner, that a fill shape is
//! stranded. A picture does, and a picture of the worst occurrence of the biggest cluster
//! does it for the whole cluster.
//!
//! # These are diagnostics, not sign-off
//!
//! Every view carries [`DISCLAIMER`], and it is not boilerplate. A ranked, hashed, rendered
//! image reads as *more* authoritative than a line of text, precisely because it can be
//! looked at. Ours come from a 14-rule geometric deck at `maturity: structured` — not a
//! foundry deck, not a sign-off. The prior art has the same disclaimer for the opposite
//! reason (it renders a report it did not compute); ours has to be louder, not quieter,
//! because the image and the finding come from the same computation and so look conclusive.
//!
//! # Bounds
//!
//! Rendering is reachable from any GDS anyone hands us, so every dimension is capped. A
//! pathological input should produce a bounded, honest artifact set or none — never an
//! unbounded write.

use crate::cluster::Cluster;
use crate::layout::gds::Cell;
use crate::layout::geom::Rect;

/// What every emitted view means, and does not — the fixed part.
///
/// See [`provenance`] for the run-specific version, which names the deck that produced the
/// findings and the layers it did not examine. A constant alone cannot say the second thing,
/// and the second thing is the one that changes what the reader should conclude.
pub const DISCLAIMER: &str = "Representative diagnostic evidence from the Vyges geometric \
    rule deck. Each view shows the worst occurrence of one rule/layer cluster, not every \
    occurrence. NOT foundry sign-off, and not a substitute for the native report or the \
    foundry rule deck.";

/// The disclaimer for one specific run: what deck produced these findings, and what it did
/// not look at.
///
/// A deck's identity matters because "checked" is meaningless without "checked against
/// what". `unchecked` matters more: geometry on a layer no rule mentions was not found
/// clean, it was not examined, and a reader who is not told that will assume otherwise. The
/// warning is deliberately blunt, and deliberately placed in the same string as the verdict
/// so it cannot be separated from it in a quote or a screenshot.
pub fn provenance(rules: &crate::rules::Rules, unchecked: &[crate::rules::Layer]) -> String {
    let (families, total) = rules.summary();
    // Deliberately does NOT include DISCLAIMER. That sentence describes the rendered views,
    // and this string is emitted whether or not any were rendered -- leading a view-less run
    // with "each view shows..." describes pictures that do not exist. The caller joins the
    // two when there are views to disclaim.
    let mut s = format!(
        "Deck: vyges-drc {} — {total} rule(s) across {} family(ies) [{}].",
        crate::VERSION,
        families.len(),
        families.join(", ")
    );
    if !unchecked.is_empty() {
        let names: Vec<String> = unchecked
            .iter()
            .take(12)
            .map(|l| format!("{}/{}", l.num, l.dt))
            .collect();
        let more = unchecked.len().saturating_sub(names.len());
        s.push_str(&format!(
            " PARTIAL COVERAGE: {} layer(s) carry geometry that NO rule in this deck examines \
             [{}{}] — those layers were not found clean, they were not checked.",
            unchecked.len(),
            names.join(", "),
            if more > 0 {
                format!(", +{more} more")
            } else {
                String::new()
            }
        ));
    }
    s
}

/// At most this many cluster views. Past a dozen the set stops being a summary and becomes a
/// pile nobody opens; the ranking is there so the ones that are dropped are the ones that
/// matter least. What was dropped is always reported — a silent cap reads as "that was
/// everything".
pub const MAX_VIEWS: usize = 12;

/// Longest side of a rendered view, in pixels.
pub const MAX_DIM: u32 = 900;

/// Smallest context window, in db units. A violation marker can be a point or a hairline, and
/// framing it exactly would render a few pixels of solid colour that show nothing about the
/// surroundings — which is the entire reason for rendering.
pub const MIN_CONTEXT: i32 = 200;

// NOT CARRIED: the verdict a view is evidence for.
//
// A rendered view looks exactly as confident whether `engineering.status` came out `pass` or
// `unknown`. Since #72 an engine can report incomplete coverage and have a held assertion
// downgraded, so the state now exists to inherit -- a view attached to an `unknown` verdict
// should not read as proof of anything.
//
// Deliberately not guessed at: how to show it (a caption? a border? a sibling JSON field?)
// depends on how these get consumed, and nothing consumes them yet. Revisit when a caller
// renders these into a report or an agent starts reasoning from them, and let that consumer
// say what it needs. See vyges-tools-internal#72.

/// One rendered view and what it depicts.
#[derive(Debug, Clone, PartialEq)]
pub struct View {
    pub path: String,
    /// Ranking position, 1-based; `0` for the whole-layout overview.
    pub rank: usize,
    pub rule: &'static str,
    pub layer: i16,
    /// Occurrences in the cluster this view represents.
    pub count: usize,
    /// The db-unit region framed.
    pub window: Rect,
}

/// Pad `r` so the violation has visible surroundings.
///
/// Padding is proportional to the region so a large marker keeps its context, with an
/// absolute floor so a zero-area or hairline marker still gets a real window. Both are
/// needed: proportional alone collapses on a point, absolute alone gives a huge marker no
/// context at all.
pub fn pad(r: Rect) -> Rect {
    // Saturating throughout: the extent of a marker spanning most of the coordinate space
    // overflows a plain subtraction, and a wrapped width yields a window on the far side of
    // the origin — a confidently rendered picture of the wrong place.
    let w = r.x1.saturating_sub(r.x0).max(0);
    let h = r.y1.saturating_sub(r.y0).max(0);
    let px = (w / 2).max(MIN_CONTEXT);
    let py = (h / 2).max(MIN_CONTEXT);
    Rect {
        x0: r.x0.saturating_sub(px),
        y0: r.y0.saturating_sub(py),
        x1: r.x1.saturating_add(px),
        y1: r.y1.saturating_add(py),
    }
}

/// A filename that encodes the ranking, so the set sorts the way it ranks and the same input
/// always writes the same names — which is what makes the hashes comparable between runs.
fn file_name(rank: usize, c: &Cluster) -> String {
    format!("drc-view-{rank:02}-{}-layer{}.png", c.rule, c.layer)
}

/// Render the top clusters plus a whole-layout overview into `dir`.
///
/// Returns the views written, in ranking order, and the number of clusters that did not fit
/// under [`MAX_VIEWS`] — the caller must report that rather than let a truncated set read as
/// the complete one.
pub fn render(
    cell: &Cell,
    clusters: &[Cluster],
    dir: &std::path::Path,
) -> Result<(Vec<View>, usize), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut out = Vec::new();

    // The overview first: it is the only view that shows where the clusters sit relative to
    // one another, and it is rank 0 because it is not part of the ranking.
    let overview = dir.join("drc-view-00-overview.png");
    let png = vyges_gds_view::png::render_png(cell, None, MAX_DIM);
    std::fs::write(&overview, &png).map_err(|e| format!("{}: {e}", overview.display()))?;
    out.push(View {
        path: overview.to_string_lossy().into_owned(),
        rank: 0,
        rule: "overview",
        layer: -1,
        count: clusters.iter().map(|c| c.count).sum(),
        window: vyges_gds_view::png::drawn_bbox(cell, None).unwrap_or(Rect {
            x0: 0,
            y0: 0,
            x1: 1,
            y1: 1,
        }),
    });

    for (i, c) in clusters.iter().take(MAX_VIEWS).enumerate() {
        let rank = i + 1;
        let window = pad(c.worst);
        let path = dir.join(file_name(rank, c));
        let png = vyges_gds_view::png::render_png_window(cell, None, MAX_DIM, window);
        std::fs::write(&path, &png).map_err(|e| format!("{}: {e}", path.display()))?;
        out.push(View {
            path: path.to_string_lossy().into_owned(),
            rank,
            rule: c.rule,
            layer: c.layer,
            count: c.count,
            window,
        });
    }
    Ok((out, clusters.len().saturating_sub(MAX_VIEWS)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::gds::Element;

    fn cell() -> Cell {
        Cell {
            elements: vec![
                Element::Boundary {
                    layer: 66,
                    datatype: 0,
                    pts: Rect::new(0, 0, 400, 400).as_boundary(),
                },
                Element::Boundary {
                    layer: 68,
                    datatype: 0,
                    pts: Rect::new(600, 600, 900, 900).as_boundary(),
                },
            ],
            ..Default::default()
        }
    }

    fn c(rule: &'static str, layer: i16, count: usize, worst: Rect) -> Cluster {
        Cluster {
            rule,
            layer,
            count,
            distinct_values: 1,
            limit: 100,
            value: 50,
            severity: 0.5,
            worst,
            extent: worst,
        }
    }

    fn tmp(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("vyges-drc-views-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    /// A zero-area marker must still get a window with surroundings in it, or the view shows
    /// a few pixels of solid colour and answers nothing.
    #[test]
    fn a_degenerate_marker_gets_a_real_window() {
        let p = pad(Rect::new(500, 500, 500, 500));
        assert!(
            p.x1 - p.x0 >= 2 * MIN_CONTEXT && p.y1 - p.y0 >= 2 * MIN_CONTEXT,
            "a point marker should still be framed with context, got {p:?}"
        );
    }

    /// A large marker keeps proportional context rather than being framed tightly.
    #[test]
    fn padding_scales_with_the_marker() {
        let small = pad(Rect::new(0, 0, 10, 10));
        let large = pad(Rect::new(0, 0, 10_000, 10_000));
        assert!(
            (large.x1 - large.x0) > (small.x1 - small.x0),
            "a bigger marker should get a bigger window"
        );
        // and the small one is floored by MIN_CONTEXT rather than being 5 db units wide
        assert!(small.x1 - small.x0 >= 2 * MIN_CONTEXT);
    }

    /// Padding near i32's edge must not wrap into a window on the far side of the coordinate
    /// space, which is the sort of thing that produces a confidently rendered wrong place.
    #[test]
    fn padding_saturates_rather_than_wrapping() {
        let p = pad(Rect::new(
            i32::MIN + 1,
            i32::MIN + 1,
            i32::MAX - 1,
            i32::MAX - 1,
        ));
        assert!(
            p.x0 <= p.x1 && p.y0 <= p.y1,
            "window inverted by overflow: {p:?}"
        );
    }

    #[test]
    fn views_are_written_in_ranking_order_with_an_overview_first() {
        let dir = tmp("order");
        let cs = vec![
            c("space", 68, 40, Rect::new(600, 600, 700, 700)),
            c("width", 66, 5, Rect::new(0, 0, 100, 100)),
        ];
        let (views, dropped) = render(&cell(), &cs, &dir).expect("render");
        assert_eq!(dropped, 0);
        assert_eq!(views.len(), 3, "an overview plus one view per cluster");
        assert_eq!((views[0].rank, views[0].rule), (0, "overview"));
        assert_eq!(
            (views[1].rank, views[1].rule, views[1].count),
            (1, "space", 40)
        );
        assert_eq!(
            (views[2].rank, views[2].rule, views[2].count),
            (2, "width", 5)
        );
        for v in &views {
            let b = std::fs::read(&v.path).expect("view file exists");
            assert_eq!(
                &b[..8],
                &[137, 80, 78, 71, 13, 10, 26, 10],
                "{} is not a PNG",
                v.path
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The cap is real, and what it dropped is returned so the caller can say so. A silent
    /// truncation reads as "that was everything", which is the failure this whole area of the
    /// codebase keeps running into.
    #[test]
    fn the_view_cap_is_enforced_and_the_remainder_reported() {
        let dir = tmp("cap");
        let cs: Vec<Cluster> = (0..MAX_VIEWS + 5)
            .map(|i| c("space", i as i16, 10, Rect::new(0, 0, 100, 100)))
            .collect();
        let (views, dropped) = render(&cell(), &cs, &dir).expect("render");
        assert_eq!(views.len(), MAX_VIEWS + 1, "capped, plus the overview");
        assert_eq!(
            dropped, 5,
            "the caller must be able to report what was dropped"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Same input, same filenames — otherwise the hashes are not comparable between runs and
    /// the evidence cannot be diffed.
    #[test]
    fn file_names_are_stable_and_encode_the_ranking() {
        let k = c("space", 68, 40, Rect::new(0, 0, 10, 10));
        assert_eq!(file_name(1, &k), "drc-view-01-space-layer68.png");
        assert_eq!(file_name(12, &k), "drc-view-12-space-layer68.png");
        // zero-padded so the set sorts in ranking order in any file listing
        assert!(file_name(2, &k) < file_name(10, &k));
    }

    #[test]
    fn rendering_no_clusters_still_yields_the_overview() {
        let dir = tmp("empty");
        let (views, dropped) = render(&cell(), &[], &dir).expect("render");
        assert_eq!(views.len(), 1);
        assert_eq!((views[0].rank, dropped), (0, 0));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The disclaimer must say what the views are NOT — the whole risk is that a rendered,
    /// hashed image reads as sign-off.
    #[test]
    fn the_disclaimer_disclaims() {
        assert!(DISCLAIMER.contains("NOT foundry sign-off"));
        assert!(
            DISCLAIMER.contains("not every"),
            "it must scope what is shown"
        );
    }
}

#[cfg(test)]
mod provenance_tests {
    use super::*;
    use crate::layout::gds::{Cell, Element};
    use crate::rules::{Layer, Rules};

    fn deck(text: &str) -> Rules {
        Rules::parse(text).expect("deck parses")
    }

    #[test]
    fn provenance_names_the_deck_that_produced_the_findings() {
        let r = deck("width 66 100\nspace 68 100\n");
        let p = provenance(&r, &[]);
        assert!(
            p.contains(crate::VERSION),
            "must name the engine version: {p}"
        );
        assert!(p.contains("2 rule(s)"), "must count the rules: {p}");
        assert!(
            p.contains("width") && p.contains("space"),
            "must name the families: {p}"
        );
    }

    /// The clause that changes what a reader should conclude. Without it, "CLEAN" on a layout
    /// whose layers the deck never mentions reads as a pass.
    #[test]
    fn partial_coverage_is_stated_loudly_when_layers_went_unexamined() {
        let r = deck("width 66 100\n");
        let p = provenance(&r, &[Layer::new(68, 0), Layer::new(70, 0)]);
        assert!(p.contains("PARTIAL COVERAGE"), "{p}");
        // it describes the CHECK, so it must not claim things about views that may not exist
        assert!(
            !p.contains("Each view shows"),
            "provenance must not describe views: {p}"
        );
        assert!(
            p.contains("68/0") && p.contains("70/0"),
            "must name the layers: {p}"
        );
        assert!(
            p.contains("not found clean, they were not checked"),
            "must say what the silence means: {p}"
        );
    }

    /// ...and is absent when the deck really did cover everything, so the warning keeps its
    /// force. A disclaimer printed unconditionally is one nobody reads.
    #[test]
    fn full_coverage_carries_no_partial_warning() {
        let p = provenance(&deck("width 66 100\n"), &[]);
        assert!(!p.contains("PARTIAL"), "{p}");
    }

    #[test]
    fn a_long_unchecked_list_is_capped_and_says_how_many_more() {
        let r = deck("width 66 100\n");
        let many: Vec<Layer> = (0..30).map(|i| Layer::new(100 + i, 0)).collect();
        let p = provenance(&r, &many);
        assert!(p.contains("30 layer(s)"), "the total must be exact: {p}");
        assert!(p.contains("more"), "the elision must be visible: {p}");
    }

    /// `elem_rect` returns a sentinel Layer(0,0) for anything it cannot rectangle-ise, so
    /// reading layers through it would report a phantom 0/0 on any layout containing a label
    /// — and would mis-attribute a Path, which is real drawn geometry, to that sentinel
    /// instead of its own layer. Both directions are checked here.
    #[test]
    fn coverage_reads_real_layers_and_ignores_annotations() {
        let cell = Cell {
            elements: vec![
                Element::Boundary {
                    layer: 66,
                    datatype: 0,
                    pts: crate::layout::geom::Rect::new(0, 0, 10, 10).as_boundary(),
                },
                // a Path on an uncovered layer: must be reported as 68/0, not as 0/0
                Element::Path {
                    layer: 68,
                    datatype: 0,
                    width: 4,
                    pts: vec![(0, 0), (100, 0)],
                },
                // an annotation: not geometry, must not appear at all
                Element::Text {
                    layer: 99,
                    texttype: 0,
                    x: 0,
                    y: 0,
                    string: "label".into(),
                },
            ],
            ..Default::default()
        };
        let un = crate::drc::unchecked_layers(&cell, &deck("width 66 100\n"));
        assert_eq!(un, vec![Layer::new(68, 0)], "got {un:?}");
    }

    #[test]
    fn a_deck_covering_everything_reports_nothing_unchecked() {
        let cell = Cell {
            elements: vec![Element::Boundary {
                layer: 66,
                datatype: 0,
                pts: crate::layout::geom::Rect::new(0, 0, 10, 10).as_boundary(),
            }],
            ..Default::default()
        };
        assert!(crate::drc::unchecked_layers(&cell, &deck("width 66 100\n")).is_empty());
    }

    /// `covers` has to consult every rule family, or a layer checked only by (say) an
    /// enclosure rule would be reported as unexamined and the warning would be wrong.
    #[test]
    fn coverage_counts_every_rule_family_not_just_width_and_space() {
        let cell = Cell {
            elements: vec![Element::Boundary {
                layer: 77,
                datatype: 0,
                pts: crate::layout::geom::Rect::new(0, 0, 10, 10).as_boundary(),
            }],
            ..Default::default()
        };
        // layer 77 appears only as the inner of an enclosure rule
        let r = deck("enclosure 76 77 40\n");
        assert!(
            crate::drc::unchecked_layers(&cell, &r).is_empty(),
            "a layer named by an enclosure rule IS examined"
        );
    }

    /// The fill family is consumed by the fill generator, not the checker, so counting it
    /// would overstate how much checking happened.
    #[test]
    fn the_rule_count_excludes_families_the_checker_does_not_run() {
        let (_, with_fill) = deck("width 66 100\nfill 70 40 1000 100 50\n").summary();
        let (_, without) = deck("width 66 100\n").summary();
        assert_eq!(
            with_fill, without,
            "fill must not inflate the checked-rule count"
        );
    }
}
