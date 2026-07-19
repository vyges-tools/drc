//! Group violations into the handful of things actually worth looking at.
//!
//! A block with 40,000 violations does not have 40,000 problems. It usually has a few
//! mistakes repeated through the hierarchy, and a flat list buries that: the first 200 lines
//! of a report are 200 copies of one defect, and the one-off that matters is on line 9,000.
//!
//! # The key, and why it is not the cell
//!
//! The obvious grouping is by cell — "this standard cell is wrong, and it is placed 500
//! times". We cannot do that: the checks run on `flatten::flatten`ed geometry, so a
//! [`Violation`] carries no cell attribution. Inventing one by looking up which cell a
//! coordinate fell in would be a guess dressed as provenance.
//!
//! So the key is `(rule, layer)` — the rule family view. It is coarse, deterministic, and
//! honest about what the data supports. Each cluster additionally reports how many *distinct
//! measured values* it contains, which recovers most of what cell grouping would have told
//! you: one repeated value across hundreds of occurrences is one mistake copied, while
//! hundreds of distinct values are hundreds of separate near-misses.
//!
//! # The two rankings
//!
//! Neither ordering is sufficient alone, so both are produced rather than picking one:
//!
//! * **by count** — what dominates the report, and what fixing would remove most violations;
//! * **by severity** — how badly the worst occurrence misses its bound, relative to the
//!   bound. One catastrophic violation must not be buried under forty trivial ones, which is
//!   exactly what count-ranking alone does.
//!
//! Ties break on the rule name then the layer, so the same input always yields the same
//! order — and therefore the same rendered views, and the same hashes for them.

use crate::drc::Violation;
use crate::layout::geom::Rect;
use std::collections::BTreeMap;

/// One rule-family group, with the evidence needed to render and rank it.
#[derive(Debug, Clone, PartialEq)]
pub struct Cluster {
    pub rule: &'static str,
    pub layer: i16,
    /// How many violations fell in this group.
    pub count: usize,
    /// Distinct measured values across the group. `1` means one defect repeated; a large
    /// number means many separate near-misses that happen to share a rule and layer.
    pub distinct_values: usize,
    /// The bound, taken from the worst occurrence (a deck may state several for one layer).
    pub limit: i64,
    /// The measured value of the worst occurrence.
    pub value: i64,
    /// How badly the worst occurrence misses, as a fraction of the bound. See
    /// [`severity`] — 0.25 means it misses by a quarter of the bound.
    pub severity: f64,
    /// Where the worst occurrence is — the region to render for this cluster.
    pub worst: Rect,
    /// A box covering every occurrence, for a "where is this concentrated" overview.
    pub extent: Rect,
}

/// How badly `v` misses its bound, as a fraction of the bound.
///
/// Rules differ in direction and in unit, so this cannot be `|value - limit|`:
///
/// * most rules are minima (width, space, area, enclosure, venc) — missing by 10 dbu on a
///   20 dbu minimum is far worse than on a 2000 dbu one, so the miss is relative;
/// * `antenna` is a maximum, and `density` can be violated from either side;
/// * `enclosure` and `venc` use a negative `value` as a sentinel for "not enclosed at all",
///   which is total failure rather than a small miss;
/// * `grid` and `track` are positional — a vertex is on the grid or it is not, so there is
///   no meaningful magnitude. They score 1.0 rather than 0.0: off-grid geometry is a real
///   defect, and scoring it zero would rank it below every trivial near-miss.
///
/// A zero bound would divide by zero, so it yields 1.0 — a violation of a zero bound is not
/// a near miss.
pub fn severity(v: &Violation) -> f64 {
    let limit = v.limit;
    match v.rule {
        // total-failure sentinel: not enclosed at all
        "enclosure" | "venc" if v.value < 0 => 1.0,
        // positional: no magnitude to speak of
        "grid" | "track" => 1.0,
        _ if limit == 0 => 1.0,
        // a maximum: how far above
        "antenna" => ((v.value - limit) as f64 / limit as f64).max(0.0),
        // either direction
        "density" => ((v.value - limit).abs() as f64 / limit as f64).max(0.0),
        // span carries the deviation itself rather than a measured quantity
        "span" => (v.value as f64 / limit.max(1) as f64).abs(),
        // the rest are minima: how far below
        _ => ((limit - v.value) as f64 / limit as f64).max(0.0),
    }
}

fn union(a: Rect, b: Rect) -> Rect {
    Rect {
        x0: a.x0.min(b.x0),
        y0: a.y0.min(b.y0),
        x1: a.x1.max(b.x1),
        y1: a.y1.max(b.y1),
    }
}

/// The region a violation occupies — both shapes of a spacing pair, so the rendered window
/// shows what is too close to what rather than one of the two in isolation.
pub fn region(v: &Violation) -> Rect {
    match v.b {
        Some(b) => union(v.a, b),
        None => v.a,
    }
}

/// Group `viols` by `(rule, layer)`.
///
/// The returned order is by descending count, ties broken on rule then layer, so the same
/// input always produces the same order — which matters because these become rendered views
/// whose hashes are part of the evidence.
pub fn cluster(viols: &[Violation]) -> Vec<Cluster> {
    // BTreeMap for a deterministic starting order before the sort, so even the tie-break
    // path cannot depend on hash iteration order.
    let mut groups: BTreeMap<(&'static str, i16), Vec<&Violation>> = BTreeMap::new();
    for v in viols {
        groups.entry((v.rule, v.layer)).or_default().push(v);
    }

    let mut out: Vec<Cluster> = groups
        .into_iter()
        .map(|((rule, layer), members)| {
            // The worst occurrence is the representative: it is the one worth rendering, and
            // the one whose numbers describe the group's ceiling rather than its average.
            let worst = members
                .iter()
                .max_by(|a, b| {
                    severity(a)
                        .partial_cmp(&severity(b))
                        .unwrap_or(std::cmp::Ordering::Equal)
                        // deterministic tie-break: lowest coordinate wins, so the same input
                        // always nominates the same representative
                        .then_with(|| b.a.x0.cmp(&a.a.x0))
                        .then_with(|| b.a.y0.cmp(&a.a.y0))
                })
                .expect("a group is never empty");
            let mut values: Vec<i64> = members.iter().map(|v| v.value).collect();
            values.sort_unstable();
            values.dedup();
            let extent = members
                .iter()
                .map(|v| region(v))
                .reduce(union)
                .expect("a group is never empty");
            Cluster {
                rule,
                layer,
                count: members.len(),
                distinct_values: values.len(),
                limit: worst.limit,
                value: worst.value,
                severity: severity(worst),
                worst: region(worst),
                extent,
            }
        })
        .collect();

    out.sort_by(|a, b| {
        b.count
            .cmp(&a.count)
            .then_with(|| a.rule.cmp(b.rule))
            .then_with(|| a.layer.cmp(&b.layer))
    });
    out
}

/// The same clusters ordered by the severity of their worst occurrence.
///
/// Kept separate from [`cluster`] rather than replacing it: count answers "what should I fix
/// to clear the report", severity answers "what is most broken". A single catastrophic
/// violation is last by count and first here, and both facts matter.
pub fn by_severity(clusters: &[Cluster]) -> Vec<Cluster> {
    let mut out = clusters.to_vec();
    out.sort_by(|a, b| {
        b.severity
            .partial_cmp(&a.severity)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| b.count.cmp(&a.count))
            .then_with(|| a.rule.cmp(b.rule))
            .then_with(|| a.layer.cmp(&b.layer))
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(rule: &'static str, layer: i16, limit: i64, value: i64, x: i32) -> Violation {
        Violation {
            rule,
            layer,
            limit,
            value,
            a: Rect::new(x, 0, x + 10, 10),
            b: None,
        }
    }

    #[test]
    fn violations_group_by_rule_and_layer() {
        let vs = vec![
            v("space", 68, 100, 90, 0),
            v("space", 68, 100, 80, 20),
            v("space", 69, 100, 90, 40),
            v("width", 68, 100, 90, 60),
        ];
        let cs = cluster(&vs);
        assert_eq!(cs.len(), 3, "three distinct (rule, layer) pairs");
        assert_eq!((cs[0].rule, cs[0].layer, cs[0].count), ("space", 68, 2));
    }

    /// These become rendered views whose hashes are part of the evidence, so the order must
    /// not depend on input order or on hash-map iteration.
    #[test]
    fn the_ordering_is_deterministic_regardless_of_input_order() {
        let a = vec![
            v("space", 68, 100, 90, 0),
            v("width", 12, 100, 90, 10),
            v("space", 68, 100, 80, 20),
            v("area", 3, 100, 90, 30),
        ];
        let mut b = a.clone();
        b.reverse();
        let mut c = a.clone();
        c.swap(0, 3);
        let key = |cs: &[Cluster]| -> Vec<(&'static str, i16, usize)> {
            cs.iter().map(|k| (k.rule, k.layer, k.count)).collect()
        };
        assert_eq!(key(&cluster(&a)), key(&cluster(&b)));
        assert_eq!(key(&cluster(&a)), key(&cluster(&c)));
    }

    /// Equal counts must not be ordered arbitrarily: rule name, then layer.
    #[test]
    fn equal_counts_break_ties_on_rule_then_layer() {
        let vs = vec![
            v("width", 9, 100, 90, 0),
            v("area", 9, 100, 90, 10),
            v("width", 2, 100, 90, 20),
        ];
        let cs = cluster(&vs);
        assert_eq!(
            cs.iter().map(|c| (c.rule, c.layer)).collect::<Vec<_>>(),
            vec![("area", 9), ("width", 2), ("width", 9)]
        );
    }

    #[test]
    fn count_ranking_puts_the_dominant_group_first() {
        let mut vs = vec![v("antenna", 5, 100, 500, 0)]; // one catastrophic
        for i in 0..40 {
            vs.push(v("space", 68, 100, 99, i * 10)); // forty trivial
        }
        let cs = cluster(&vs);
        assert_eq!(cs[0].rule, "space", "count ranking leads with the bulk");
        assert_eq!(cs[0].count, 40);
    }

    /// The reason both rankings exist: the one catastrophic violation is last by count and
    /// first by severity. Shipping only the count ranking buries it.
    #[test]
    fn severity_ranking_surfaces_what_count_ranking_buries() {
        let mut vs = vec![v("antenna", 5, 100, 500, 0)];
        for i in 0..40 {
            vs.push(v("space", 68, 100, 99, i * 10));
        }
        let cs = cluster(&vs);
        let sev = by_severity(&cs);
        assert_eq!(
            sev[0].rule, "antenna",
            "severity ranking leads with the worst"
        );
        assert!(
            sev[0].severity > cs[0].severity * 10.0,
            "the antenna miss ({}) should dwarf the spacing miss ({})",
            sev[0].severity,
            cs[0].severity
        );
    }

    /// A minimum missed by a little on a small bound is worse than by a lot on a large one,
    /// which is why severity is relative rather than absolute.
    #[test]
    fn severity_is_relative_to_the_bound() {
        let tight = severity(&v("width", 1, 20, 10, 0)); // 10 short of 20
        let loose = severity(&v("width", 1, 2000, 1900, 0)); // 100 short of 2000
        assert!(
            tight > loose,
            "missing half a small bound ({tight}) must outrank missing 5% of a large one ({loose})"
        );
        assert!(
            (tight - 0.5).abs() < 1e-9,
            "10 short of 20 is a 0.5 miss, got {tight}"
        );
    }

    /// Direction matters: antenna is a maximum, so exceeding it is the violation. Treating it
    /// as a minimum would score every real antenna violation zero.
    #[test]
    fn a_maximum_rule_scores_on_exceeding_not_falling_short() {
        let bad = severity(&v("antenna", 5, 100, 300, 0)); // 3x the limit
        assert!(
            (bad - 2.0).abs() < 1e-9,
            "300 against a 100 max is a 2.0 miss, got {bad}"
        );
        // and a value under the maximum is not a miss at all
        assert_eq!(severity(&v("antenna", 5, 100, 50, 0)), 0.0);
    }

    /// Density can be violated from either side, so both directions must score.
    #[test]
    fn density_scores_in_both_directions() {
        let under = severity(&v("density", 1, 40, 20, 0));
        let over = severity(&v("density", 1, 40, 60, 0));
        assert!(under > 0.0 && over > 0.0, "under={under} over={over}");
        assert!(
            (under - over).abs() < 1e-9,
            "a 20% miss either way scores the same"
        );
    }

    /// `enclosure`/`venc` use a negative value as "not enclosed at all". That is total
    /// failure, and must not score as a small miss — still less a negative one.
    #[test]
    fn the_not_enclosed_sentinel_is_maximal_not_a_small_miss() {
        for rule in ["enclosure", "venc"] {
            let s = severity(&v(rule, 1, 100, -1, 0));
            assert_eq!(s, 1.0, "{rule} sentinel should be total failure, got {s}");
        }
    }

    /// Positional rules have no magnitude. Scoring them 0.0 would rank real off-grid geometry
    /// below every trivial near-miss.
    #[test]
    fn positional_rules_score_as_real_defects() {
        for rule in ["grid", "track"] {
            assert_eq!(severity(&v(rule, 1, 0, 0, 0)), 1.0, "{rule}");
        }
    }

    #[test]
    fn a_zero_bound_does_not_divide_by_zero() {
        let s = severity(&v("width", 1, 0, 5, 0));
        assert!(s.is_finite(), "a zero bound must not produce {s}");
        assert_eq!(s, 1.0);
    }

    /// One repeated defect and many separate ones look identical by count; distinct_values is
    /// what tells them apart, standing in for the cell attribution we do not have.
    #[test]
    fn distinct_values_separates_one_repeated_defect_from_many() {
        let repeated: Vec<_> = (0..20).map(|i| v("space", 68, 100, 90, i * 10)).collect();
        assert_eq!(cluster(&repeated)[0].distinct_values, 1);
        let varied: Vec<_> = (0..20)
            .map(|i| v("space", 68, 100, 90 - i as i64, i * 10))
            .collect();
        assert_eq!(cluster(&varied)[0].distinct_values, 20);
    }

    /// A spacing violation is about two shapes; rendering one of them alone would not show
    /// what it is too close to.
    #[test]
    fn a_spacing_region_covers_both_shapes() {
        let mut sp = v("space", 68, 100, 90, 0);
        sp.b = Some(Rect::new(200, 0, 210, 10));
        let r = region(&sp);
        assert_eq!((r.x0, r.x1), (0, 210), "the region must span both shapes");
    }

    #[test]
    fn extent_covers_every_occurrence_while_worst_points_at_one() {
        let vs = vec![
            v("space", 68, 100, 99, 0),
            v("space", 68, 100, 10, 500), // the worst, far away
        ];
        let c = &cluster(&vs)[0];
        assert_eq!((c.extent.x0, c.extent.x1), (0, 510), "extent covers all");
        assert_eq!(c.worst.x0, 500, "worst points at the worst occurrence");
        assert_eq!(c.value, 10, "and reports its measurement");
    }

    #[test]
    fn no_violations_means_no_clusters() {
        assert!(cluster(&[]).is_empty());
        assert!(by_severity(&[]).is_empty());
    }
}
