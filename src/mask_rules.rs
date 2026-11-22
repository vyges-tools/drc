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
//! space2      R.S.2         20/1 20/2 32                             [touch]
//! eol         R.S.4_A       20/1 70 eolmax=80 adjmin=20 ext=16       [lenlt=N | lenge=N] [to=L | toeol | toeol=L] [exclude=L | only=L]
//! oddcycle    R.colour      20/1 20/2 66
//! stitch      R.stitch      20/1 20/2
//! ratio       R.DEN.7       20/1 / 20/1 20/2 20/0   window=25000 step=12500 min=0.2   [exempt=L,L..] [inside=L] [minextent=D]
//! width       R.W.1_A       20/1 32
//! area        R.A.1         20/1 20/2 8800                           [shape=rect | shape=notrect]
//! hole        R.A.4         20/1 20/2 48000
//! shortvertex R.SE.1_A      20/1 57
//! orthogonal  R.C.1_A       20/1
//! rectangle   R.CF.C.1      20/53+20/54
//! size        R.CF.LW.1     20/53 20/54 280 280
//! widespace   R.WS.1        20/0 20/1 20/2 120 50                    [to=W] [run=R]
//! notchecked  R.C.10_A      why it is not checked, free text
//! ```
//!
//! What each kind measures, on the merged (unioned) shapes of each layer:
//! - `space L d`: two boundary edges of L facing each other across empty space closer than `d`,
//!   side to side (projections overlap) or corner to corner (Euclidean); a notch of one polygon
//!   counts. Edge-to-edge external spacing, the plain same-mask spacing rule.
//! - `space2 A B d`: the same between an edge of A and an edge of B, facing each other across
//!   space that is in neither layer. Shapes of A and B that touch or overlap (a stitch) are not a
//!   spacing case, unless `touch` is given: then each meeting is a result too (value 0).
//! - In `space` and `space2` a layer may be a union, written `L+L...`: the rule reads the merged
//!   union, so a union spacing counts pairs within each layer and across them.
//! - `eol L d`: a **line end** is a boundary edge no longer than `eolmax` whose two corners are
//!   convex and whose two neighbouring edges are at least `adjmin` long. Its window reaches `d`
//!   outward from the edge, widened by `ext` on each side; any shape of the target in the open
//!   window violates. The target is L itself, the layer `to=`, or (`toeol`, `toeol=L2`) another line end of
//!   L (of L2) facing it, itself widened by `ext`. `lenlt` / `lenge` keep only line ends shorter than /
//!   at least that long.
//! - `oddcycle A B d`: the shapes of A and B (each polygon a node; a polygon of A overlapping
//!   one of B is a stitch, not a constraint) joined wherever two are closer than `d`; an odd
//!   cycle cannot be two-coloured by any assignment. Each odd cycle found is one result.
//! - `stitch A B`: every place the two masks meet (overlap or abut). A stitch is legal only
//!   under the deck's stitch rules; this lists each one so none passes unseen.
//! - `ratio NUM... / DEN...`: colour balance. Over windows `window` square, stepped by `step`
//!   from the extent's lower left, the last row and column backed up to end on the extent (an
//!   extent smaller than a window is one window), the area of the union of the NUM layers divided
//!   by the area of the union of the DEN layers is below `min=` (or above `max=`). A window with
//!   no DEN area, or touching any `exempt=` layer, is not measured. The extent is the bounding box
//!   of the `inside=` layer, else of the whole layout; an extent narrower than `minextent=` on
//!   either axis is not measured at all. Each failing window is one result, its ratio in parts
//!   per million.
//! - `exclude=L` drops a result whose shapes both lie in layer L (touch it); `only=L` keeps only
//!   those. A region marker, read by interaction.
//! - `width L... d`: over the union of the layers, two boundary edges of one polygon facing each
//!   other across its inside (side to side), closer than `d`, the rectangle between them wholly
//!   inside: the minimum width.
//! - `area L... a [shape=rect|notrect]`: a merged polygon (tiles sharing an edge) whose area is
//!   below `a` (DB units squared); `shape=` keeps only rectangles or only other polygons.
//! - `hole L... a`: an enclosed hole whose area is below `a`.
//! - `shortvertex L... d`: a boundary vertex whose two edges are both shorter than `d`.
//! - `orthogonal L...`: a drawn polygon edge that is neither horizontal nor vertical.
//! - `widespace L... w d [to=w2] [run=r]`: the part of the union wider than `w` (an opening by a
//!   `w` square) facing the union, or with `to=` its part wider than `w2`,
//!   across space closer than `d`, side to side only, the shared run longer than `r` (default 0).
//!   `bridge`: the part of the run over metal joining the wide part to the other side's part
//!   beyond it (a neck) is dropped; what is left must be longer than `r`. No shielding then.
//!   `bridge2`: the same for metal touching exactly two wide parts (space and notch between them).
//!   `jog=J`: the measured pieces are summed per cluster (pieces meeting on one line, and lines
//!   joined by a step of the union no longer than `J` with exactly one convex end); a cluster
//!   longer than `r` is one result (each side of the gap counts on its own).
//! - `notch L... len side w`: a notch bottom (both corners concave) shorter than `len`, beside a
//!   notch side shorter than `side`, with less than `w` of metal behind it (inside, side to side).
//! - `rectangle L...`: a merged polygon (of the union) that is not a rectangle.
//! - `size L... w h`: a merged polygon that is not a `w` by `h` rectangle, either way round.
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
    /// a line end of the rule's layer, or (`toeol=L`) of layer L
    LineEnd(Option<Layer>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    Space { layers: Vec<Layer>, d: i64 },
    /// `touch`: shapes of A and B that meet (touch or overlap) are a result too (value 0)
    Space2 { a: Vec<Layer>, b: Vec<Layer>, d: i64, touch: bool },
    Eol { layer: Layer, d: i64, eol_max: i64, adj_min: i64, ext: i64, len_lt: Option<i64>, len_ge: Option<i64>, to: EolTarget },
    OddCycle { a: Layer, b: Layer, d: i64 },
    Stitch { a: Layer, b: Layer },
    Ratio { num: Vec<Layer>, den: Vec<Layer>, window: i64, step: i64, min: Option<i64>, max: Option<i64>, exempt: Vec<Layer>, inside: Option<Layer>, min_extent: i64 },
    /// Interior width below `d` (over the union of `layers`).
    Width { layers: Vec<Layer>, d: i64 },
    /// Polygon area below `a` (DB units squared), optionally only rectangles or only non-rectangles.
    Area { layers: Vec<Layer>, a: i64, shape: AreaShape },
    /// Enclosed hole area below `a`.
    Hole { layers: Vec<Layer>, a: i64 },
    /// A vertex whose two edges are both shorter than `d`.
    ShortVertex { layers: Vec<Layer>, d: i64 },
    /// A notch bottom (both corners concave) shorter than `len`, beside a notch side shorter than
    /// `side`, with less than `w` of metal behind it (inside, side to side).
    Notch { layers: Vec<Layer>, len: i64, side: i64, w: i64 },
    /// An edge that is neither horizontal nor vertical (on the drawn polygons).
    Orthogonal { layers: Vec<Layer> },
    Rectangle { layers: Vec<Layer> },
    Size { layers: Vec<Layer>, w: i64, h: i64 },
    /// The wide part (width > `w`) of the union facing the union (or, `to=`, its part wider than
    /// `to`) across space closer than `d`, side to side only, run length above `run`.
    /// `bridge`: the run over metal that touches both the wide part and the other side's part
    /// beyond it (a neck inside one polygon) is not space; `bridge2`: over metal that touches two
    /// wide parts.
    /// `jog`: the measured pieces are summed per cluster (one line, or lines joined by a step no
    /// longer than `jog`), and a cluster longer than `run` is one result.
    WideSpace { layers: Vec<Layer>, w: i64, d: i64, to: Option<i64>, run: i64, bridge: Bridge, jog: Option<i64> },
    NotChecked { why: String },
}

/// Which polygons an `area` rule measures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AreaShape {
    Any,
    Rect,
    NotRect,
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
            Kind::Stitch { .. } => "stitch",
            Kind::Ratio { .. } => "ratio",
            Kind::Width { .. } => "width",
            Kind::Area { .. } => "area",
            Kind::Hole { .. } => "hole",
            Kind::ShortVertex { .. } => "shortvertex",
            Kind::Notch { .. } => "notch",
            Kind::Orthogonal { .. } => "orthogonal",
            Kind::Rectangle { .. } => "rectangle",
            Kind::Size { .. } => "size",
            Kind::WideSpace { .. } => "widespace",
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
    /// A window and its ratio, parts per million.
    Window { rect: Rect, ppm: i64 },
    /// A polygon (or hole), by its bounding box, and the measured value (an area).
    Shape { rect: Rect, value: i64 },
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
    /// What the rule measured (windows for `ratio`, else 0): a clean `ratio` that measured no
    /// window is vacuous.
    pub measured: usize,
    /// For `ratio`: the lowest and highest ratio over the measured windows, ppm (how balanced,
    /// not only whether a bound broke).
    pub range_ppm: Option<(i64, i64)>,
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
        let (pos, opts): (Vec<&str>, Vec<&str>) = toks[2..].iter().partition(|t| !t.contains('=') && **t != "toeol" && **t != "touch" && **t != "bridge" && **t != "bridge2");
        let layer = |k: usize| pos.get(k).and_then(|s| Layer::parse(s)).ok_or_else(|| err("expected a layer"));
        // a layer, or the union of several joined by `+`
        let lset = |k: usize| -> Result<Vec<Layer>, String> {
            let s = pos.get(k).ok_or_else(|| err("expected a layer"))?;
            let mut v: Vec<Layer> = s.split('+').map(|t| Layer::parse(t).ok_or_else(|| err("expected a layer"))).collect::<Result<_, _>>()?;
            v.sort();
            v.dedup();
            Ok(v)
        };
        let num = |k: usize| pos.get(k).and_then(|s| s.parse::<i64>().ok()).filter(|v| *v > 0).ok_or_else(|| err("expected a positive DB-unit value"));
        let mut region = Region::All;
        let mut kv: BTreeMap<&str, &str> = BTreeMap::new();
        let mut toeol = false;
        let mut touch = false;
        let mut bridge = Bridge::None;
        for o in &opts {
            if *o == "bridge" || *o == "bridge2" {
                if bridge != Bridge::None {
                    return Err(err("one bridge option per rule"));
                }
                bridge = if *o == "bridge" { Bridge::Beyond } else { Bridge::Two };
                continue;
            }
            if *o == "toeol" {
                toeol = true;
                continue;
            }
            if *o == "touch" {
                touch = true;
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
                Kind::Space { layers: lset(0)?, d: num(1)? }
            }
            "space2" => {
                want(3)?;
                Kind::Space2 { a: lset(0)?, b: lset(1)?, d: num(2)?, touch }
            }
            "oddcycle" => {
                want(3)?;
                Kind::OddCycle { a: layer(0)?, b: layer(1)?, d: num(2)? }
            }
            "stitch" => {
                want(2)?;
                Kind::Stitch { a: layer(0)?, b: layer(1)? }
            }
            "ratio" => {
                let slash = pos.iter().position(|t| *t == "/").ok_or_else(|| err("ratio needs NUM... / DEN..."))?;
                let lay = |v: &[&str]| -> Result<Vec<Layer>, String> {
                    v.iter().map(|s| Layer::parse(s).ok_or_else(|| err("expected a layer"))).collect()
                };
                let (num, den) = (lay(&pos[..slash])?, lay(&pos[slash + 1..])?);
                if num.is_empty() || den.is_empty() {
                    return Err(err("ratio needs a layer on each side of /"));
                }
                // the bound is held in parts per million so the comparison is exact integer arithmetic
                let frac = |k: &str| -> Result<Option<i64>, String> {
                    kv.get(k)
                        .map(|v| {
                            v.parse::<f64>().ok().filter(|x| (0.0..=1.0).contains(x)).map(|x| (x * 1e6).round() as i64).ok_or_else(|| err("min=/max= is a fraction 0..1"))
                        })
                        .transpose()
                };
                let (min, max) = (frac("min")?, frac("max")?);
                if min.is_some() == max.is_some() {
                    return Err(err("ratio takes one of min= or max="));
                }
                let window = opt("window")?.filter(|v| *v > 0).ok_or_else(|| err("ratio needs window= > 0"))?;
                let step = opt("step")?.filter(|v| *v > 0).ok_or_else(|| err("ratio needs step= > 0"))?;
                let l = |k: &str| -> Result<Option<Layer>, String> {
                    kv.get(k).map(|v| Layer::parse(v).ok_or_else(|| err("bad layer option"))).transpose()
                };
                let exempt = match kv.get("exempt") {
                    Some(v) => v.split(',').map(|s| Layer::parse(s).ok_or_else(|| err("bad exempt= layer"))).collect::<Result<Vec<_>, _>>()?,
                    None => Vec::new(),
                };
                let min_extent = opt("minextent")?.unwrap_or(0);
                Kind::Ratio { num, den, window, step, min, max, exempt, inside: l("inside")?, min_extent }
            }
            "notch" => {
                if pos.len() < 4 {
                    return Err(err("wrong number of arguments"));
                }
                let n = pos.len();
                let layers: Vec<Layer> = pos[..n - 3].iter().map(|s| Layer::parse(s).ok_or_else(|| err("expected a layer"))).collect::<Result<_, _>>()?;
                Kind::Notch { layers, len: num(n - 3)?, side: num(n - 2)?, w: num(n - 1)? }
            }
            "widespace" => {
                if pos.len() < 3 {
                    return Err(err("wrong number of arguments"));
                }
                let layers: Vec<Layer> = pos[..pos.len() - 2].iter().map(|s| Layer::parse(s).ok_or_else(|| err("expected a layer"))).collect::<Result<_, _>>()?;
                let (w, d) = (num(pos.len() - 2)?, num(pos.len() - 1)?);
                let to = opt("to")?;
                if w <= 0 || to.is_some_and(|t| t <= 0) {
                    return Err(err("widespace widths must be positive"));
                }
                Kind::WideSpace { layers, w, d, to, run: opt("run")?.unwrap_or(0), bridge, jog: opt("jog")? }
            }
            "width" | "area" | "hole" | "shortvertex" | "orthogonal" | "rectangle" | "size" => {
                // layers (their union), then the value; `orthogonal` and `rectangle` take none, `size` two
                let nv = match kind {
                    "orthogonal" | "rectangle" => 0,
                    "size" => 2,
                    _ => 1,
                };
                if pos.len() < 1 + nv {
                    return Err(err("wrong number of arguments"));
                }
                let layers: Vec<Layer> = pos[..pos.len() - nv].iter().map(|s| Layer::parse(s).ok_or_else(|| err("expected a layer"))).collect::<Result<_, _>>()?;
                let v = if nv >= 1 { num(pos.len() - 1)? } else { 0 };
                match kind {
                    "rectangle" => Kind::Rectangle { layers },
                    "size" => Kind::Size { layers, w: num(pos.len() - 2)?, h: v },
                    "width" => Kind::Width { layers, d: v },
                    "hole" => Kind::Hole { layers, a: v },
                    "shortvertex" => Kind::ShortVertex { layers, d: v },
                    "orthogonal" => Kind::Orthogonal { layers },
                    _ => {
                        let shape = match kv.get("shape").copied() {
                            None | Some("any") => AreaShape::Any,
                            Some("rect") => AreaShape::Rect,
                            Some("notrect") => AreaShape::NotRect,
                            Some(_) => return Err(err("shape= is any, rect or notrect")),
                        };
                        Kind::Area { layers, a: v, shape }
                    }
                }
            }
            "eol" => {
                want(2)?;
                let toeol_l = kv.get("toeol").map(|v| Layer::parse(v).ok_or_else(|| err("bad toeol= layer"))).transpose()?;
                let to = match (kv.get("to"), toeol || toeol_l.is_some()) {
                    (Some(_), true) => return Err(err("to= and toeol together")),
                    (Some(v), false) => EolTarget::Layer(Layer::parse(v).ok_or_else(|| err("bad to= layer"))?),
                    (None, true) => EolTarget::LineEnd(toeol_l),
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
        let allowed: &[&str] = match kind {
            "eol" => &["eolmax", "adjmin", "ext", "lenlt", "lenge", "to", "toeol"],
            "ratio" => &["window", "step", "min", "max", "exempt", "inside", "minextent"],
            "area" => &["shape"],
            "widespace" => &["to", "run", "jog"],
            _ => &[],
        };
        if let Some(k) = kv.keys().find(|k| !allowed.contains(k)) {
            return Err(err(&format!("option {k}= does not apply")));
        }
        if toeol && kind != "eol" {
            return Err(err("toeol applies to eol only"));
        }
        if bridge != Bridge::None && kind != "widespace" {
            return Err(err("bridge applies to widespace only"));
        }
        if touch && kind != "space2" {
            return Err(err("touch applies to space2 only"));
        }
        out.push(MaskRule { name, kind: rule, region });
    }
    Ok(out)
}

/// One layer's merged geometry: the tiles of its union, their index, and its boundary rings.
/// Which metal inside a widespace gap is a bridge (not space).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bridge {
    None,
    /// touches the wide part and the other side's part outside it
    Beyond,
    /// touches exactly two wide parts
    Two,
}

struct Merged {
    tiles: Vec<Rect>,
    idx: RegionIndex,
    rings: Vec<Vec<Edge>>,
}

impl Merged {
    fn from_tiles(tiles: &[Rect]) -> Merged {
        let tiles = boolean::boolean(tiles, &[], Op::Or);
        let idx = RegionIndex::build(&tiles);
        let rings = trace_contours(&tiles).iter().map(|r| ring_edges(r)).collect();
        Merged { tiles, idx, rings }
    }

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

    /// Does the closed rectangle `q` meet the region, an edge or a corner in common enough? (The
    /// index query is widened by one unit: it only returns tiles that overlap with area.)
    fn meets(&self, q: &Rect) -> bool {
        self.idx.overlaps(&grown(q, 1)).into_iter().any(|i| {
            let t = self.tiles[i as usize];
            t.x0 <= q.x1 && q.x0 <= t.x1 && t.y0 <= q.y1 && q.y0 <= t.y1
        })
    }

    /// Does the closed rectangle `q` touch the region?
    fn touches(&self, q: &Rect) -> bool {
        self.idx.overlaps(q).into_iter().any(|i| {
            let t = self.tiles[i as usize];
            t.x0 <= q.x1 && q.x0 <= t.x1 && t.y0 <= q.y1 && q.y0 <= t.y1
        })
    }
}

fn grown(t: &Rect, r: i32) -> Rect {
    Rect { x0: t.x0 - r, y0: t.y0 - r, x1: t.x1 + r, y1: t.y1 + r }
}

/// The part of the region wider than `w`: the union of the `w` x `w` squares inside it that do not
/// degenerate (an opening by a square). Erode by the square `[0, w]^2` (a point stays when the
/// square at it fits), dilate back by the same square, within the region. A strip exactly `w`
/// wide erodes to a line, which has no area, so it is not wide; one DB unit wider is. The one-sided
/// square makes any whole `w` work, odd or even. This is the wide part of a polygon, not the whole
/// polygon that has one.
fn wide_part(m: &Merged, w: i64) -> Merged {
    let r = w as i32;
    let Some(bb) = bbox_of(&m.tiles) else { return Merged::from_tiles(&[]) };
    let outside = boolean::boolean(&[grown(&bb, r + 1)], &m.tiles, Op::Not);
    // a point is lost when some outside point lies in its square: grow the outside left and down
    let reach: Vec<Rect> = outside.iter().map(|t| Rect { x0: t.x0 - r, y0: t.y0 - r, x1: t.x1, y1: t.y1 }).collect();
    let core = boolean::boolean(&m.tiles, &reach, Op::Not);
    let back: Vec<Rect> = core.iter().map(|t| Rect { x0: t.x0, y0: t.y0, x1: t.x1 + r, y1: t.y1 + r }).collect();
    Merged::from_tiles(&boolean::boolean(&back, &m.tiles, Op::And))
}

/// Side-to-side pairs of the wide part `a` and `b` closer than `d`, no shielding, where the gap
/// is checked edge by edge: the part of the shared run under a bridge (a piece of the union `m`
/// inside the gap that touches `a` and touches `b` outside `a`, or with `Two` that touches two
/// polygons of `a`) is dropped, what is left of the run must be longer than `run`. Any other
/// metal in the gap does not hide the pair.
fn bridged_pairs(a: &Merged, b: &Merged, m: &Merged, d: i64, run: i64, mode: Bridge, jog: Option<i64>) -> Vec<Marker> {
    let mut pieces: Vec<Piece> = Vec::new();
    let b_not_a = Merged::from_tiles(&boolean::boolean(&b.tiles, &a.tiles, Op::Not));
    // the wide polygon each tile of `a` belongs to, for `Two`
    let mut part_of = vec![0usize; a.tiles.len()];
    for (k, comp) in components(a).iter().enumerate() {
        for &i in comp {
            part_of[i] = k;
        }
    }
    let parts_met = |r: &Rect| -> BTreeSet<usize> {
        a.idx.overlaps(&grown(r, 1)).into_iter().map(|i| i as usize).filter(|&i| {
            let t = a.tiles[i];
            t.x0 <= r.x1 && r.x0 <= t.x1 && t.y0 <= r.y1 && r.y0 <= t.y1
        }).map(|i| part_of[i]).collect()
    };
    let eb = b.edges();
    let boxes: Vec<Rect> = eb.iter().map(bbox_edge).collect();
    let idx = RegionIndex::build(&boxes);
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    for ea in a.edges() {
        for j in idx.within(&bbox_edge(&ea), d as i32, None) {
            let eb = &eb[j as usize];
            if ea.axis() != eb.axis() || !projecting_edges(&ea, eb) {
                continue;
            }
            let (ra, rb) = (bbox_edge(&ea), bbox_edge(eb));
            let horiz = ea.axis() == Axis::Horizontal;
            let (pa, pb) = if horiz { (ra.y0, rb.y0) } else { (ra.x0, rb.x0) };
            let gap = i64::from((pb - pa).abs());
            if gap == 0 || gap >= d {
                continue;
            }
            let dir = if horiz { (0, (pb - pa).signum() as i64) } else { ((pb - pa).signum() as i64, 0) };
            let (oa, ob) = (outward_edge(&ea), outward_edge(eb));
            if oa.0 * dir.0 + oa.1 * dir.1 <= 0 || ob.0 * dir.0 + ob.1 * dir.1 >= 0 {
                continue;
            }
            let (lo, hi) = if horiz { (ra.x0.max(rb.x0), ra.x1.min(rb.x1)) } else { (ra.y0.max(rb.y0), ra.y1.min(rb.y1)) };
            let g = if horiz {
                Rect { x0: lo, y0: pa.min(pb), x1: hi, y1: pa.max(pb) }
            } else {
                Rect { x0: pa.min(pb), y0: lo, x1: pa.max(pb), y1: hi }
            };
            // only the tiles near the gap: the index, not the whole union, per pair
            let near: Vec<Rect> = m.idx.overlaps(&grown(&g, 1)).into_iter().map(|i| m.tiles[i as usize]).collect();
            let inside = Merged::from_tiles(&boolean::boolean(&near, &[g], Op::And));
            let mut cut: Vec<(i32, i32)> = Vec::new();
            for comp in components(&inside) {
                let t: Vec<Rect> = comp.iter().map(|&i| inside.tiles[i]).collect();
                let is_bridge = match mode {
                    Bridge::None => false,
                    Bridge::Beyond => t.iter().any(|r| a.meets(r)) && t.iter().any(|r| b_not_a.meets(r)),
                    Bridge::Two => t.iter().flat_map(&parts_met).collect::<BTreeSet<usize>>().len() == 2,
                };
                if is_bridge {
                    let bb = bbox_of(&t).expect("a component has tiles");
                    cut.push(if horiz { (bb.x0, bb.x1) } else { (bb.y0, bb.y1) });
                }
            }
            cut.sort_unstable();
            // the stretches of the shared run left uncovered, and the longest
            let (mut at, mut best, mut left) = (lo, 0i64, Vec::new());
            for (c0, c1) in cut {
                if c0 > at {
                    left.push((at, c0));
                }
                best = best.max(i64::from(c0 - at));
                at = at.max(c1);
            }
            if hi > at {
                left.push((at, hi));
            }
            best = best.max(i64::from(hi - at));
            if jog.is_some() {
                for &(l0, l1) in &left {
                    for line in [pa, pb] {
                        pieces.push(Piece { horiz, line, lo: l0, hi: l1, gap });
                    }
                }
                continue;
            }
            if best <= run {
                continue;
            }
            let k = {
                let (x, y) = (seg(&ea), seg(eb));
                if x <= y { (x, y) } else { (y, x) }
            };
            if seen.insert(k) {
                out.push(Marker::Edges { a: seg(&ea), b: seg(eb), value: gap });
            }
        }
    }
    match jog {
        Some(j) => clusters(pieces, m, j, run),
        None => out,
    }
}

/// One measured stretch of an edge: on the line `line` (y for a horizontal edge, x for a
/// vertical one), from `lo` to `hi`, at distance `gap` from its partner.
#[derive(Debug, Clone, Copy)]
struct Piece {
    horiz: bool,
    line: i32,
    lo: i32,
    hi: i32,
    gap: i64,
}

/// Pieces summed per cluster. Pieces on one line that meet are one stretch; stretches are joined
/// by a step of the union `m`: an edge across them no longer than `jog` with exactly one convex
/// end, meeting exactly two stretches at its ends. A cluster longer than `run` is one result.
fn clusters(pieces: Vec<Piece>, m: &Merged, jog: i64, run: i64) -> Vec<Marker> {
    // stretches per line
    // (horizontal, line) -> (lo, hi, gap)
    type Span = (i32, i32, i64);
    let mut by_line: BTreeMap<(bool, i32), Vec<Span>> = BTreeMap::new();
    for p in &pieces {
        by_line.entry((p.horiz, p.line)).or_default().push((p.lo, p.hi, p.gap));
    }
    let mut st: Vec<(bool, i32, i32, i32, i64)> = Vec::new(); // horiz, line, lo, hi, min gap
    for ((h, line), mut v) in by_line {
        v.sort_unstable();
        let mut cur: Option<(i32, i32, i64)> = None;
        for (lo, hi, g) in v {
            cur = match cur {
                Some((c0, c1, cg)) if lo <= c1 => Some((c0, c1.max(hi), cg.min(g))),
                Some((c0, c1, cg)) => {
                    st.push((h, line, c0, c1, cg));
                    Some((lo, hi, g))
                }
                None => Some((lo, hi, g)),
            };
        }
        if let Some((c0, c1, cg)) = cur {
            st.push((h, line, c0, c1, cg));
        }
    }
    let mut p: Vec<usize> = (0..st.len()).collect();
    fn find(p: &mut [usize], mut x: usize) -> usize {
        while p[x] != x {
            p[x] = p[p[x]];
            x = p[x];
        }
        x
    }
    let meets = |h: bool, pt: (i32, i32)| -> Vec<usize> {
        let (line, at) = if h { (pt.1, pt.0) } else { (pt.0, pt.1) };
        st.iter().enumerate().filter(|(_, s)| s.0 == h && s.1 == line && s.2 <= at && at <= s.3).map(|(i, _)| i).collect()
    };
    let cross = |e: &Edge, f: &Edge| -> i64 {
        let (ux, uy) = (i64::from(e.b.0 - e.a.0), i64::from(e.b.1 - e.a.1));
        let (vx, vy) = (i64::from(f.b.0 - f.a.0), i64::from(f.b.1 - f.a.1));
        ux * vy - uy * vx
    };
    for ring in &m.rings {
        let n = ring.len();
        for i in 0..n {
            let e = &ring[i];
            let len = i64::from((e.b.0 - e.a.0).abs() + (e.b.1 - e.a.1).abs());
            if len > jog {
                continue;
            }
            // a convex corner turns left (metal on the left of the ring)
            let convex = [cross(&ring[(i + n - 1) % n], e) > 0, cross(e, &ring[(i + 1) % n]) > 0];
            if convex[0] == convex[1] {
                continue;
            }
            // a vertical step joins horizontal stretches, and the other way round
            let h = e.axis() != Axis::Horizontal;
            let mut ids: Vec<usize> = meets(h, e.a).into_iter().chain(meets(h, e.b)).collect();
            ids.sort_unstable();
            ids.dedup();
            if ids.len() == 2 {
                let (ra, rb) = (find(&mut p, ids[0]), find(&mut p, ids[1]));
                p[ra.max(rb)] = ra.min(rb);
            }
        }
    }
    let mut groups: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for i in 0..st.len() {
        let r = find(&mut p, i);
        groups.entry(r).or_default().push(i);
    }
    let mut out = Vec::new();
    for g in groups.into_values() {
        let total: i64 = g.iter().map(|&i| i64::from(st[i].3 - st[i].2)).sum();
        if total <= run {
            continue;
        }
        let rect = g.iter().map(|&i| {
            let (h, line, lo, hi, _) = st[i];
            if h { Rect { x0: lo, y0: line, x1: hi, y1: line } } else { Rect { x0: line, y0: lo, x1: line, y1: hi } }
        }).reduce(|x, y| Rect { x0: x.x0.min(y.x0), y0: x.y0.min(y.y0), x1: x.x1.max(y.x1), y1: x.y1.max(y.y1) }).expect("a group has stretches");
        out.push(Marker::Shape { rect, value: total });
    }
    out
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
/// `run`: side to side only (projecting), with the shared run longer than this.
fn ext_pairs(ea: &[Edge], eb: &[Edge], d: i64, ma: &Merged, mb: &Merged, run: Option<i64>) -> Vec<Marker> {
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
            if let Some(r) = run {
                if !projecting_edges(a, b) {
                    continue;
                }
                let (ra, rb) = (bbox_edge(a), bbox_edge(b));
                let shared = if a.axis() == Axis::Horizontal { ra.x1.min(rb.x1) - ra.x0.max(rb.x0) } else { ra.y1.min(rb.y1) - ra.y0.max(rb.y0) };
                if i64::from(shared) <= r {
                    continue;
                }
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

/// Where the two masks meet: each connected piece of A AND B (an overlap), and each abutment
/// of an A edge on a B edge (a zero-width meeting), as one polygon result.
fn stitches(ma: &Merged, mb: &Merged) -> Vec<Marker> {
    let mut boxes: Vec<Rect> = Vec::new();
    for t in &ma.tiles {
        for j in mb.idx.within(t, 1, None) {
            let u = &mb.tiles[j as usize];
            let r = Rect { x0: t.x0.max(u.x0), y0: t.y0.max(u.y0), x1: t.x1.min(u.x1), y1: t.y1.min(u.y1) };
            // overlap, or a shared edge of positive length
            if r.x0 <= r.x1 && r.y0 <= r.y1 && (r.x0 < r.x1 || r.y0 < r.y1) {
                boxes.push(r);
            }
        }
    }
    // pieces that touch are one meeting
    boxes.sort_by_key(|b| (b.x0, b.y0, b.x1, b.y1));
    boxes.dedup();
    let mut p: Vec<usize> = (0..boxes.len()).collect();
    fn find(p: &mut [usize], mut x: usize) -> usize {
        while p[x] != x {
            p[x] = p[p[x]];
            x = p[x];
        }
        x
    }
    let idx = RegionIndex::build(&boxes.iter().map(|b| b.inflate(1)).collect::<Vec<_>>());
    for (i, b) in boxes.iter().enumerate() {
        for j in idx.overlaps(&b.inflate(1)) {
            let c = &boxes[j as usize];
            if b.x0 <= c.x1 && c.x0 <= b.x1 && b.y0 <= c.y1 && c.y0 <= b.y1 {
                let (x, y) = (find(&mut p, i), find(&mut p, j as usize));
                p[x.max(y)] = x.min(y);
            }
        }
    }
    let mut groups: BTreeMap<usize, Rect> = BTreeMap::new();
    for (i, b) in boxes.iter().enumerate() {
        let r = find(&mut p, i);
        let g = groups.entry(r).or_insert(*b);
        *g = Rect { x0: g.x0.min(b.x0), y0: g.y0.min(b.y0), x1: g.x1.max(b.x1), y1: g.y1.max(b.y1) };
    }
    groups.into_values().map(|b| Marker::Cycle { boxes: vec![b] }).collect()
}

/// Run the rule file over the flattened top cell.
pub fn check(lib: &Library, top: Option<&str>, rules: &[MaskRule]) -> Result<Vec<RuleResult>, String> {
    let top = pick_top(lib, top)?;
    let cell = flatten::flatten(lib, &top)?;
    let mut want: BTreeSet<Layer> = BTreeSet::new();
    for r in rules {
        match &r.kind {
            Kind::Space { layers, .. } => {
                want.extend(layers.iter().copied());
            }
            Kind::Space2 { a, b, .. } => {
                want.extend(a.iter().chain(b).copied());
            }
            Kind::OddCycle { a, b, .. } | Kind::Stitch { a, b } => {
                want.insert(*a);
                want.insert(*b);
            }
            Kind::Eol { layer, to, .. } => {
                want.insert(*layer);
                if let EolTarget::Layer(l) | EolTarget::LineEnd(Some(l)) = to {
                    want.insert(*l);
                }
            }
            Kind::Ratio { num, den, exempt, inside, .. } => {
                want.extend(num.iter().chain(den).chain(exempt).copied());
                want.extend(inside.iter().copied());
            }
            Kind::Width { layers, .. } | Kind::Area { layers, .. } | Kind::Hole { layers, .. } | Kind::ShortVertex { layers, .. } | Kind::Notch { layers, .. } | Kind::Orthogonal { layers } | Kind::Rectangle { layers } | Kind::Size { layers, .. } | Kind::WideSpace { layers, .. } => {
                want.extend(layers.iter().copied());
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
    // the drawn outlines as written (the merge squares off a non-orthogonal polygon), for `orthogonal`
    let mut drawn: BTreeMap<Layer, Vec<Vec<(i32, i32)>>> = BTreeMap::new();
    for e in &cell.elements {
        if let crate::layout::gds::Element::Boundary { layer, datatype, pts } = e {
            let l = Layer::new(*layer, *datatype);
            if want.contains(&l) {
                drawn.entry(l).or_default().push(pts.clone());
            }
        }
    }
    let merged: BTreeMap<Layer, Merged> = want.iter().map(|l| (*l, Merged::new(polys.get(l).map_or(&[][..], |v| v)))).collect();
    let mut out = Vec::new();
    // unions shared between rules (a balance rule pair, min and max, reads the same two)
    let mut unions: BTreeMap<Vec<Layer>, Merged> = BTreeMap::new();
    let union_of = |ls: &[Layer]| {
        let v: Vec<Vec<(i32, i32)>> = ls.iter().flat_map(|l| polys.get(l).cloned().unwrap_or_default()).collect();
        Merged::new(&v)
    };
    for r in rules {
        let sets: Vec<&Vec<Layer>> = match &r.kind {
            Kind::Space { layers, .. } => vec![layers],
            Kind::Space2 { a, b, .. } => vec![a, b],
            _ => vec![],
        };
        for ls in sets.into_iter().filter(|ls| ls.len() > 1) {
            if !unions.contains_key(ls) {
                unions.insert(ls.clone(), union_of(ls));
            }
        }
    }
    for r in rules {
        let (limit, markers) = match &r.kind {
            Kind::NotChecked { why } => {
                out.push(RuleResult { name: r.name.clone(), kind: r.kind_name(), limit: 0, checked: false, why_not: Some(why.clone()), markers: vec![], measured: 0, range_ppm: None });
                continue;
            }
            Kind::Ratio { num, den, window, step, min, max, exempt, inside, min_extent } => {
                for ls in [num, den, exempt] {
                    unions.entry(ls.clone()).or_insert_with(|| {
                        let v: Vec<Vec<(i32, i32)>> = ls.iter().flat_map(|l| polys.get(l).cloned().unwrap_or_default()).collect();
                        Merged::new(&v)
                    });
                }
                let (mn, md, mx) = (&unions[num], &unions[den], &unions[exempt]);
                let ext = match inside {
                    Some(l) => bbox_of(&merged[l].tiles),
                    None => cell_bbox(&cell),
                };
                let wide = |e: &Rect| i64::from(e.x1) - i64::from(e.x0) >= *min_extent && i64::from(e.y1) - i64::from(e.y0) >= *min_extent;
                let (markers, measured, range_ppm) = match ext.filter(wide) {
                    None => (Vec::new(), 0, None),
                    Some(ext) => ratio_windows(mn, md, (!exempt.is_empty()).then_some(mx), ext, *window, *step, *min, *max),
                };
                let limit = min.or(*max).unwrap_or(0);
                out.push(RuleResult { name: r.name.clone(), kind: r.kind_name(), limit, checked: true, why_not: None, markers, measured, range_ppm });
                continue;
            }
            Kind::Width { layers, .. } | Kind::Area { layers, .. } | Kind::Hole { layers, .. } | Kind::ShortVertex { layers, .. } | Kind::Notch { layers, .. } | Kind::Orthogonal { layers } | Kind::Rectangle { layers } | Kind::Size { layers, .. } | Kind::WideSpace { layers, .. } => {
                let m: &Merged = if layers.len() > 1 {
                    unions.entry(layers.clone()).or_insert_with(|| {
                        let v: Vec<Vec<(i32, i32)>> = layers.iter().flat_map(|l| polys.get(l).cloned().unwrap_or_default()).collect();
                        Merged::new(&v)
                    })
                } else {
                    &merged[&layers[0]]
                };
                shape_rule(&r.kind, m, layers, &drawn)
            }
            Kind::Space { layers, d } => {
                let m = if layers.len() > 1 { &unions[layers] } else { &merged[&layers[0]] };
                let e = m.edges();
                (*d, ext_pairs(&e, &e, *d, m, m, None))
            }
            Kind::Space2 { a, b, d, touch } => {
                let ma = if a.len() > 1 { &unions[a] } else { &merged[&a[0]] };
                let mb = if b.len() > 1 { &unions[b] } else { &merged[&b[0]] };
                let mut found = ext_pairs(&ma.edges(), &mb.edges(), *d, ma, mb, None);
                if *touch {
                    found.extend(stitches(ma, mb));
                }
                (*d, found)
            }
            Kind::OddCycle { a, b, d } => (*d, odd_cycles(&merged[a], &merged[b], *d)),
            Kind::Stitch { a, b } => (0, stitches(&merged[a], &merged[b])),
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
                            // a target that covers or abuts the end itself is the two masks meeting
                            // (a stitch), not a spacing: external spacing measures across space
                            // only, and the `stitch` kind reports the meeting
                            if matches!(to, EolTarget::Layer(_)) && t.touches(&bbox_edge(e)) {
                                continue;
                            }
                            if let Some(hit) = t.overlaps_open(&eol_window(e, *d, *ext)) {
                                let near = hit.iter().min_by_key(|h| (eol_gap(e, h), h.x0, h.y0)).expect("hit");
                                found.push(Marker::Edges { a: seg(e), b: [near.x0, near.y0, near.x1, near.y1], value: eol_gap(e, near) });
                            }
                        }
                    }
                    EolTarget::LineEnd(other) => {
                        let all = line_ends(other.map_or(m, |l| &merged[&l]), *eol_max, *adj_min);
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
                            Marker::Window { rect, .. } | Marker::Shape { rect, .. } => m.touches(rect),
                        };
                        inside == keep_inside
                    })
                    .collect()
            }
        };
        out.push(RuleResult { name: r.name.clone(), kind: r.kind_name(), limit, checked: true, why_not: None, markers, measured: 0, range_ppm: None });
    }
    Ok(out)
}

/// The shape rules (`width`, `area`, `hole`, `shortvertex`, `orthogonal`) on merged region `m`
/// (the union of `layers`); `polys` holds the drawn polygons, for `orthogonal`.
fn shape_rule(kind: &Kind, m: &Merged, layers: &[Layer], polys: &BTreeMap<Layer, Vec<Vec<(i32, i32)>>>) -> (i64, Vec<Marker>) {
    match kind {
        Kind::Width { d, .. } => (*d, narrow(m, *d)),
        Kind::Area { a, shape, .. } => (*a, small_polygons(m, *a, *shape)),
        Kind::Hole { a, .. } => (*a, small_holes(m, *a)),
        Kind::ShortVertex { d, .. } => (*d, short_vertices(m, *d)),
        Kind::Notch { len, side, w, .. } => (*w, thin_notches(m, *len, *side, *w)),
        Kind::Orthogonal { .. } => (0, off_axis(layers, polys)),
        Kind::Rectangle { .. } => (0, not_sized(m, None)),
        Kind::Size { w, h, .. } => (*w, not_sized(m, Some((*w, *h)))),
        Kind::WideSpace { w, d, to, run, bridge, jog, .. } => {
            let a = wide_part(m, *w);
            let other;
            let b: &Merged = match to {
                Some(t) => {
                    other = wide_part(m, *t);
                    &other
                }
                None => m,
            };
            if *bridge != Bridge::None || jog.is_some() {
                (*d, bridged_pairs(&a, b, m, *d, *run, *bridge, *jog))
            } else {
                (*d, ext_pairs(&a.edges(), &b.edges(), *d, &a, b, Some(*run)))
            }
        }
        _ => (0, Vec::new()),
    }
}

/// Interior width below `d`: two boundary edges of one region facing each other across its
/// inside (parallel, opposite outward normals, overlapping along their length), closer than `d`,
/// with the rectangle between them wholly inside the region. The projecting (side to side) form.
fn narrow(m: &Merged, d: i64) -> Vec<Marker> {
    let edges = m.edges();
    let boxes: Vec<Rect> = edges.iter().map(bbox_edge).collect();
    let idx = RegionIndex::build(&boxes);
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    for (i, a) in edges.iter().enumerate() {
        let (ox, oy) = outward_edge(a);
        for j in idx.within(&bbox_edge(a), d as i32, None) {
            let b = &edges[j as usize];
            if j as usize == i || b.axis() != a.axis() || outward_edge(b) != (-ox, -oy) || !projecting_edges(a, b) {
                continue;
            }
            let (ba, bb) = (bbox_edge(a), bbox_edge(b));
            // b lies on a's inner side: against a's outward normal
            let (gap, between) = if a.axis() == Axis::Horizontal {
                let g = if oy > 0 { i64::from(ba.y0) - i64::from(bb.y0) } else { i64::from(bb.y0) - i64::from(ba.y0) };
                let (x0, x1) = (ba.x0.max(bb.x0), ba.x1.min(bb.x1));
                (g, Rect { x0, y0: ba.y0.min(bb.y0), x1, y1: ba.y0.max(bb.y0) })
            } else {
                let g = if ox > 0 { i64::from(ba.x0) - i64::from(bb.x0) } else { i64::from(bb.x0) - i64::from(ba.x0) };
                let (y0, y1) = (ba.y0.max(bb.y0), ba.y1.min(bb.y1));
                (g, Rect { x0: ba.x0.min(bb.x0), y0, x1: ba.x0.max(bb.x0), y1 })
            };
            if gap <= 0 || gap >= d {
                continue;
            }
            let want = i64::from(between.x1 - between.x0) * i64::from(between.y1 - between.y0);
            let have: i64 = m
                .overlaps_open(&between)
                .unwrap_or_default()
                .iter()
                .map(|t| {
                    let dx = i64::from(t.x1.min(between.x1)) - i64::from(t.x0.max(between.x0));
                    let dy = i64::from(t.y1.min(between.y1)) - i64::from(t.y0.max(between.y0));
                    dx.max(0) * dy.max(0)
                })
                .sum();
            if have != want {
                continue;
            }
            let key = if i < j as usize { (i, j as usize) } else { (j as usize, i) };
            if seen.insert(key) {
                out.push(Marker::Edges { a: seg(a), b: seg(b), value: gap });
            }
        }
    }
    out
}

/// The polygons of a merged region: groups of its tiles that share an edge of positive length.
fn components(m: &Merged) -> Vec<Vec<usize>> {
    fn find(p: &mut [usize], mut x: usize) -> usize {
        while p[x] != x {
            p[x] = p[p[x]];
            x = p[x];
        }
        x
    }
    let n = m.tiles.len();
    let mut p: Vec<usize> = (0..n).collect();
    for i in 0..n {
        let a = m.tiles[i];
        for j in m.idx.within(&a, 1, None) {
            let j = j as usize;
            if j <= i {
                continue;
            }
            let b = m.tiles[j];
            let ox = i64::from(a.x1.min(b.x1)) - i64::from(a.x0.max(b.x0));
            let oy = i64::from(a.y1.min(b.y1)) - i64::from(a.y0.max(b.y0));
            if ox >= 0 && oy >= 0 && (ox > 0 || oy > 0) {
                let (ra, rb) = (find(&mut p, i), find(&mut p, j));
                p[ra.max(rb)] = ra.min(rb);
            }
        }
    }
    let mut groups: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for i in 0..n {
        let r = find(&mut p, i);
        groups.entry(r).or_default().push(i);
    }
    groups.into_values().collect()
}

/// Polygons whose area is below `a`; `shape` keeps only rectangles or only non-rectangles.
fn small_polygons(m: &Merged, a: i64, shape: AreaShape) -> Vec<Marker> {
    let mut out = Vec::new();
    for g in components(m) {
        let area: i64 = g.iter().map(|&i| {
            let t = m.tiles[i];
            i64::from(t.x1 - t.x0) * i64::from(t.y1 - t.y0)
        }).sum();
        let bb = bbox_of(&g.iter().map(|&i| m.tiles[i]).collect::<Vec<_>>()).expect("a component has a tile");
        let is_rect = area == i64::from(bb.x1 - bb.x0) * i64::from(bb.y1 - bb.y0);
        let keep = match shape {
            AreaShape::Any => true,
            AreaShape::Rect => is_rect,
            AreaShape::NotRect => !is_rect,
        };
        if keep && area < a {
            out.push(Marker::Shape { rect: bb, value: area });
        }
    }
    out
}

/// Merged polygons that are not a rectangle, or (`size`) not a `w` by `h` rectangle in either
/// orientation. The marker is the polygon's bounding box, its value the polygon's area.
fn not_sized(m: &Merged, size: Option<(i64, i64)>) -> Vec<Marker> {
    let mut out = Vec::new();
    for g in components(m) {
        let area: i64 = g.iter().map(|&i| {
            let t = m.tiles[i];
            i64::from(t.x1 - t.x0) * i64::from(t.y1 - t.y0)
        }).sum();
        let bb = bbox_of(&g.iter().map(|&i| m.tiles[i]).collect::<Vec<_>>()).expect("a component has a tile");
        let (bw, bh) = (i64::from(bb.x1 - bb.x0), i64::from(bb.y1 - bb.y0));
        let ok = area == bw * bh && size.is_none_or(|(w, h)| (bw, bh) == (w, h) || (bw, bh) == (h, w));
        if !ok {
            out.push(Marker::Shape { rect: bb, value: area });
        }
    }
    out
}

/// Twice the signed area of a ring (positive with the solid on the left, an outer boundary).
fn ring_area2(r: &[Edge]) -> i64 {
    r.iter().map(|e| i64::from(e.a.0) * i64::from(e.b.1) - i64::from(e.b.0) * i64::from(e.a.1)).sum()
}

/// Holes (rings with the solid outside them) whose area is below `a`.
/// Notch bottoms with thin metal behind: an edge whose two corners are concave (the ring turns
/// right at both ends; metal is on its left), shorter than `len`, with a neighbouring edge (a notch
/// side) shorter than `side`, and an edge of the same region facing it from inside, side to side,
/// closer than `w`. One result per such edge.
fn thin_notches(m: &Merged, len: i64, side: i64, w: i64) -> Vec<Marker> {
    let elen = |e: &Edge| i64::from((e.b.0 - e.a.0).abs() + (e.b.1 - e.a.1).abs());
    let cross = |e: &Edge, f: &Edge| -> i64 {
        let (ux, uy) = (i64::from(e.b.0 - e.a.0), i64::from(e.b.1 - e.a.1));
        let (vx, vy) = (i64::from(f.b.0 - f.a.0), i64::from(f.b.1 - f.a.1));
        ux * vy - uy * vx
    };
    let all = m.edges();
    let boxes: Vec<Rect> = all.iter().map(bbox_edge).collect();
    let idx = RegionIndex::build(&boxes);
    let mut out = Vec::new();
    for ring in &m.rings {
        let n = ring.len();
        for i in 0..n {
            let (p, e, q) = (&ring[(i + n - 1) % n], &ring[i], &ring[(i + 1) % n]);
            if cross(p, e) >= 0 || cross(e, q) >= 0 || elen(e) >= len || elen(p).min(elen(q)) >= side {
                continue;
            }
            // the metal behind: a parallel edge in projection, facing it through metal
            let oe = outward_edge(e);
            let behind = idx.within(&bbox_edge(e), w as i32 + 1, None).into_iter().map(|j| &all[j as usize]).filter_map(|f| {
                if f.axis() != e.axis() || !projecting_edges(e, f) {
                    return None;
                }
                let (re, rf) = (bbox_edge(e), bbox_edge(f));
                // only the offset across the edges counts
                let (dx, dy) = if e.axis() == Axis::Horizontal { (0, i64::from(rf.y0 - re.y0)) } else { (i64::from(rf.x0 - re.x0), 0) };
                let gap = dx.abs() + dy.abs();
                // f lies against e's outward normal (inside the metal), and faces back at e
                let of = outward_edge(f);
                (gap > 0 && gap < w && oe.0 * dx + oe.1 * dy < 0 && of.0 * dx + of.1 * dy > 0).then_some(gap)
            }).min();
            if let Some(g) = behind {
                out.push(Marker::Shape { rect: bbox_edge(e), value: g });
            }
        }
    }
    out
}

fn small_holes(m: &Merged, a: i64) -> Vec<Marker> {
    let mut out = Vec::new();
    for r in &m.rings {
        let a2 = ring_area2(r);
        if a2 < 0 && -a2 < 2 * a {
            let xs = r.iter().map(|e| e.a.0);
            let ys = r.iter().map(|e| e.a.1);
            let rect = Rect { x0: xs.clone().min().unwrap_or(0), y0: ys.clone().min().unwrap_or(0), x1: xs.max().unwrap_or(0), y1: ys.max().unwrap_or(0) };
            out.push(Marker::Shape { rect, value: -a2 / 2 });
        }
    }
    out
}

/// Vertices whose two edges are both shorter than `d`.
fn short_vertices(m: &Merged, d: i64) -> Vec<Marker> {
    let mut out = Vec::new();
    for r in &m.rings {
        let n = r.len();
        for i in 0..n {
            let (p, q) = (&r[i], &r[(i + 1) % n]);
            if p.len() < d && q.len() < d {
                out.push(Marker::Edges { a: seg(p), b: seg(q), value: p.len().max(q.len()) });
            }
        }
    }
    out
}

/// Drawn polygon edges that are neither horizontal nor vertical.
fn off_axis(layers: &[Layer], polys: &BTreeMap<Layer, Vec<Vec<(i32, i32)>>>) -> Vec<Marker> {
    let mut out = Vec::new();
    for l in layers {
        for p in polys.get(l).map_or(&[][..], Vec::as_slice) {
            let n = p.len();
            for i in 0..n {
                let (a, b) = (p[i], p[(i + 1) % n]);
                if a.0 != b.0 && a.1 != b.1 {
                    let s = [a.0, a.1, b.0, b.1];
                    out.push(Marker::Edges { a: s, b: s, value: 0 });
                }
            }
        }
    }
    out
}

fn bbox_of(tiles: &[Rect]) -> Option<Rect> {
    tiles.iter().copied().reduce(|a, b| Rect { x0: a.x0.min(b.x0), y0: a.y0.min(b.y0), x1: a.x1.max(b.x1), y1: a.y1.max(b.y1) })
}

fn cell_bbox(cell: &crate::layout::gds::Cell) -> Option<Rect> {
    let pts: Vec<(i32, i32)> = cell.elements.iter().filter_map(|e| elem_poly(e).1).flatten().collect();
    let (x0, x1) = (pts.iter().map(|p| p.0).min()?, pts.iter().map(|p| p.0).max()?);
    let (y0, y1) = (pts.iter().map(|p| p.1).min()?, pts.iter().map(|p| p.1).max()?);
    Some(Rect { x0, y0, x1, y1 })
}

/// Window origins along one axis: `lo`, `lo + step`, … while the window fits, then one more
/// backed up to end at `hi` if the last did not; a single window when the span is shorter.
pub(crate) fn window_starts(lo: i32, hi: i32, window: i64, step: i64) -> Vec<(i32, i32)> {
    let span = i64::from(hi) - i64::from(lo);
    if span <= window {
        return vec![(lo, hi)];
    }
    let mut v = Vec::new();
    let mut x = i64::from(lo);
    while x + window <= i64::from(hi) {
        v.push((x as i32, (x + window) as i32));
        x += step;
    }
    if v.last().is_some_and(|l| l.1 < hi) {
        v.push(((i64::from(hi) - window) as i32, hi));
    }
    v
}

/// Area of a merged region inside window `w`.
fn area_in(m: &Merged, w: &Rect) -> i64 {
    m.idx
        .overlaps(w)
        .into_iter()
        .map(|i| {
            let t = m.tiles[i as usize];
            let dx = i64::from(t.x1.min(w.x1)) - i64::from(t.x0.max(w.x0));
            let dy = i64::from(t.y1.min(w.y1)) - i64::from(t.y0.max(w.y0));
            if dx > 0 && dy > 0 { dx * dy } else { 0 }
        })
        .sum()
}

#[allow(clippy::too_many_arguments)]
fn ratio_windows(num: &Merged, den: &Merged, exempt: Option<&Merged>, ext: Rect, window: i64, step: i64, min: Option<i64>, max: Option<i64>) -> (Vec<Marker>, usize, Option<(i64, i64)>) {
    let mut out = Vec::new();
    let mut measured = 0;
    let mut range: Option<(i64, i64)> = None;
    for (y0, y1) in window_starts(ext.y0, ext.y1, window, step) {
        for (x0, x1) in window_starts(ext.x0, ext.x1, window, step) {
            let w = Rect { x0, y0, x1, y1 };
            if exempt.is_some_and(|e| e.touches(&w)) {
                continue;
            }
            let d = area_in(den, &w);
            if d == 0 {
                continue;
            }
            measured += 1;
            // num/den < m/1e6, exact: num*1e6 < m*den (i128: areas reach 1e18 on a large die)
            let (n, d) = (i128::from(area_in(num, &w)) * 1_000_000, i128::from(d));
            let ppm = (n / d) as i64;
            range = Some(range.map_or((ppm, ppm), |(lo, hi)| (lo.min(ppm), hi.max(ppm))));
            if min.is_some_and(|m| n < i128::from(m) * d) || max.is_some_and(|m| n > i128::from(m) * d) {
                out.push(Marker::Window { rect: w, ppm });
            }
        }
    }
    (out, measured, range)
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
                Marker::Window { rect: b, .. } | Marker::Shape { rect: b, .. } => {
                    k += 1;
                    s.push_str(&format!("p {k} 4\n{} {}\n{} {}\n{} {}\n{} {}\n", b.x0, b.y0, b.x1, b.y0, b.x1, b.y1, b.x0, b.y1));
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

    /// Rule (width): interior width below d, side to side, over the merged shapes: two shapes
    /// side by side make one wide polygon; at d it is clean; the union of two layers counts.
    #[test]
    fn width_is_interior_width_of_the_merged_shape() {
        let r = "width W 1/1 32";
        assert_eq!(count(r, vec![bx(1, 1, 0, 0, 100, 31)]), 1, "31 wide");
        assert_eq!(count(r, vec![bx(1, 1, 0, 0, 100, 32)]), 0, "32 wide");
        assert_eq!(count(r, vec![bx(1, 1, 0, 0, 100, 20), bx(1, 1, 0, 20, 100, 40)]), 0, "two halves merge");
        assert_eq!(count(r, vec![bx(1, 1, 0, 0, 100, 20), bx(1, 2, 0, 20, 100, 40)]), 1, "the other mask's half does not widen it");
        assert_eq!(count("width W 1/1 1/2 32", vec![bx(1, 1, 0, 0, 100, 20), bx(1, 2, 0, 20, 100, 40)]), 0, "union");
        // the gap between two shapes is not interior
        assert_eq!(count(r, vec![bx(1, 1, 0, 0, 100, 40), bx(1, 1, 0, 50, 100, 90)]), 0);
        // two stacked shapes, 30 and 40 wide, 10 apart: each is narrow, and the far edges (80
        // apart, across the gap) are not a width
        assert_eq!(count("width W 1/1 100", vec![bx(1, 1, 0, 0, 100, 30), bx(1, 1, 0, 40, 100, 80)]), 2);
        // a narrow neck in a wide shape
        assert_eq!(count(r, vec![bx(1, 1, 0, 0, 100, 100), bx(1, 1, 100, 40, 200, 60), bx(1, 1, 200, 0, 300, 100)]), 1);
    }

    /// Rule (area): polygon area below a, on the merged shapes; shape=rect / notrect keep only
    /// rectangles / only other polygons.
    #[test]
    fn area_is_per_merged_polygon() {
        assert_eq!(count("area A 1/1 1000", vec![bx(1, 1, 0, 0, 33, 30)]), 1, "990");
        assert_eq!(count("area A 1/1 1000", vec![bx(1, 1, 0, 0, 50, 20)]), 0, "1000");
        assert_eq!(count("area A 1/1 1000", vec![bx(1, 1, 0, 0, 20, 20), bx(1, 1, 20, 0, 40, 30)]), 0, "1000 as one L");
        let l = vec![bx(1, 1, 0, 0, 20, 20), bx(1, 1, 20, 0, 40, 25)];
        assert_eq!(count("area A 1/1 1000 shape=notrect", l.clone()), 1, "900, an L");
        assert_eq!(count("area A 1/1 1000 shape=rect", l), 0, "an L is not a rectangle");
        assert_eq!(count("area A 1/1 1000 shape=rect", vec![bx(1, 1, 0, 0, 30, 30)]), 1);
        assert!(parse("area A 1/1 10 shape=round").is_err());
    }

    /// Rule (hole): an enclosed hole of area below a; the ring around it is not a hole.
    #[test]
    fn hole_is_an_enclosed_gap() {
        let ring = |w: i32| vec![bx(1, 1, 0, 0, 100, 20), bx(1, 1, 0, 20 + w, 100, 40 + w), bx(1, 1, 0, 0, 20, 40 + w), bx(1, 1, 80, 0, 100, 40 + w)];
        // inner hole 60 x w
        assert_eq!(count("hole H 1/1 1000", ring(16)), 1, "960");
        assert_eq!(count("hole H 1/1 1000", ring(17)), 0, "1020");
        assert_eq!(count("hole H 1/1 1000", vec![bx(1, 1, 0, 0, 10, 10)]), 0, "no hole");
    }

    /// Rule (shortvertex): a vertex whose two edges are both shorter than d; a wire end (one
    /// short edge between two long ones) is clean.
    #[test]
    fn short_vertex_needs_two_short_edges() {
        let r = "shortvertex V 1/1 57";
        assert_eq!(count(r, vec![bx(1, 1, 0, 0, 1000, 32)]), 0, "wire end");
        assert_eq!(count(r, vec![bx(1, 1, 0, 0, 56, 56)]), 4, "small square: every corner");
        assert_eq!(count(r, vec![bx(1, 1, 0, 0, 57, 56)]), 0, "57 is not short");
        // a jog: a 30-long step between two long wires makes two short-short vertices? no: each
        // step vertex has one long edge
        assert_eq!(count(r, vec![bx(1, 1, 0, 0, 500, 32), bx(1, 1, 470, 0, 1000, 62)]), 0);
    }

    /// Rule (orthogonal): a drawn edge that is neither horizontal nor vertical.
    #[test]
    fn orthogonal_flags_a_diagonal_edge() {
        let tri = Element::Boundary { layer: 1, datatype: 1, pts: vec![(0, 0), (100, 0), (0, 100), (0, 0)] };
        assert_eq!(count("orthogonal O 1/1", vec![tri]), 1);
        assert_eq!(count("orthogonal O 1/1", vec![bx(1, 1, 0, 0, 10, 10)]), 0);
        assert!(parse("orthogonal O").is_err(), "needs a layer");
        assert!(parse("width W 1/1").is_err(), "needs a value");
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
        assert_eq!(count("eol E 1/1 70 eolmax=40 ext=16 to=1/2", vec![wire.clone(), bx(1, 1, -200, 369, 200, 400)]), 0);
        // the other mask covering the end is the masks meeting, not a spacing
        assert_eq!(count("eol E 1/1 70 eolmax=40 ext=16 to=1/2", vec![wire, bx(1, 2, 0, 200, 30, 301)]), 0);
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
        // toeol=L2: an end of L facing an end of L2; ends of L alone, or of L2 alone, are not it
        let x = "eol T 1/1 80 eolmax=40 adjmin=20 ext=32 toeol=1/2";
        assert_eq!(count(x, vec![bx(1, 1, 0, 0, 30, 300), bx(1, 2, 0, 379, 30, 600)]), 1, "79 tip to tip across masks");
        assert_eq!(count(x, vec![bx(1, 1, 0, 0, 30, 300), bx(1, 2, 0, 380, 30, 600)]), 0, "80");
        assert_eq!(count(x, vec![bx(1, 1, 0, 0, 30, 300), bx(1, 1, 0, 379, 30, 600)]), 0, "same mask");
        assert_eq!(count(x, vec![bx(1, 2, 0, 0, 30, 300), bx(1, 2, 0, 379, 30, 600)]), 0, "other mask only");
        assert_eq!(count(x, vec![bx(1, 1, 0, 0, 30, 300), bx(1, 2, -300, 340, 300, 400)]), 0, "a side of L2");
    }

    /// Rule (space and space2 on a union, `touch`): a union spacing counts pairs across its
    /// layers; with `touch`, A meeting B is a result, without it a stitch is not spacing.
    #[test]
    fn union_space_and_touching_prohibited() {
        let r = "space U 1/1+1/2 140";
        assert_eq!(count(r, vec![bx(1, 1, 0, 0, 280, 280), bx(1, 2, 419, 0, 699, 280)]), 1, "139 across the layers");
        assert_eq!(count(r, vec![bx(1, 1, 0, 0, 280, 280), bx(1, 2, 420, 0, 700, 280)]), 0, "140");
        assert_eq!(count("space U 1/1 140", vec![bx(1, 1, 0, 0, 280, 280), bx(1, 2, 419, 0, 699, 280)]), 0, "one layer alone");
        let t = "space2 T 1/1+1/2 2/0+2/1 200 touch";
        assert_eq!(count(t, vec![bx(1, 2, 0, 0, 280, 280), bx(2, 1, 479, 0, 600, 280)]), 1, "199");
        assert_eq!(count(t, vec![bx(1, 2, 0, 0, 280, 280), bx(2, 1, 480, 0, 600, 280)]), 0, "200");
        assert_eq!(count(t, vec![bx(1, 1, 0, 0, 280, 280), bx(2, 0, 280, 0, 600, 280)]), 1, "touching");
        assert_eq!(count(t, vec![bx(1, 1, 0, 0, 280, 280), bx(2, 0, 100, 100, 200, 200)]), 1, "inside");
        let n = "space2 T 1/1+1/2 2/0+2/1 200";
        assert_eq!(count(n, vec![bx(1, 1, 0, 0, 280, 280), bx(2, 0, 280, 0, 600, 280)]), 0, "touching, no touch");
        assert!(parse("space2 T 1/1 2/0 200 touch").is_ok());
        assert!(parse("space T 1/1 200 touch").is_err());
    }

    /// Rule (widespace): the part of the union wider than w facing the union side to side closer
    /// than d. A polygon's narrow arm is not its wide part; exactly w wide is not wide; a corner
    /// is not side to side; `run` drops a shared run no longer than it; `to=` measures to the
    /// other side's own wide part only.
    #[test]
    fn wide_metal_space_is_measured_from_the_wide_part() {
        let r = "widespace W 1/1 1/2 120 50";
        // 200 wide, a 40-wide wire beside it (across masks: the union)
        assert_eq!(count(r, vec![bx(1, 1, 0, 0, 1000, 200), bx(1, 2, 0, 249, 1000, 289)]), 1, "49");
        assert_eq!(count(r, vec![bx(1, 1, 0, 0, 1000, 200), bx(1, 2, 0, 250, 1000, 290)]), 0, "50");
        // exactly 120 wide is not wide; 122 is
        assert_eq!(count(r, vec![bx(1, 1, 0, 0, 1000, 120), bx(1, 2, 0, 160, 1000, 200)]), 0, "120");
        assert_eq!(count(r, vec![bx(1, 1, 0, 0, 1000, 122), bx(1, 2, 0, 162, 1000, 202)]), 1, "122");
        // a wide block with a 40-wide arm: a wire facing only the arm is not measured from a wide part
        let arm = vec![bx(1, 1, 0, 0, 200, 200), bx(1, 1, 200, 0, 1000, 40), bx(1, 2, 300, 80, 1000, 120)];
        assert_eq!(count(r, arm), 0, "the arm is narrow");
        // a corner neighbour 30 x 30 away diagonally: closer than 50, but not side to side
        assert_eq!(count(r, vec![bx(1, 1, 0, 0, 200, 200), bx(1, 2, 230, 230, 400, 270)]), 0, "corner");
        // run: a 250-long shared run is not more than 250
        let run = "widespace W 1/1 120 80 run=250";
        assert_eq!(count(run, vec![bx(1, 1, 0, 0, 1000, 250), bx(1, 1, 0, 300, 250, 340)]), 0, "run 250");
        assert_eq!(count(run, vec![bx(1, 1, 0, 0, 1000, 250), bx(1, 1, 0, 300, 251, 340)]), 1, "run 251");
        // to=: to the other side's part wider than 60 only
        let to = "widespace W 1/1 120 56 to=60";
        assert_eq!(count(to, vec![bx(1, 1, 0, 0, 1000, 200), bx(1, 1, 0, 250, 1000, 290)]), 0, "a 40 wire is not wider than 60");
        assert_eq!(count(to, vec![bx(1, 1, 0, 0, 1000, 200), bx(1, 1, 0, 250, 1000, 330)]), 1, "an 80 wire is");
        // an odd width: wider than 61 is 62 and up
        let odd = "widespace W 1/1 1/2 61 50";
        assert_eq!(count(odd, vec![bx(1, 1, 0, 0, 1000, 61), bx(1, 2, 0, 101, 1000, 141)]), 0, "61 is not wider than 61");
        assert_eq!(count(odd, vec![bx(1, 1, 0, 0, 1000, 62), bx(1, 2, 0, 102, 1000, 142)]), 1, "62 is");
        assert!(parse("widespace W 1/1 0 50").is_err(), "a width must be positive");
    }

    /// Rule (widespace bridge): a neck of metal joining the wide part to the other side's part is
    /// not space, but only the run it covers; air beside it still is. Metal in the gap that does not
    /// join both sides (floating, or a stub from one side only) hides nothing.
    #[test]
    fn a_bridge_hides_only_the_run_it_covers() {
        let block = bx(1, 1, 0, 0, 200, 200);
        let part = bx(1, 1, 250, 60, 1000, 140); // 80 wide, beyond to=60
        let neck = bx(1, 1, 200, 80, 250, 120); // 40 wide, narrower than to=60
        let r = "widespace W 1/1 120 56 to=60 bridge";
        assert_eq!(count(r, vec![block.clone(), part.clone()]), 1, "plain space 50");
        assert_eq!(count(r, vec![block.clone(), part.clone(), neck.clone()]), 1, "20 of air above and below the neck");
        let r20 = "widespace W 1/1 120 56 to=60 bridge run=20";
        assert_eq!(count(r20, vec![block.clone(), part.clone(), neck.clone()]), 0, "the air left is 20, not more");
        assert_eq!(count(r20, vec![block.clone(), part.clone()]), 1, "no neck: the run is 80");
        let float = bx(1, 1, 215, 60, 235, 140);
        assert_eq!(count(r, vec![block.clone(), part.clone(), float]), 1, "floating metal is no bridge");
        // 40 tall: no 60 square fits it, so it is not in the other side's part either
        let stub = bx(1, 1, 200, 80, 230, 120);
        assert_eq!(count(r20, vec![block.clone(), part.clone(), stub]), 1, "a stub from one side is no bridge");
        // two wide parts joined by a neck: the other side is wide too, not the part beyond the wide
        // one, so the neck is no bridge here (a wide-to-wide check is another rule's)
        let block2 = bx(1, 1, 250, 0, 450, 200);
        assert_eq!(count(r20, vec![block.clone(), block2, neck.clone()]), 1, "wide to wide across a neck");
        // 80 tall: 60 squares straddling the block and the stub put it in the part wider than 60,
        // so it joins the wide part to metal of the other side: a bridge (the opening's reading)
        let deep = bx(1, 1, 200, 60, 230, 140);
        assert_eq!(count(r, vec![block.clone(), part.clone(), deep]), 0, "a stub the opening takes in");
        assert!(parse("space W 1/1 50 bridge").is_err(), "bridge is widespace only");
    }

    /// Rule (widespace bridge2): between two wide parts (space or notch) a neck touching both is
    /// not space; a stub touching one is; `bridge` (the part beyond) finds no bridge here.
    #[test]
    fn a_neck_between_two_wide_parts_is_a_bridge2() {
        let (b1, b2) = (bx(1, 1, 0, 0, 200, 200), bx(1, 1, 250, 0, 450, 200));
        let neck = bx(1, 1, 200, 80, 250, 120);
        let r = "widespace W 1/1 120 56 to=120 bridge2";
        assert_eq!(count(r, vec![b1.clone(), b2.clone(), neck.clone()]), 1, "air of 80 above and below");
        let r80 = "widespace W 1/1 120 56 to=120 bridge2 run=80";
        assert_eq!(count(r80, vec![b1.clone(), b2.clone(), neck.clone()]), 0, "80 left, not more");
        assert_eq!(count(r80, vec![b1.clone(), b2.clone()]), 1, "no neck: 200");
        let stub = bx(1, 1, 200, 80, 230, 120);
        assert_eq!(count(r80, vec![b1.clone(), b2.clone(), stub]), 1, "a stub touches one part");
        let beyond = "widespace W 1/1 120 56 to=120 bridge run=80";
        assert_eq!(count(beyond, vec![b1.clone(), b2.clone(), neck.clone()]), 1, "no part beyond: no bridge");
        assert!(parse("widespace W 1/1 120 56 bridge bridge2").is_err(), "one bridge option");
    }

    /// Rule (widespace jog): pieces are summed along a line and across a short step with one
    /// convex end; a cluster longer than `run` is one result (each side of the gap its own).
    #[test]
    fn a_run_is_summed_across_short_steps() {
        let wide = bx(1, 1, 0, 0, 1000, 200);
        let w1 = bx(1, 1, 0, 250, 200, 290);
        let w2 = bx(1, 1, 200, 260, 400, 300); // a 10 step up at x=200, one polygon with w1
        let r = "widespace W 1/1 160 70 run=250 jog=70";
        assert_eq!(count(r, vec![wide.clone(), w1.clone()]), 0, "a run of 200");
        assert_eq!(count(r, vec![wide.clone(), w1.clone(), w2.clone()]), 2, "200 + 200 on each side");
        let short = "widespace W 1/1 160 70 run=250 jog=8";
        assert_eq!(count(short, vec![wide.clone(), w1.clone(), w2.clone()]), 1, "the step is longer than 8: only the wide side's line sums");
        // two wires apart (not one polygon): no step joins them
        let w2b = bx(1, 1, 210, 260, 400, 300);
        assert_eq!(count(r, vec![wide.clone(), w1.clone(), w2b]), 0, "apart: 200 and 190, and the wide side's pieces do not meet either");
        // a bump down: each side has one convex and one concave end, so both are steps and the
        // neighbour's line sums through the bump (190 + 20 + 190); the wide side's line sums too
        let bump = vec![wide.clone(), bx(1, 1, 0, 260, 400, 300), bx(1, 1, 190, 250, 210, 260)];
        assert_eq!(count(r, bump.clone()), 2, "joined through the bump");
        assert_eq!(count(short, bump), 1, "steps longer than the jog: the neighbour's pieces stay apart");
        // the step is exactly the jog: joined; a run exactly the limit is not longer
        assert_eq!(count("widespace W 1/1 160 70 run=250 jog=10", vec![wide.clone(), w1.clone(), w2.clone()]), 2, "jog 10");
        assert_eq!(count("widespace W 1/1 160 70 run=200 jog=70", vec![wide.clone(), w1.clone()]), 0, "run 200 is not more than 200");
        // a wire between two wide parts: its ends (both corners convex) are line ends, not steps,
        // so its bottom and top pieces (200 each) stay two clusters
        let sandwich = vec![wide.clone(), w1.clone(), bx(1, 1, 0, 340, 1000, 540)];
        assert_eq!(count(r, sandwich), 0, "line ends do not join");
    }

    /// Rule (notch): a short notch bottom with thin metal behind it. The notch is cut into a bar
    /// from above; the bar under the notch is the metal behind.
    #[test]
    fn a_short_notch_over_thin_metal() {
        // a bar 0..400 x 0..100 with a notch 180..220 cut from the top down to y=40: 40 of metal
        // behind, a bottom 40 long, sides 60 long
        let notched = vec![bx(1, 1, 0, 0, 180, 100), bx(1, 1, 180, 0, 220, 40), bx(1, 1, 220, 0, 400, 100)];
        let r = "notch W 1/1 90 130 46";
        assert_eq!(count(r, notched.clone()), 1, "40 behind");
        assert_eq!(count("notch W 1/1 90 130 40", notched.clone()), 0, "40 is not less than 40");
        assert_eq!(count("notch W 1/1 40 130 46", notched.clone()), 0, "the bottom is 40, not shorter than 40");
        assert_eq!(count("notch W 1/1 90 60 46", notched.clone()), 0, "the sides are 60, not shorter than 60");
        assert_eq!(count("notch W 1/1 90 61 46", notched.clone()), 1, "sides shorter than 61");
        let deep = vec![bx(1, 1, 0, 0, 180, 100), bx(1, 1, 180, 0, 220, 50), bx(1, 1, 220, 0, 400, 100)];
        assert_eq!(count(r, deep), 0, "50 behind is not thin");
        // a short shelf (its top has one concave end) is no notch bottom, though 40 lies under it
        let shelf = vec![bx(1, 1, 0, 0, 200, 100), bx(1, 1, 200, 0, 240, 40)];
        assert_eq!(count(r, shelf), 0, "a shelf");
        // one side long, the other short: a side shorter than 130 is enough
        let lop = vec![bx(1, 1, 0, 0, 180, 200), bx(1, 1, 180, 0, 220, 40), bx(1, 1, 220, 0, 400, 100)];
        assert_eq!(count(r, lop), 1, "sides 160 and 60");
        // a floating piece inside a notch with thick metal behind is outside, not behind
        let float = vec![bx(1, 1, 0, 0, 180, 100), bx(1, 1, 180, 0, 220, 50), bx(1, 1, 220, 0, 400, 100), bx(1, 1, 190, 60, 210, 70)];
        assert_eq!(count(r, float), 0, "the floating piece lies outside the metal");
        let mirrored = vec![bx(1, 1, 0, 0, 200, 100), bx(1, 1, -40, 0, 0, 40)];
        assert_eq!(count(r, mirrored), 0, "a shelf on the other side");
        // 50 under the notch, and a slot cut from below beside it leaving 30: not under the notch
        let aside = vec![
            bx(1, 1, 0, 0, 100, 100),
            bx(1, 1, 100, 10, 170, 100),
            bx(1, 1, 170, 0, 180, 100),
            bx(1, 1, 180, 0, 220, 50),
            bx(1, 1, 220, 0, 400, 100),
        ];
        assert_eq!(count(r, aside), 0, "thin metal beside, not behind");
        // metal over the notch mouth is outside, not behind
        let mut lid = notched.clone();
        lid.push(bx(1, 1, 0, 130, 400, 160));
        assert_eq!(count(r, lid), 1, "the bar under the notch, not the wire over it");
        let mut close = notched.clone();
        close.push(bx(1, 1, 0, 110, 400, 140)); // 10 over the mouth: still outside
        assert_eq!(count("notch W 1/1 90 130 41", close), 1, "outside metal 70 away from the bottom is not behind; 40 is");
    }

    /// Rule (rectangle, size): a merged polygon that is not a rectangle; one that is not w by h
    /// (either way round).
    #[test]
    fn rectangle_and_exact_size() {
        let l = vec![bx(1, 1, 0, 0, 280, 100), bx(1, 1, 0, 100, 100, 280)];
        assert_eq!(count("rectangle C 1/1", l.clone()), 1, "an L");
        assert_eq!(count("rectangle C 1/1", vec![bx(1, 1, 0, 0, 280, 100), bx(1, 1, 0, 100, 280, 280)]), 0, "two tiles, one rectangle");
        assert_eq!(count("rectangle C 1/1 1/2", vec![bx(1, 1, 0, 0, 280, 100), bx(1, 2, 0, 100, 100, 280)]), 1, "an L across the union");
        let z = "size S 1/1 1/2 280 140";
        assert_eq!(count(z, vec![bx(1, 1, 0, 0, 280, 140)]), 0);
        assert_eq!(count(z, vec![bx(1, 2, 0, 0, 140, 280)]), 0, "turned");
        assert_eq!(count(z, vec![bx(1, 1, 0, 0, 281, 140)]), 1, "one too long");
        assert_eq!(count(z, vec![bx(1, 1, 0, 0, 280, 139)]), 1, "one too narrow");
        assert_eq!(count(z, l), 1, "not a rectangle");
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

    /// Rule (stitch): each place the two masks overlap or abut is one result; apart, none.
    #[test]
    fn every_meeting_of_the_masks_is_listed() {
        let r = "stitch M 1/1 1/2";
        assert_eq!(count(r, vec![bx(1, 1, 0, 0, 100, 30), bx(1, 2, 80, 0, 200, 30)]), 1, "overlap");
        assert_eq!(count(r, vec![bx(1, 1, 0, 0, 100, 30), bx(1, 2, 100, 0, 200, 30)]), 1, "abut");
        assert_eq!(count(r, vec![bx(1, 1, 0, 0, 100, 30), bx(1, 2, 101, 0, 200, 30)]), 0, "apart");
        assert_eq!(count(r, vec![bx(1, 1, 0, 0, 100, 30), bx(1, 2, 100, 30, 200, 60)]), 0, "a corner only");
        // one E2 shape over two touching E1 pieces is one meeting
        assert_eq!(count(r, vec![bx(1, 1, 0, 0, 100, 30), bx(1, 1, 100, 0, 200, 30), bx(1, 2, 50, 0, 150, 30)]), 1);
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

    /// Window placement (the deck's `WINDOW w STEP s BACKUP`): origins step by s from the lower
    /// left while the window fits; if the last does not end on the extent, one more window is
    /// backed up to end there; an extent shorter than a window is one window over the extent.
    #[test]
    fn ratio_windows_step_then_back_up() {
        assert_eq!(window_starts(0, 100, 100, 50), vec![(0, 100)]);
        assert_eq!(window_starts(0, 60, 100, 50), vec![(0, 60)], "shorter than a window");
        assert_eq!(window_starts(0, 200, 100, 50), vec![(0, 100), (50, 150), (100, 200)]);
        assert_eq!(window_starts(0, 230, 100, 50), vec![(0, 100), (50, 150), (100, 200), (130, 230)], "backed up");
        assert_eq!(window_starts(10, 210, 100, 100), vec![(10, 110), (110, 210)]);
    }

    /// Rule (ratio): area(union NUM) / area(union DEN) per window, compared exactly; at the bound
    /// is clean, below it (min=) or above it (max=) is one result per window; a window with no
    /// DEN area is not measured.
    #[test]
    fn ratio_is_colour_balance_per_window() {
        let min = "ratio B 1/1 / 1/1 1/2 window=100 step=50 min=0.2";
        let max = "ratio B 1/1 / 1/1 1/2 window=100 step=50 max=0.8";
        // E1 area 1000, E2 4000: exactly 0.2
        let at = vec![bx(1, 1, 0, 0, 100, 10), bx(1, 2, 0, 20, 100, 60)];
        assert_eq!(count(min, at.clone()), 0, "at the bound");
        let below = vec![bx(1, 1, 0, 0, 99, 10), bx(1, 2, 0, 20, 100, 60)];
        assert_eq!(count(min, below.clone()), 1, "990/4990 < 0.2");
        assert_eq!(count(max, below), 0);
        // E1 4000, E2 1000: exactly 0.8, then above it
        assert_eq!(count(max, vec![bx(1, 1, 0, 20, 100, 60), bx(1, 2, 0, 0, 100, 10)]), 0);
        assert_eq!(count(max, vec![bx(1, 1, 0, 20, 100, 61), bx(1, 2, 0, 0, 100, 10)]), 1);
        // an overlap counts once in each union: E1 over E2 entirely is ratio 1
        assert_eq!(count(max, vec![bx(1, 1, 0, 0, 100, 10), bx(1, 2, 0, 0, 100, 10)]), 1);
        // three windows across x (0, 50, 100): the left is balanced, the middle and right hold
        // only E2 (ratio 0)
        let two = vec![bx(1, 1, 0, 0, 50, 50), bx(1, 2, 50, 0, 100, 50), bx(1, 2, 150, 0, 200, 50)];
        let r = check(&lib(two), None, &parse(min).unwrap()).unwrap();
        assert_eq!(r[0].measured, 3);
        // the range over the measured windows: 0 (two E2-only windows) to 0.5 (the left one)
        assert_eq!(r[0].range_ppm, Some((0, 500_000)));
        let w = |x0| Marker::Window { rect: Rect { x0, y0: 0, x1: x0 + 100, y1: 50 }, ppm: 0 };
        assert_eq!(r[0].markers, vec![w(50), w(100)]);
        // only NUM-free, DEN-free windows: nothing measured, so a clean count is vacuous
        let r = check(&lib(vec![bx(2, 0, 0, 0, 10, 10)]), None, &parse(min).unwrap()).unwrap();
        assert_eq!((r[0].measured, r[0].markers.len(), r[0].range_ppm), (0, 0, None));
    }

    /// Rule (ratio): a window touching the exempt layer is not measured; `inside=` sets the
    /// extent the windows tile.
    #[test]
    fn ratio_exempt_and_inside() {
        let below = vec![bx(1, 1, 0, 0, 99, 10), bx(1, 2, 0, 20, 100, 60)];
        let mut ex = below.clone();
        ex.push(bx(9, 0, 90, 90, 95, 95));
        assert_eq!(count("ratio B 1/1 / 1/1 1/2 window=100 step=50 min=0.2 exempt=9/0", ex), 0);
        let mut far = below.clone();
        far.push(bx(9, 0, 300, 300, 310, 310));
        // inside= limits the extent to 0..100: one window, the far exempt shape does not touch it
        assert_eq!(count("ratio B 1/1 / 1/1 1/2 window=100 step=50 min=0.2 exempt=9/0 inside=8/0", {
            let mut v = far.clone();
            v.push(bx(8, 0, 0, 0, 100, 100));
            v
        }), 1);
        // exempt is a union: the second layer of the list exempts the window too
        let mut ex2 = vec![bx(1, 1, 0, 0, 99, 10), bx(1, 2, 0, 20, 100, 60)];
        ex2.push(bx(9, 1, 90, 90, 95, 95));
        assert_eq!(count("ratio B 1/1 / 1/1 1/2 window=100 step=50 min=0.2 exempt=9/0,9/1", ex2.clone()), 0);
        assert_eq!(count("ratio B 1/1 / 1/1 1/2 window=100 step=50 min=0.2 exempt=9/0", ex2), 1);
        // minextent: the 100 x 60 layout is narrower than 61 in y, so nothing is measured
        let r = check(&lib(vec![bx(1, 1, 0, 0, 99, 10), bx(1, 2, 0, 20, 100, 60)]), None, &parse("ratio B 1/1 / 1/1 1/2 window=100 step=50 min=0.2 minextent=61").unwrap()).unwrap();
        assert_eq!((r[0].measured, r[0].markers.len()), (0, 0));
        assert_eq!(count("ratio B 1/1 / 1/1 1/2 window=100 step=50 min=0.2 minextent=60", vec![bx(1, 1, 0, 0, 99, 10), bx(1, 2, 0, 20, 100, 60)]), 1);
        // a guard layer far from the metal: windows inside it hold no DEN area
        let mut g = below;
        g.push(bx(8, 0, 1000, 1000, 1100, 1100));
        let r = check(&lib(g), None, &parse("ratio B 1/1 / 1/1 1/2 window=100 step=50 min=0.2 inside=8/0").unwrap()).unwrap();
        assert_eq!((r[0].measured, r[0].markers.len()), (0, 0));
    }

    #[test]
    fn ratio_parse_errors() {
        for bad in [
            "ratio B 1/1 1/2 window=100 step=50 min=0.2",
            "ratio B / 1/2 window=100 step=50 min=0.2",
            "ratio B 1/1 / window=100 step=50 min=0.2",
            "ratio B 1/1 / 1/2 window=100 step=50",
            "ratio B 1/1 / 1/2 window=100 step=50 min=0.2 max=0.8",
            "ratio B 1/1 / 1/2 window=100 step=50 min=1.5",
            "ratio B 1/1 / 1/2 step=50 min=0.2",
            "ratio B 1/1 / 1/2 window=100 min=0.2",
            "ratio B 1/1 / 1/2 window=100 step=50 min=0.2 eolmax=3",
            "space S 1/1 60 min=0.2",
        ] {
            assert!(parse(bad).is_err(), "{bad}");
        }
        assert!(parse("ratio B 1/1 / 1/1 1/2 window=100 step=50 max=0.8 exempt=9/0,9/1 inside=8/0 minextent=100").is_ok());
        assert!(parse("ratio B 1/1 / 1/2 window=100 step=50 min=0.2 exempt=9/0,x").is_err());
    }

}
