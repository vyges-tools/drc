//! Named multi-patterning sign-off rules on a finished layout, and their results database.
//!
//! On a double-patterned layer each mask is drawn on its own GDS layer/datatype, and sign-off
//! checks the layout as written: spacing within one mask, spacing between the two masks, line
//! ends, and whether the shapes could be two-coloured at all. The rules come from a **rule file**
//! read at run time (`.mprules`), one named rule per line, so the names in the report are the
//! names the foundry's deck uses and nothing about any foundry lives in this code:
//!
//! ```text
//! # kind      name          layers + values (DB units)              options
//! space       R.S.1_A       20/1 66                                  [exclude=L | only=L]
//! space2      R.S.2         20/1 20/2 32
//! eol         R.S.4_A       20/1 70 eolmax=80 adjmin=20 ext=16       [lenlt=N | lenge=N] [to=L | toeol] [exclude=L | only=L]
//! oddcycle    R.colour      20/1 20/2 66
//! notchecked  R.C.10_A      why it is not checked, free text
//! ```
//!
//! What each kind measures, on the merged (unioned) shapes of each layer:
//! - `space L d`: two boundary edges of L facing each other across empty space closer than `d`,
//!   side to side (projections overlap) or corner to corner (Euclidean); a notch of one polygon
//!   counts. Edge-to-edge external spacing, the plain same-mask spacing rule.
//! - `space2 A B d`: the same between an edge of A and an edge of B, facing each other across
//!   space that is in neither layer. Shapes of A and B that touch or overlap (a stitch) are not a
//!   spacing case.
//! - `eol L d`: a **line end** is a boundary edge no longer than `eolmax` whose two corners are
//!   convex and whose two neighbouring edges are at least `adjmin` long. Its window reaches `d`
//!   outward from the edge, widened by `ext` on each side; any shape of the target in the open
//!   window violates. The target is L itself, the layer `to=`, or (`toeol`) another line end of
//!   L facing it, itself widened by `ext`. `lenlt` / `lenge` keep only line ends shorter than /
//!   at least that long.
//! - `oddcycle A B d`: the shapes of A and B (each polygon a node; a polygon of A overlapping
//!   one of B is a stitch, not a constraint) joined wherever two are closer than `d`; an odd
//!   cycle cannot be two-coloured by any assignment. Each odd cycle found is one result.
//! - `exclude=L` drops a result whose shapes both lie in layer L (touch it); `only=L` keeps only
//!   those. A region marker, read by interaction.
//! - `notchecked` states a rule the file knows of and this engine does not check, so the report
//!   says so instead of letting silence read as clean.
//!
//! [`write_rdb`] writes the results in the ASCII results-database layout physical-verification
//! tools use, one check per rule name, so the output lines up rule by rule with another tool's.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use crate::drc::{bbox_edge, closest_pts, elem_poly, outward_edge, pick_top, point_in_region, projecting_edges};
use crate::layout::boolean::{self, Op};
use crate::layout::contour::trace_contours;
use crate::layout::edges::{ring_edges, Axis, Edge};
use crate::layout::flatten;
use crate::layout::gds::Library;
use crate::layout::geom::Rect;
use crate::layout::index::RegionIndex;
use crate::rules::Layer;

/// Where a rule's results are kept, by a marker layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Region {
    All,
    Exclude(Layer),
    Only(Layer),
}

/// What a line-end rule measures to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EolTarget {
    SameLayer,
    Layer(Layer),
    LineEnd,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    Space { layer: Layer, d: i64 },
    Space2 { a: Layer, b: Layer, d: i64 },
    Eol { layer: Layer, d: i64, eol_max: i64, adj_min: i64, ext: i64, len_lt: Option<i64>, len_ge: Option<i64>, to: EolTarget },
    OddCycle { a: Layer, b: Layer, d: i64 },
    NotChecked { why: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaskRule {
    pub name: String,
    pub kind: Kind,
    pub region: Region,
}

impl MaskRule {
    pub fn kind_name(&self) -> &'static str {
        match self.kind {
            Kind::Space { .. } => "space",
            Kind::Space2 { .. } => "space2",
            Kind::Eol { .. } => "eol",
            Kind::OddCycle { .. } => "oddcycle",
            Kind::NotChecked { .. } => "notchecked",
        }
    }
}

/// One result of a rule: an edge pair (spacing, line end) or a polygon (odd cycle).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Marker {
    /// Two edges, each `(x1, y1, x2, y2)`, and the measured distance.
    Edges { a: [i32; 4], b: [i32; 4], value: i64 },
    /// The bounding boxes of the polygons of one odd cycle, in cycle order.
    Cycle { boxes: Vec<Rect> },
}

#[derive(Debug, Clone)]
pub struct RuleResult {
    pub name: String,
    pub kind: &'static str,
    /// The bound, DB units (0 for `notchecked`).
    pub limit: i64,
    pub checked: bool,
    pub why_not: Option<String>,
    pub markers: Vec<Marker>,
}

/// Parse a rule file. Every line is a rule or a comment (`#`); anything else is an error with
/// its line number, never skipped. Rule names are unique.
pub fn parse(text: &str) -> Result<Vec<MaskRule>, String> {
    let mut out: Vec<MaskRule> = Vec::new();
    let mut names = BTreeSet::new();
    for (n, raw) in text.lines().enumerate() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let err = |what: &str| format!("line {}: {what}: {:?}", n + 1, raw.trim());
        let toks: Vec<&str> = line.split_whitespace().collect();
        if toks.len() < 2 {
            return Err(err("expected `<kind> <name> ...`"));
        }
        let (kind, name) = (toks[0], toks[1].to_string());
        if !names.insert(name.clone()) {
            return Err(err("rule name given twice"));
        }
        if kind == "notchecked" {
            let why = toks[2..].join(" ");
            if why.is_empty() {
                return Err(err("notchecked needs a reason"));
            }
            out.push(MaskRule { name, kind: Kind::NotChecked { why }, region: Region::All });
            continue;
        }
        let (pos, opts): (Vec<&str>, Vec<&str>) = toks[2..].iter().partition(|t| !t.contains('=') && **t != "toeol");
        let layer = |k: usize| pos.get(k).and_then(|s| Layer::parse(s)).ok_or_else(|| err("expected a layer"));
        let num = |k: usize| pos.get(k).and_then(|s| s.parse::<i64>().ok()).filter(|v| *v > 0).ok_or_else(|| err("expected a positive DB-unit value"));
        let mut region = Region::All;
        let mut kv: BTreeMap<&str, &str> = BTreeMap::new();
        let mut toeol = false;
        for o in &opts {
            if *o == "toeol" {
                toeol = true;
                continue;
            }
            let (k, v) = o.split_once('=').expect("partitioned on '='");
            match k {
                "exclude" | "only" => {
                    let l = Layer::parse(v).ok_or_else(|| err("bad region layer"))?;
                    if region != Region::All {
                        return Err(err("one region option per rule"));
                    }
                    region = if k == "exclude" { Region::Exclude(l) } else { Region::Only(l) };
                }
                _ => {
                    if kv.insert(k, v).is_some() {
                        return Err(err("option given twice"));
                    }
                }
            }
        }
        let opt = |k: &str| -> Result<Option<i64>, String> {
            kv.get(k).map(|v| v.parse::<i64>().map_err(|_| err("bad option value"))).transpose()
        };
        let want = |n: usize| if pos.len() == n { Ok(()) } else { Err(err("wrong number of arguments")) };
        let rule = match kind {
            "space" => {
                want(2)?;
                Kind::Space { layer: layer(0)?, d: num(1)? }
            }
            "space2" => {
                want(3)?;
                Kind::Space2 { a: layer(0)?, b: layer(1)?, d: num(2)? }
            }
            "oddcycle" => {
                want(3)?;
                Kind::OddCycle { a: layer(0)?, b: layer(1)?, d: num(2)? }
            }
            "eol" => {
                want(2)?;
                let to = match (kv.get("to"), toeol) {
                    (Some(_), true) => return Err(err("to= and toeol together")),
                    (Some(v), false) => EolTarget::Layer(Layer::parse(v).ok_or_else(|| err("bad to= layer"))?),
                    (None, true) => EolTarget::LineEnd,
                    (None, false) => EolTarget::SameLayer,
                };
                let eol_max = opt("eolmax")?.ok_or_else(|| err("eol needs eolmax="))?;
                Kind::Eol {
                    layer: layer(0)?,
                    d: num(1)?,
                    eol_max,
                    adj_min: opt("adjmin")?.unwrap_or(0),
                    ext: opt("ext")?.unwrap_or(0),
                    len_lt: opt("lenlt")?,
                    len_ge: opt("lenge")?,
                    to,
                }
            }
            _ => return Err(err("unknown rule kind")),
        };
        let allowed: &[&str] = if kind == "eol" { &["eolmax", "adjmin", "ext", "lenlt", "lenge", "to"] } else { &[] };
        if let Some(k) = kv.keys().find(|k| !allowed.contains(k)) {
            return Err(err(&format!("option {k}= does not apply")));
        }
        if toeol && kind != "eol" {
            return Err(err("toeol applies to eol only"));
        }
        out.push(MaskRule { name, kind: rule, region });
    }
    Ok(out)
}

/// One layer's merged geometry: the tiles of its union, their index, and its boundary rings.
struct Merged {
    tiles: Vec<Rect>,
    idx: RegionIndex,
    rings: Vec<Vec<Edge>>,
}

impl Merged {
    fn new(polys: &[Vec<(i32, i32)>]) -> Merged {
        let tiles = boolean::boolean_poly(polys, &[], Op::Or);
        let idx = RegionIndex::build(&tiles);
        let rings = trace_contours(&tiles).iter().map(|r| ring_edges(r)).collect();
        Merged { tiles, idx, rings }
    }

    fn edges(&self) -> Vec<Edge> {
        self.rings.iter().flatten().copied().collect()
    }

    /// Does the open rectangle `q` overlap the region with positive area?
    fn overlaps_open(&self, q: &Rect) -> Option<Vec<Rect>> {
        let hit: Vec<Rect> = self
            .idx
            .overlaps(q)
            .into_iter()
            .map(|i| self.tiles[i as usize])
            .filter(|t| t.x0.max(q.x0) < t.x1.min(q.x1) && t.y0.max(q.y0) < t.y1.min(q.y1))
            .collect();
        (!hit.is_empty()).then_some(hit)
    }

    /// Does the closed rectangle `q` touch the region?
    fn touches(&self, q: &Rect) -> bool {
        self.idx.overlaps(q).into_iter().any(|i| {
            let t = self.tiles[i as usize];
            t.x0 <= q.x1 && q.x0 <= t.x1 && t.y0 <= q.y1 && q.y0 <= t.y1
        })
    }
}

fn seg(e: &Edge) -> [i32; 4] {
    [e.a.0, e.a.1, e.b.0, e.b.1]
}

fn seg_box(s: &[i32; 4]) -> Rect {
    Rect { x0: s[0].min(s[2]), y0: s[1].min(s[3]), x1: s[0].max(s[2]), y1: s[1].max(s[3]) }
}

/// Line ends of one layer: boundary edges no longer than `eol_max`, both corners convex (the
/// solid on the left of travel, so a left turn), both neighbours at least `adj_min` long.
fn line_ends(m: &Merged, eol_max: i64, adj_min: i64) -> Vec<Edge> {
    let mut out = Vec::new();
    for ring in &m.rings {
        let n = ring.len();
        if n < 4 {
            continue;
        }
        let turn = |p: &Edge, q: &Edge| -> i64 {
            let (ux, uy) = ((p.b.0 - p.a.0) as i64, (p.b.1 - p.a.1) as i64);
            let (vx, vy) = ((q.b.0 - q.a.0) as i64, (q.b.1 - q.a.1) as i64);
            ux * vy - uy * vx
        };
        for i in 0..n {
            let (p, e, q) = (&ring[(i + n - 1) % n], &ring[i], &ring[(i + 1) % n]);
            if e.len() <= eol_max && p.len() >= adj_min && q.len() >= adj_min && turn(p, e) > 0 && turn(e, q) > 0 {
                out.push(*e);
            }
        }
    }
    out
}

/// The open window of a line end: from its edge outward by `d`, widened by `ext` each side.
fn eol_window(e: &Edge, d: i64, ext: i64) -> Rect {
    let (ox, oy) = outward_edge(e);
    let b = bbox_edge(e);
    let (d, ext) = (d as i32, ext as i32);
    if e.axis() == Axis::Horizontal {
        let (y0, y1) = if oy > 0 { (b.y0, b.y0 + d) } else { (b.y0 - d, b.y0) };
        Rect { x0: b.x0 - ext, y0, x1: b.x1 + ext, y1 }
    } else {
        let (x0, x1) = if ox > 0 { (b.x0, b.x0 + d) } else { (b.x0 - d, b.x0) };
        Rect { x0, y0: b.y0 - ext, x1, y1: b.y1 + ext }
    }
}

/// Perpendicular distance from a line end's edge to a rectangle in its window.
fn eol_gap(e: &Edge, r: &Rect) -> i64 {
    let (ox, oy) = outward_edge(e);
    let at = if e.axis() == Axis::Horizontal { e.a.1 } else { e.a.0 };
    let v = match (ox, oy) {
        (1, _) => r.x0 - at,
        (-1, _) => at - r.x1,
        (_, 1) => r.y0 - at,
        _ => at - r.y1,
    };
    v.max(0) as i64
}

/// External spacing between edges of `ea` (layer A) and `eb` (layer B): parallel, facing each
/// other (outward normals toward each other), closer than `d`, across space in neither region.
fn ext_pairs(ea: &[Edge], eb: &[Edge], d: i64, ma: &Merged, mb: &Merged) -> Vec<Marker> {
    let boxes: Vec<Rect> = eb.iter().map(bbox_edge).collect();
    let idx = RegionIndex::build(&boxes);
    let d2 = d * d;
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    for a in ea {
        for j in idx.within(&bbox_edge(a), d as i32, None) {
            let b = &eb[j as usize];
            if a.axis() != b.axis() {
                continue;
            }
            let (ca, cb) = closest_pts(a, b);
            let (dx, dy) = ((cb.0 - ca.0) as i64, (cb.1 - ca.1) as i64);
            let ds = dx * dx + dy * dy;
            if ds == 0 || ds >= d2 {
                continue;
            }
            // facing: along the separating direction for a side-to-side pair, along the
            // diagonal for a corner pair
            let dir = if projecting_edges(a, b) {
                if a.axis() == Axis::Horizontal { (0, dy.signum()) } else { (dx.signum(), 0) }
            } else {
                (dx, dy)
            };
            let (oa, ob) = (outward_edge(a), outward_edge(b));
            if oa.0 * dir.0 + oa.1 * dir.1 <= 0 || ob.0 * dir.0 + ob.1 * dir.1 >= 0 {
                continue;
            }
            let mid = if projecting_edges(a, b) {
                let (ra, rb) = (bbox_edge(a), bbox_edge(b));
                if a.axis() == Axis::Horizontal {
                    let x = (ra.x0.max(rb.x0) + ra.x1.min(rb.x1)) / 2;
                    (x, (ca.1 + cb.1) / 2)
                } else {
                    let y = (ra.y0.max(rb.y0) + ra.y1.min(rb.y1)) / 2;
                    ((ca.0 + cb.0) / 2, y)
                }
            } else {
                ((ca.0 + cb.0) / 2, (ca.1 + cb.1) / 2)
            };
            if point_in_region(mid, &ma.tiles, &ma.idx) || point_in_region(mid, &mb.tiles, &mb.idx) {
                continue;
            }
            // one result per approach: a side-to-side pair once whichever edge comes first, a
            // corner pair once though both its horizontal and its vertical edges see it
            let k = if projecting_edges(a, b) {
                let (x, y) = (seg(a), seg(b));
                if x <= y { (x, y) } else { (y, x) }
            } else {
                let (x, y) = ([ca.0, ca.1, ca.0, ca.1], [cb.0, cb.1, cb.0, cb.1]);
                if x <= y { (x, y) } else { (y, x) }
            };
            if seen.insert(k) {
                out.push(Marker::Edges { a: seg(a), b: seg(b), value: (ds as f64).sqrt().round() as i64 });
            }
        }
    }
    out
}

/// Odd cycles of the conflict graph over the polygons of A and B.
fn odd_cycles(ma: &Merged, mb: &Merged, d: i64) -> Vec<Marker> {
    // polygons: connected tiles of each region (tiles of one union that touch on an edge or
    // overlap; the union's tiles never overlap)
    let comps = |m: &Merged| -> Vec<usize> {
        let mut p: Vec<usize> = (0..m.tiles.len()).collect();
        fn find(p: &mut [usize], mut x: usize) -> usize {
            while p[x] != x {
                p[x] = p[p[x]];
                x = p[x];
            }
            x
        }
        for (i, t) in m.tiles.iter().enumerate() {
            for j in m.idx.overlaps(t) {
                let (j, u) = (j as usize, m.tiles[j as usize]);
                let ox = t.x1.min(u.x1) - t.x0.max(u.x0);
                let oy = t.y1.min(u.y1) - t.y0.max(u.y0);
                if j != i && ox >= 0 && oy >= 0 && (ox > 0 || oy > 0) {
                    let (a, b) = (find(&mut p, i), find(&mut p, j));
                    p[a.max(b)] = a.min(b);
                }
            }
        }
        (0..m.tiles.len()).map(|i| find(&mut p, i)).collect()
    };
    let (ca, cb) = (comps(ma), comps(mb));
    // node ids: A's polygons, then B's, numbered by their lowest tile
    let mut node: BTreeMap<(u8, usize), usize> = BTreeMap::new();
    for (s, c) in [(0u8, &ca), (1u8, &cb)] {
        for &r in c.iter() {
            let n = node.len();
            node.entry((s, r)).or_insert(n);
        }
    }
    let mut bbox: Vec<Option<Rect>> = vec![None; node.len()];
    let mut grow = |n: usize, t: &Rect| {
        bbox[n] = Some(match bbox[n] {
            None => *t,
            Some(b) => Rect { x0: b.x0.min(t.x0), y0: b.y0.min(t.y0), x1: b.x1.max(t.x1), y1: b.y1.max(t.y1) },
        });
    };
    for (i, t) in ma.tiles.iter().enumerate() {
        grow(node[&(0, ca[i])], t);
    }
    for (i, t) in mb.tiles.iter().enumerate() {
        grow(node[&(1, cb[i])], t);
    }
    // conflict edges: two tiles of different polygons closer than d (not touching)
    let gap2 = |a: &Rect, b: &Rect| -> Option<i64> {
        let dx = (a.x0 - b.x1).max(b.x0 - a.x1).max(0) as i64;
        let dy = (a.y0 - b.y1).max(b.y0 - a.y1).max(0) as i64;
        (dx > 0 || dy > 0).then_some(dx * dx + dy * dy)
    };
    let mut adj: BTreeMap<usize, BTreeSet<usize>> = BTreeMap::new();
    let sides: [(&Merged, &Vec<usize>, u8); 2] = [(ma, &ca, 0), (mb, &cb, 1)];
    for &(m1, c1, s1) in &sides {
        for &(m2, c2, s2) in &sides {
            if s2 < s1 {
                continue;
            }
            for (i, t) in m1.tiles.iter().enumerate() {
                for j in m2.idx.within(t, d as i32, None) {
                    let u = &m2.tiles[j as usize];
                    let (n1, n2) = (node[&(s1, c1[i])], node[&(s2, c2[j as usize])]);
                    if n1 == n2 {
                        continue;
                    }
                    if gap2(t, u).is_some_and(|g| g < d * d) {
                        adj.entry(n1).or_default().insert(n2);
                        adj.entry(n2).or_default().insert(n1);
                    }
                }
            }
        }
    }
    // breadth first, lowest node first; every non-tree edge joining one side closes an odd cycle
    let mut side: BTreeMap<usize, (u8, usize)> = BTreeMap::new(); // side, parent
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    for &start in adj.keys() {
        if side.contains_key(&start) {
            continue;
        }
        side.insert(start, (0, start));
        let mut q = VecDeque::from([start]);
        while let Some(u) = q.pop_front() {
            for &v in &adj[&u] {
                match side.get(&v).copied() {
                    None => {
                        side.insert(v, (1 - side[&u].0, u));
                        q.push_back(v);
                    }
                    Some((s, _)) if s == side[&u].0 && u < v => {
                        let path = |mut x: usize| {
                            let mut p = vec![x];
                            while side[&x].1 != x {
                                x = side[&x].1;
                                p.push(x);
                            }
                            p
                        };
                        let (pu, pv) = (path(u), path(v));
                        let common: BTreeSet<usize> = pu.iter().copied().collect();
                        let lca = *pv.iter().find(|x| common.contains(x)).expect("one tree");
                        let mut cyc: Vec<usize> = pu.iter().copied().take_while(|&x| x != lca).collect();
                        cyc.push(lca);
                        let back: Vec<usize> = pv.iter().copied().take_while(|&x| x != lca).collect();
                        cyc.extend(back.into_iter().rev());
                        let key: BTreeSet<usize> = cyc.iter().copied().collect();
                        if seen.insert(key) {
                            out.push(Marker::Cycle { boxes: cyc.iter().map(|&n| bbox[n].expect("node has tiles")).collect() });
                        }
                    }
                    Some(_) => {}
                }
            }
        }
    }
    out
}

/// Run the rule file over the flattened top cell.
pub fn check(lib: &Library, top: Option<&str>, rules: &[MaskRule]) -> Result<Vec<RuleResult>, String> {
    let top = pick_top(lib, top)?;
    let cell = flatten::flatten(lib, &top)?;
    let mut want: BTreeSet<Layer> = BTreeSet::new();
    for r in rules {
        match &r.kind {
            Kind::Space { layer, .. } => {
                want.insert(*layer);
            }
            Kind::Space2 { a, b, .. } | Kind::OddCycle { a, b, .. } => {
                want.insert(*a);
                want.insert(*b);
            }
            Kind::Eol { layer, to, .. } => {
                want.insert(*layer);
                if let EolTarget::Layer(l) = to {
                    want.insert(*l);
                }
            }
            Kind::NotChecked { .. } => {}
        }
        if let Region::Exclude(l) | Region::Only(l) = r.region {
            want.insert(l);
        }
    }
    let mut polys: BTreeMap<Layer, Vec<Vec<(i32, i32)>>> = BTreeMap::new();
    for e in &cell.elements {
        let (l, p) = elem_poly(e);
        if let Some(p) = p.filter(|_| want.contains(&l)) {
            polys.entry(l).or_default().push(p);
        }
    }
    let merged: BTreeMap<Layer, Merged> = want.iter().map(|l| (*l, Merged::new(polys.get(l).map_or(&[][..], |v| v)))).collect();
    let mut out = Vec::new();
    for r in rules {
        let (limit, markers) = match &r.kind {
            Kind::NotChecked { why } => {
                out.push(RuleResult { name: r.name.clone(), kind: r.kind_name(), limit: 0, checked: false, why_not: Some(why.clone()), markers: vec![] });
                continue;
            }
            Kind::Space { layer, d } => {
                let m = &merged[layer];
                let e = m.edges();
                (*d, ext_pairs(&e, &e, *d, m, m))
            }
            Kind::Space2 { a, b, d } => {
                let (ma, mb) = (&merged[a], &merged[b]);
                (*d, ext_pairs(&ma.edges(), &mb.edges(), *d, ma, mb))
            }
            Kind::OddCycle { a, b, d } => (*d, odd_cycles(&merged[a], &merged[b], *d)),
            Kind::Eol { layer, d, eol_max, adj_min, ext, len_lt, len_ge, to } => {
                let m = &merged[layer];
                let ends: Vec<Edge> = line_ends(m, *eol_max, *adj_min)
                    .into_iter()
                    .filter(|e| len_lt.is_none_or(|v| e.len() < v) && len_ge.is_none_or(|v| e.len() >= v))
                    .collect();
                let mut found = Vec::new();
                match to {
                    EolTarget::SameLayer | EolTarget::Layer(_) => {
                        let t = match to {
                            EolTarget::Layer(l) => &merged[l],
                            _ => m,
                        };
                        for e in &ends {
                            if let Some(hit) = t.overlaps_open(&eol_window(e, *d, *ext)) {
                                let near = hit.iter().min_by_key(|h| (eol_gap(e, h), h.x0, h.y0)).expect("hit");
                                found.push(Marker::Edges { a: seg(e), b: [near.x0, near.y0, near.x1, near.y1], value: eol_gap(e, near) });
                            }
                        }
                    }
                    EolTarget::LineEnd => {
                        let all = line_ends(m, *eol_max, *adj_min);
                        let mut seen = BTreeSet::new();
                        for e in &ends {
                            let w = eol_window(e, *d, *ext);
                            for f in &all {
                                if f == e || f.axis() != e.axis() || outward_edge(f) != (-outward_edge(e).0, -outward_edge(e).1) {
                                    continue;
                                }
                                // the other end, widened by ext the same way, inside the open window
                                let fb = eol_window(f, 0, *ext);
                                let fb = Rect { x0: fb.x0, y0: fb.y0, x1: fb.x1.max(fb.x0), y1: fb.y1.max(fb.y0) };
                                let g = eol_gap(e, &bbox_edge(f));
                                let along = if e.axis() == Axis::Horizontal { w.x0.max(fb.x0) < w.x1.min(fb.x1) } else { w.y0.max(fb.y0) < w.y1.min(fb.y1) };
                                if g > 0 && g < *d && along {
                                    let k = if seg(e) <= seg(f) { (seg(e), seg(f)) } else { (seg(f), seg(e)) };
                                    if seen.insert(k) {
                                        found.push(Marker::Edges { a: k.0, b: k.1, value: g });
                                    }
                                }
                            }
                        }
                    }
                }
                (*d, found)
            }
        };
        let markers = match r.region {
            Region::All => markers,
            Region::Exclude(l) | Region::Only(l) => {
                let keep_inside = matches!(r.region, Region::Only(_));
                let m = &merged[&l];
                markers
                    .into_iter()
                    .filter(|mk| {
                        let inside = match mk {
                            Marker::Edges { a, b, .. } => m.touches(&seg_box(a)) && m.touches(&seg_box(b)),
                            Marker::Cycle { boxes } => boxes.iter().all(|b| m.touches(b)),
                        };
                        inside == keep_inside
                    })
                    .collect()
            }
        };
        out.push(RuleResult { name: r.name.clone(), kind: r.kind_name(), limit, checked: true, why_not: None, markers });
    }
    Ok(out)
}

/// The results in the ASCII results-database layout: a header line (`top precision`), then per
/// checked rule its name, a count line (`current original 1`), one text line stating the rule,
/// and its results: `e <n> 2` with two `x1 y1 x2 y2` edge lines for a pair, `p <n> <k>` with k
/// vertex lines for a polygon (an odd cycle: the hull boxes, one polygon per member). Rules not
/// checked are not written: absent is not clean, and the JSON report lists them.
pub fn write_rdb(results: &[RuleResult], top: &str, precision: u32) -> String {
    let mut s = format!("{top} {precision}\n");
    for r in results.iter().filter(|r| r.checked) {
        let n = r.markers.len();
        s.push_str(&format!("{}\n{n} {n} 1\n{} {} {}\n", r.name, r.kind, r.name, r.limit));
        let mut k = 0;
        for m in &r.markers {
            match m {
                Marker::Edges { a, b, .. } => {
                    k += 1;
                    s.push_str(&format!("e {k} 2\n{} {} {} {}\n{} {} {} {}\n", a[0], a[1], a[2], a[3], b[0], b[1], b[2], b[3]));
                }
                Marker::Cycle { boxes } => {
                    k += 1;
                    let pts: Vec<(i32, i32)> = boxes.iter().flat_map(|b| [(b.x0, b.y0), (b.x1, b.y0), (b.x1, b.y1), (b.x0, b.y1)]).collect();
                    s.push_str(&format!("p {k} {}\n", pts.len()));
                    for (x, y) in pts {
                        s.push_str(&format!("{x} {y}\n"));
                    }
                }
            }
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::gds::{Cell, Element};

    fn bx(l: i16, dt: i16, x0: i32, y0: i32, x1: i32, y1: i32) -> Element {
        Element::Boundary { layer: l, datatype: dt, pts: vec![(x0, y0), (x1, y0), (x1, y1), (x0, y1), (x0, y0)] }
    }

    fn lib(els: Vec<Element>) -> Library {
        Library { cells: vec![Cell { name: "t".into(), elements: els }], ..Library::default() }
    }

    fn run(rules: &str, els: Vec<Element>) -> Vec<(String, usize)> {
        let r = parse(rules).unwrap();
        check(&lib(els), None, &r).unwrap().into_iter().map(|x| (x.name, x.markers.len())).collect()
    }

    fn count(rules: &str, els: Vec<Element>) -> usize {
        run(rules, els)[0].1
    }

    /// Rule (space): two shapes of one mask closer than d, side to side or corner to corner, are
    /// one result; at d they are clean; a notch counts; another mask does not.
    #[test]
    fn space_is_external_spacing_on_one_mask() {
        let r = "space S 1/1 60";
        assert_eq!(count(r, vec![bx(1, 1, 0, 0, 100, 30), bx(1, 1, 0, 89, 100, 119)]), 1, "59 apart");
        assert_eq!(count(r, vec![bx(1, 1, 0, 0, 100, 30), bx(1, 1, 0, 90, 100, 120)]), 0, "60 apart");
        assert_eq!(count(r, vec![bx(1, 1, 0, 0, 100, 30), bx(1, 2, 0, 50, 100, 80)]), 0, "other mask");
        // corner to corner: 40 and 40 apart diagonally, 56.6 < 60
        assert_eq!(count(r, vec![bx(1, 1, 0, 0, 100, 30), bx(1, 1, 140, 70, 240, 100)]), 1);
        // a U: the notch between its arms is 50 wide
        let u = vec![bx(1, 1, 0, 0, 200, 30), bx(1, 1, 0, 0, 30, 200), bx(1, 1, 80, 0, 110, 200)];
        assert_eq!(count(r, u), 1);
    }

    /// Rule (space2): an edge of one mask facing an edge of the other closer than d; shapes of
    /// the two masks that touch or overlap (a stitch) are not a spacing case.
    #[test]
    fn space2_is_spacing_between_the_masks() {
        let r = "space2 X 1/1 1/2 32";
        assert_eq!(count(r, vec![bx(1, 1, 0, 0, 100, 30), bx(1, 2, 0, 61, 100, 91)]), 1, "31 apart");
        assert_eq!(count(r, vec![bx(1, 1, 0, 0, 100, 30), bx(1, 2, 0, 62, 100, 92)]), 0, "32 apart");
        assert_eq!(count(r, vec![bx(1, 1, 0, 0, 100, 30), bx(1, 2, 80, 0, 200, 30)]), 0, "overlap: a stitch");
        assert_eq!(count(r, vec![bx(1, 1, 0, 0, 100, 30), bx(1, 1, 0, 61, 100, 91)]), 0, "same mask: not this rule");
        // across metal: A faces the second B shape 31 away, but the first B shape lies between
        // them, so only the 10 to the first is a result
        let between = vec![bx(1, 1, 0, 0, 100, 30), bx(1, 2, 0, 40, 100, 60), bx(1, 2, 0, 61, 100, 90)];
        assert_eq!(count(r, between), 1);
    }

    /// Rule (eol): a line end (short edge, two convex corners, long neighbours) looks `d`
    /// outward through a window widened by `ext`; a shape in the open window is a result. A
    /// side edge is not a line end; `lenlt` keeps only short ends.
    #[test]
    fn a_line_end_sees_its_window() {
        let wire = bx(1, 1, 0, 0, 30, 300); // ends at y=0 and y=300, 30 wide
        let r = "eol E 1/1 70 eolmax=40 adjmin=20 ext=16";
        assert_eq!(count(r, vec![wire.clone(), bx(1, 1, -200, 369, 200, 400)]), 1, "69 above the top end");
        assert_eq!(count(r, vec![wire.clone(), bx(1, 1, -200, 370, 200, 400)]), 0, "70 above");
        // beside the window: 17 left of the end's widened span (x < -16) is out, 15 is in
        assert_eq!(count(r, vec![wire.clone(), bx(1, 1, -100, 320, -17, 340)]), 0);
        assert_eq!(count(r, vec![wire.clone(), bx(1, 1, -100, 320, -15, 340)]), 1);
        assert_eq!(count(r, vec![wire.clone(), bx(1, 1, -100, 320, -16, 340)]), 0, "touching the window's side");
        // a stub whose neighbours are shorter than adjmin has no line end
        assert_eq!(count(r, vec![bx(1, 1, 0, 0, 30, 10), bx(1, 1, -200, 69, 200, 100)]), 0);
        // the bottom of a notch is short but its corners are concave: not a line end
        let u = vec![bx(1, 1, 0, 0, 110, 30), bx(1, 1, 0, 0, 30, 300), bx(1, 1, 80, 0, 110, 300)];
        assert_eq!(count("eol E 1/1 70 eolmax=60 adjmin=20 ext=16", u), 0);
        // the long sides are not line ends: a shape 50 to the side is not seen
        assert_eq!(count(r, vec![wire.clone(), bx(1, 1, 80, 100, 110, 200)]), 0);
        // only ends shorter than 30: this one is 30
        assert_eq!(count("eol E 1/1 70 eolmax=40 adjmin=20 ext=16 lenlt=30", vec![wire.clone(), bx(1, 1, -200, 369, 200, 400)]), 0);
        // to another layer
        assert_eq!(count("eol E 1/1 70 eolmax=40 ext=16 to=1/2", vec![wire.clone(), bx(1, 2, -200, 369, 200, 400)]), 1);
        assert_eq!(count("eol E 1/1 70 eolmax=40 ext=16 to=1/2", vec![wire, bx(1, 1, -200, 369, 200, 400)]), 0);
    }

    /// Rule (eol, toeol): two line ends facing each other closer than d; a line end facing a
    /// side is not this rule.
    #[test]
    fn line_end_to_line_end() {
        let r = "eol T 1/1 80 eolmax=40 adjmin=20 ext=32 toeol";
        assert_eq!(count(r, vec![bx(1, 1, 0, 0, 30, 300), bx(1, 1, 0, 379, 30, 600)]), 1, "79 tip to tip");
        assert_eq!(count(r, vec![bx(1, 1, 0, 0, 30, 300), bx(1, 1, 0, 380, 30, 600)]), 0, "80");
        assert_eq!(count(r, vec![bx(1, 1, 0, 0, 30, 300), bx(1, 1, -300, 340, 300, 400)]), 0, "a side, not an end");
        // a short wire above: its bottom end faces the tip (30), its top end faces away (60)
        assert_eq!(count(r, vec![bx(1, 1, 0, 0, 30, 300), bx(1, 1, 0, 330, 30, 360)]), 1);
    }

    /// Rule (oddcycle): three shapes each closer than d to the other two cannot be two-coloured,
    /// whatever their masks; four in a ring can. A stitch (the two masks overlapping) is not a
    /// constraint.
    #[test]
    fn an_odd_cycle_is_found_on_the_union_of_the_masks() {
        let r = "oddcycle C 1/1 1/2 60";
        let tri = vec![bx(1, 1, 0, 0, 100, 30), bx(1, 2, 0, 80, 100, 110), bx(1, 1, 130, 30, 230, 80)];
        assert_eq!(count(r, tri), 1);
        // one side of the triangle exactly d long is no conflict: no cycle
        let at_d = vec![bx(1, 1, 0, 0, 100, 30), bx(1, 2, 0, 90, 100, 120), bx(1, 1, 130, 30, 230, 90)];
        assert_eq!(count(r, at_d), 0);
        // 50 apart side by side, 70.7 across the diagonal
        let ring = vec![bx(1, 1, 0, 0, 30, 30), bx(1, 2, 80, 0, 110, 30), bx(1, 1, 80, 80, 110, 110), bx(1, 2, 0, 80, 30, 110)];
        assert_eq!(count(r, ring), 0);
        // a stitched wire (1/1 and 1/2 overlapping) between two shapes: no cycle through it
        let st = vec![bx(1, 1, 0, 0, 100, 30), bx(1, 2, 90, 0, 200, 30), bx(1, 1, 50, 80, 150, 110)];
        assert_eq!(count(r, st), 0);
    }

    /// Region options: `exclude=` drops a result inside the marker, `only=` keeps only those.
    #[test]
    fn a_marker_region_selects_results() {
        let els = vec![bx(1, 1, 0, 0, 100, 30), bx(1, 1, 0, 70, 100, 100), bx(9, 9, -10, -10, 110, 110)];
        assert_eq!(count("space S 1/1 60 exclude=9/9", els.clone()), 0);
        assert_eq!(count("space S 1/1 60 only=9/9", els.clone()), 1);
        assert_eq!(count("space S 1/1 60 only=9/8", els), 0);
    }

    /// The file: every line parses or is an error with its line; names are unique; a
    /// notchecked rule is reported, not checked.
    #[test]
    fn the_rule_file_is_strict() {
        assert!(parse("space A 1/1 60\nspace A 1/2 60").unwrap_err().contains("line 2"));
        assert!(parse("spaec A 1/1 60").is_err());
        assert!(parse("space A 1/1").is_err());
        assert!(parse("space A 1/1 60 eolmax=4").is_err(), "an option that does not apply");
        assert!(parse("eol A 1/1 60").is_err(), "eol needs eolmax");
        assert!(parse("notchecked A").is_err(), "a reason is required");
        let r = parse("# c\nnotchecked N.1 stitch geometry, no stitches\nspace S 1/1 60 # tail").unwrap();
        let res = check(&lib(vec![bx(1, 1, 0, 0, 10, 10)]), None, &r).unwrap();
        assert_eq!((res[0].checked, res[0].why_not.as_deref()), (false, Some("stitch geometry, no stitches")));
        assert!(res[1].checked);
    }

    /// The results database: header, one block per checked rule with its count, an edge pair
    /// per spacing result; a rule not checked is absent.
    #[test]
    fn the_results_database_has_one_check_per_rule() {
        let r = parse("space S 1/1 60\nnotchecked N stated only").unwrap();
        let res = check(&lib(vec![bx(1, 1, 0, 0, 100, 30), bx(1, 1, 0, 80, 100, 110)]), None, &r).unwrap();
        let db = write_rdb(&res, "t", 1000);
        let lines: Vec<&str> = db.lines().collect();
        assert_eq!(&lines[..4], ["t 1000", "S", "1 1 1", "space S 60"]);
        assert_eq!(lines[4], "e 1 2");
        assert_eq!(lines.len(), 7);
        assert!(!db.contains("\nN\n"));
    }
}
