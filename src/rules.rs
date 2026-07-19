//! DRC rule deck — the per-layer geometry limits.
//!
//! A `.drc` file is a small whitespace table (std-only parser, no deps). Comments
//! start with `#`. Each rule names a **GDS layer number** and a minimum in DB
//! units:
//!
//! ```text
//! # rule    layer  args
//! width     66     170             # min width on layer 66 (e.g. poli)
//! space     68     140             # min spacing on layer 68 (e.g. met1)
//! area      68     20000           # min polygon area (dbu²) on layer 68
//! density   68     20 70 100000    # metal coverage on 68 must be 20–70% per 100000-dbu window
//! ```
//!
//! A layer is a **GDS layer/datatype** pair, written `num/datatype` (e.g. `66/20`
//! for sky130 drawn poly). A bare `num` means datatype `0`. This qualification is
//! required on real PDKs: different physical layers share a GDS layer number and are
//! told apart only by datatype (sky130 packs drawn poly `66/20` and the licon1
//! contact `66/44` onto layer 66), so a datatype-blind check would union them.

use std::collections::BTreeMap;

/// A GDS layer, identified by number **and** datatype. `dt` defaults to 0 for a bare
/// layer number in the deck. Ordered/hashable so it can key the per-layer shape maps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Layer {
    pub num: i16,
    pub dt: i16,
}

impl Layer {
    pub fn new(num: i16, dt: i16) -> Layer {
        Layer { num, dt }
    }

    /// Parse a deck layer token: `"66/20"` → `66/20`, bare `"66"` → `66/0`.
    pub fn parse(s: &str) -> Option<Layer> {
        match s.split_once('/') {
            Some((n, d)) => Some(Layer {
                num: n.trim().parse().ok()?,
                dt: d.trim().parse().ok()?,
            }),
            None => Some(Layer {
                num: s.trim().parse().ok()?,
                dt: 0,
            }),
        }
    }
}

impl std::fmt::Display for Layer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.num, self.dt)
    }
}

/// A windowed-density rule: coverage on the layer must stay within `min..=max`
/// percent, measured per square `window`-DB-unit tile.
#[derive(Debug, Clone, Copy)]
pub struct Density {
    pub min_pct: i64,
    pub max_pct: i64,
    pub window: i64,
}

/// An antenna rule: on any net, the ratio of `conductor`-layer area to connected
/// `gate`-layer area must not exceed `max_ratio` (process-antenna / plasma-damage
/// protection). Checked per extracted net, so it needs connectivity (`connect`).
#[derive(Debug, Clone, Copy)]
pub struct Antenna {
    pub conductor: Layer,
    pub gate: Layer,
    pub max_ratio: f64,
}

/// An enclosure rule: every `inner`-layer shape must sit inside an `outer`-layer
/// shape with at least `min` DB units of margin on every side (e.g. metal must
/// enclose a via/cut).
#[derive(Debug, Clone, Copy)]
pub struct Enclosure {
    pub outer: Layer,
    pub inner: Layer,
    pub min: i64,
}

/// A via-span rule: every `cut`-layer shape sitting on a `metal`-layer shape must
/// span the metal's full width (its shorter dimension), edges coincident on both
/// width-sides. A cut narrower than the metal, shifted, or protruding past the metal
/// edge violates — the generic form of an advanced-node "via lands on the full wire
/// width" rule.
#[derive(Debug, Clone, Copy)]
pub struct Span {
    pub cut: Layer,
    pub metal: Layer,
}

/// An asymmetric via-enclosure rule: every `inner`-layer shape must be enclosed by a
/// single `outer`-layer shape such that, on at least one axis, the enclosure on **both**
/// opposite sides is ≥ `minor` and on **at least one** side is ≥ `major`. This is the
/// generic advanced-node "via line-end / side" enclosure — a large enclosure along the
/// routing direction and a small one across it, required on only one axis. An inner not
/// enclosed by any single outer shape, or meeting the margins on neither axis, violates.
#[derive(Debug, Clone, Copy)]
pub struct Venc {
    pub outer: Layer,
    pub inner: Layer,
    pub major: i64,
    pub minor: i64,
}

/// A manufacturing-grid rule: every shape vertex on the layer must lie on the grid — its
/// x a multiple of `xpitch` and its y a multiple of `ypitch` (DB units). A pitch of 1
/// leaves that axis unconstrained. Flags each distinct off-grid vertex on the layer.
#[derive(Debug, Clone, Copy)]
pub struct Grid {
    pub layer: Layer,
    pub xpitch: i64,
    pub ypitch: i64,
}

/// A routing-track rule: a **minimum-width** wire (short dimension equal to `width`) must
/// be centered on the routing-track grid — its centerline, on the width axis, a multiple
/// of `pitch` offset by `offset` (DB units). The generic advanced-node "min-width tracks
/// lie on the routing grid" rule.
#[derive(Debug, Clone, Copy)]
pub struct Track {
    pub layer: Layer,
    pub width: i64,
    pub pitch: i64,
    pub offset: i64,
}

/// A via-corner (edge-coincidence) rule: every `inner`-layer shape must have all of its
/// corners lie on the merged `outer`-layer boundary — i.e. the via matches the metal
/// outline. A corner where **both** incident edges depart from the merged outer boundary
/// (a convex corner not backed by metal on either side) is flagged. This is the generic
/// advanced-node "a via must match the metal width across the routing direction" rule.
#[derive(Debug, Clone, Copy)]
pub struct Corner {
    pub outer: Layer,
    pub inner: Layer,
}

/// A directional edge-spacing rule: on `layer`'s merged boundary, an edge of length in
/// `[a_min, a_max]` that faces an edge of length in `[b_min, b_max]` across empty space,
/// closer than `dist`, violates. An `_max` of `0` means unbounded. When the two length
/// classes are identical it is same-layer (`space`) spacing; otherwise it is `separation`
/// between two classes. This is the generic advanced-node tip-to-side / tip-to-tip family
/// (classify boundary edges by length: tip / wide-tip / narrow-tip / side).
#[derive(Debug, Clone, Copy)]
pub struct Sep {
    pub layer: Layer,
    pub a_min: i64,
    pub a_max: i64,
    pub b_min: i64,
    pub b_max: i64,
    pub dist: i64,
}

/// A corner-to-corner spacing rule: on `layer`'s merged boundary, two **parallel**
/// non-projecting edges (their closest approach is at corners) that face each other across
/// empty space closer than `dist` violate. The generic advanced-node "minimum diagonal
/// corner-to-corner spacing" rule — the close approaches that side-to-side / tip spacing
/// (projection-overlapping) do not cover.
#[derive(Debug, Clone, Copy)]
pub struct C2c {
    pub layer: Layer,
    pub dist: i64,
}

/// A parallel-run-length rule: where two same-layer edges face across a gap closer than
/// `space`, the length of that parallel run (the merged gap's extent along the run
/// direction) must be at least `min_run`; a shorter run violates. The generic advanced-node
/// "a short parallel run at tight spacing is forbidden" rule.
#[derive(Debug, Clone, Copy)]
pub struct RunLen {
    pub layer: Layer,
    pub space: i64,
    pub min_run: i64,
}

/// A metal-fill rule (drives the `fill` generator, not the checker): top up
/// `layer` coverage to at least `target_pct` per square `window`, by tiling
/// `size`-square fill shapes that keep `gap` clearance from existing geometry.
#[derive(Debug, Clone, Copy)]
pub struct Fill {
    pub layer: Layer,
    pub target_pct: i64,
    pub window: i64,
    pub size: i64,
    pub gap: i64,
}

#[derive(Debug, Clone, Default)]
pub struct Rules {
    /// layer → minimum width (DB units).
    pub width: BTreeMap<Layer, i64>,
    /// layer → minimum spacing (DB units).
    pub space: BTreeMap<Layer, i64>,
    /// layer → minimum polygon area (DB units²).
    pub area: BTreeMap<Layer, i64>,
    /// layer → windowed metal-density bounds.
    pub density: BTreeMap<Layer, Density>,
    /// layer pairs that electrically connect where they overlap (vias / contacts).
    /// Same-layer overlap/touch always connects; cross-layer needs a `connect` rule.
    pub connect: Vec<(Layer, Layer)>,
    /// antenna ratio rules (need connectivity from `connect`).
    pub antenna: Vec<Antenna>,
    /// enclosure rules (inner must be enclosed by outer with a margin).
    pub enclosure: Vec<Enclosure>,
    /// via-span rules (a cut must span the full width of the metal it sits on).
    pub span: Vec<Span>,
    /// asymmetric via-enclosure rules (inner enclosed by outer, satisfied on one axis).
    pub venc: Vec<Venc>,
    /// manufacturing-grid rules (layer vertices must lie on an x/y grid).
    pub grid: Vec<Grid>,
    /// routing-track rules (min-width wire centerlines must lie on the track grid).
    pub track: Vec<Track>,
    /// via-corner rules (an inner shape's corners must lie on the merged outer boundary).
    pub corner: Vec<Corner>,
    /// directional edge-spacing rules (tip-to-side / tip-to-tip by edge length class).
    pub sep: Vec<Sep>,
    /// corner-to-corner spacing rules (diagonal close approaches at corners).
    pub c2c: Vec<C2c>,
    /// parallel-run-length rules (a short run at tight spacing is forbidden).
    pub runlen: Vec<RunLen>,
    /// metal-fill rules (consumed by the `fill` generator, not the checker).
    pub fill: Vec<Fill>,
}

#[derive(Debug)]
pub struct RulesError(pub String);

impl std::fmt::Display for RulesError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "rules error: {}", self.0)
    }
}
impl std::error::Error for RulesError {}

impl Rules {
    /// Rule families that carry at least one rule, and how many rules there are in total.
    ///
    /// A view or a report that says "checked" without saying *how much was checked* invites
    /// the reader to supply their own assumption, which is usually "a foundry deck". This is
    /// the deck's own account of itself.
    pub fn summary(&self) -> (Vec<&'static str>, usize) {
        // (name, count) per family. Listed explicitly rather than derived, so adding a rule
        // family to the struct without adding it here is a compile-time nudge, not a silent
        // undercount.
        let counts: [(&'static str, usize); 15] = [
            ("width", self.width.len()),
            ("space", self.space.len()),
            ("area", self.area.len()),
            ("density", self.density.len()),
            ("connect", self.connect.len()),
            ("antenna", self.antenna.len()),
            ("enclosure", self.enclosure.len()),
            ("span", self.span.len()),
            ("venc", self.venc.len()),
            ("grid", self.grid.len()),
            ("track", self.track.len()),
            ("corner", self.corner.len()),
            ("sep", self.sep.len()),
            ("c2c", self.c2c.len()),
            ("runlen", self.runlen.len()),
        ];
        let families = counts
            .iter()
            .filter(|(_, n)| *n > 0)
            .map(|(k, _)| *k)
            .collect();
        // `fill` is deliberately excluded: it is consumed by the fill generator, not the
        // checker, so counting it would overstate what was checked.
        let total = counts.iter().map(|(_, n)| n).sum();
        (families, total)
    }

    /// Whether any rule in the deck mentions `layer`.
    ///
    /// The basis of the coverage report: geometry on a layer no rule names was not found
    /// clean, it was not examined. Those are very different claims and only one of them is
    /// true.
    pub fn covers(&self, layer: Layer) -> bool {
        self.width.contains_key(&layer)
            || self.space.contains_key(&layer)
            || self.area.contains_key(&layer)
            || self.density.contains_key(&layer)
            || self.connect.iter().any(|(a, b)| *a == layer || *b == layer)
            || self
                .antenna
                .iter()
                .any(|a| a.conductor == layer || a.gate == layer)
            || self
                .enclosure
                .iter()
                .any(|e| e.inner == layer || e.outer == layer)
            || self.span.iter().any(|s| s.cut == layer || s.metal == layer)
            || self
                .venc
                .iter()
                .any(|v| v.inner == layer || v.outer == layer)
            || self.grid.iter().any(|g| g.layer == layer)
            || self.track.iter().any(|t| t.layer == layer)
            || self
                .corner
                .iter()
                .any(|c| c.inner == layer || c.outer == layer)
            || self.sep.iter().any(|s| s.layer == layer)
            || self.c2c.iter().any(|c| c.layer == layer)
            || self.runlen.iter().any(|r| r.layer == layer)
    }

    pub fn parse(text: &str) -> Result<Rules, RulesError> {
        let mut r = Rules::default();
        for (n, raw) in text.lines().enumerate() {
            let line = raw.split('#').next().unwrap_or("");
            let toks: Vec<&str> = line.split_whitespace().collect();
            if toks.is_empty() {
                continue;
            }
            let err = |what: &str| RulesError(format!("line {}: {what}: {:?}", n + 1, raw.trim()));
            let layer: Layer = toks
                .get(1)
                .and_then(|s| Layer::parse(s))
                .ok_or_else(|| err("expected `<rule> <layer[/datatype]> ...`"))?;
            // parse the k-th argument (after `<rule> <layer>`) as an integer
            let arg = |k: usize, what: &str| -> Result<i64, RulesError> {
                toks.get(2 + k)
                    .and_then(|s| s.parse().ok())
                    .ok_or_else(|| err(what))
            };
            // parse a secondary layer token (a second `layer[/datatype]`) at position `i`
            let layer_at = |i: usize, what: &str| -> Result<Layer, RulesError> {
                toks.get(i)
                    .and_then(|s| Layer::parse(s))
                    .ok_or_else(|| err(what))
            };
            match toks[0].to_ascii_lowercase().as_str() {
                "width" => {
                    r.width.insert(
                        layer,
                        arg(0, "expected an integer minimum width (DB units)")?,
                    );
                }
                "space" | "spacing" => {
                    r.space.insert(
                        layer,
                        arg(0, "expected an integer minimum spacing (DB units)")?,
                    );
                }
                "area" => {
                    r.area.insert(
                        layer,
                        arg(0, "expected an integer minimum area (DB units²)")?,
                    );
                }
                "density" => {
                    let d = Density {
                        min_pct: arg(0, "density: expected `<layer> <min%> <max%> <window>`")?,
                        max_pct: arg(1, "density: expected `<layer> <min%> <max%> <window>`")?,
                        window: arg(2, "density: expected `<layer> <min%> <max%> <window>`")?,
                    };
                    if d.window <= 0 {
                        return Err(err("density window must be > 0"));
                    }
                    if d.min_pct > d.max_pct {
                        return Err(err("density min% must be ≤ max%"));
                    }
                    r.density.insert(layer, d);
                }
                "connect" => {
                    // `connect <layerA> <layerB>`
                    let b = layer_at(2, "connect: expected `<layerA> <layerB>`")?;
                    r.connect.push((layer, b));
                }
                "antenna" => {
                    // `antenna <conductor_layer> <gate_layer> <max_ratio>`
                    let gate = layer_at(2, "antenna: expected `<conductor> <gate> <max_ratio>`")?;
                    let max_ratio: f64 = toks
                        .get(3)
                        .and_then(|s| s.parse().ok())
                        .ok_or_else(|| err("antenna: expected a numeric `<max_ratio>`"))?;
                    if !max_ratio.is_finite() || max_ratio <= 0.0 {
                        return Err(err("antenna max_ratio must be a finite number > 0"));
                    }
                    r.antenna.push(Antenna {
                        conductor: layer,
                        gate,
                        max_ratio,
                    });
                }
                "enclosure" | "enc" => {
                    // `enclosure <outer> <inner> <min>`
                    let inner = layer_at(2, "enclosure: expected `<outer> <inner> <min>`")?;
                    let min: i64 = toks
                        .get(3)
                        .and_then(|s| s.parse().ok())
                        .ok_or_else(|| err("enclosure: expected an integer `<min>` (DB units)"))?;
                    r.enclosure.push(Enclosure {
                        outer: layer,
                        inner,
                        min,
                    });
                }
                "span" => {
                    // `span <cut_layer> <metal_layer>`
                    let metal = layer_at(2, "span: expected `<cut_layer> <metal_layer>`")?;
                    r.span.push(Span { cut: layer, metal });
                }
                "venc" => {
                    // `venc <outer> <inner> <major> <minor>`
                    let inner = layer_at(2, "venc: expected `<outer> <inner> <major> <minor>`")?;
                    let major = arg(1, "venc: expected an integer `<major>` (DB units)")?;
                    let minor = arg(2, "venc: expected an integer `<minor>` (DB units)")?;
                    if minor < 0 || major < minor {
                        return Err(err("venc: need major ≥ minor ≥ 0"));
                    }
                    r.venc.push(Venc {
                        outer: layer,
                        inner,
                        major,
                        minor,
                    });
                }
                "grid" => {
                    // `grid <layer> <xpitch> <ypitch>`
                    let xpitch = arg(0, "grid: expected `<layer> <xpitch> <ypitch>`")?;
                    let ypitch = arg(1, "grid: expected `<layer> <xpitch> <ypitch>`")?;
                    if xpitch <= 0 || ypitch <= 0 {
                        return Err(err("grid: xpitch and ypitch must be > 0"));
                    }
                    r.grid.push(Grid {
                        layer,
                        xpitch,
                        ypitch,
                    });
                }
                "track" => {
                    // `track <layer> <width> <pitch> <offset>`
                    let width = arg(0, "track: expected `<layer> <width> <pitch> <offset>`")?;
                    let pitch = arg(1, "track: expected `<layer> <width> <pitch> <offset>`")?;
                    let offset = arg(2, "track: expected `<layer> <width> <pitch> <offset>`")?;
                    if width <= 0 || pitch <= 0 {
                        return Err(err("track: width and pitch must be > 0"));
                    }
                    r.track.push(Track {
                        layer,
                        width,
                        pitch,
                        offset,
                    });
                }
                "corner" => {
                    // `corner <outer_layer> <inner_layer>`
                    let inner = layer_at(2, "corner: expected `<outer_layer> <inner_layer>`")?;
                    r.corner.push(Corner {
                        outer: layer,
                        inner,
                    });
                }
                "sep" => {
                    // `sep <layer> <a_min> <a_max> <b_min> <b_max> <dist>` (max 0 = unbounded)
                    let e =
                        || err("sep: expected `<layer> <a_min> <a_max> <b_min> <b_max> <dist>`");
                    let s = Sep {
                        layer,
                        a_min: arg(0, "sep").map_err(|_| e())?,
                        a_max: arg(1, "sep").map_err(|_| e())?,
                        b_min: arg(2, "sep").map_err(|_| e())?,
                        b_max: arg(3, "sep").map_err(|_| e())?,
                        dist: arg(4, "sep").map_err(|_| e())?,
                    };
                    if s.dist <= 0 || s.a_min < 0 || s.b_min < 0 {
                        return Err(err("sep: dist > 0 and lengths ≥ 0"));
                    }
                    r.sep.push(s);
                }
                "c2c" => {
                    // `c2c <layer> <dist>`
                    let dist = arg(0, "c2c: expected `<layer> <dist>`")?;
                    if dist <= 0 {
                        return Err(err("c2c: dist must be > 0"));
                    }
                    r.c2c.push(C2c { layer, dist });
                }
                "runlen" => {
                    // `runlen <layer> <space> <min_run>`
                    let space = arg(0, "runlen: expected `<layer> <space> <min_run>`")?;
                    let min_run = arg(1, "runlen: expected `<layer> <space> <min_run>`")?;
                    if space <= 0 || min_run <= 0 {
                        return Err(err("runlen: space and min_run must be > 0"));
                    }
                    r.runlen.push(RunLen {
                        layer,
                        space,
                        min_run,
                    });
                }
                "fill" => {
                    // `fill <layer> <target_pct> <window> <size> <gap>`
                    let f = Fill {
                        layer,
                        target_pct: arg(
                            0,
                            "fill: expected `<layer> <target%> <window> <size> <gap>`",
                        )?,
                        window: arg(
                            1,
                            "fill: expected `<layer> <target%> <window> <size> <gap>`",
                        )?,
                        size: arg(
                            2,
                            "fill: expected `<layer> <target%> <window> <size> <gap>`",
                        )?,
                        gap: arg(
                            3,
                            "fill: expected `<layer> <target%> <window> <size> <gap>`",
                        )?,
                    };
                    if f.window <= 0 || f.size <= 0 || f.gap < 0 {
                        return Err(err("fill: window and size must be > 0, gap ≥ 0"));
                    }
                    if f.target_pct < 0 || f.target_pct > 100 {
                        return Err(err("fill: target% must be in 0..=100"));
                    }
                    r.fill.push(f);
                }
                other => return Err(err(&format!("unknown rule {other:?}"))),
            }
        }
        if r.width.is_empty()
            && r.space.is_empty()
            && r.area.is_empty()
            && r.density.is_empty()
            && r.antenna.is_empty()
            && r.enclosure.is_empty()
            && r.span.is_empty()
            && r.venc.is_empty()
            && r.grid.is_empty()
            && r.track.is_empty()
            && r.corner.is_empty()
            && r.sep.is_empty()
            && r.c2c.is_empty()
            && r.runlen.is_empty()
            && r.fill.is_empty()
        {
            return Err(RulesError("no rules defined".into()));
        }
        Ok(r)
    }

    pub fn load(path: &str) -> Result<Rules, RulesError> {
        let text = std::fs::read_to_string(path).map_err(|e| RulesError(format!("{path}: {e}")))?;
        Rules::parse(&text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn l(n: i16) -> Layer {
        Layer::new(n, 0)
    }

    #[test]
    fn parses_width_and_space() {
        let r = Rules::parse("# deck\nwidth 66 170\nspace 68 140\nspacing 66 150\n").unwrap();
        assert_eq!(r.width.get(&l(66)), Some(&170));
        assert_eq!(r.space.get(&l(68)), Some(&140));
        assert_eq!(r.space.get(&l(66)), Some(&150));
    }

    #[test]
    fn datatype_qualifies_the_layer() {
        // `num/dt` keys on the pair; a bare layer defaults to datatype 0, and two
        // datatypes on the same GDS layer number are distinct rules.
        let r = Rules::parse("width 66/20 150\nspace 66/44 340\nwidth 68 140\n").unwrap();
        assert_eq!(r.width.get(&Layer::new(66, 20)), Some(&150));
        assert_eq!(r.space.get(&Layer::new(66, 44)), Some(&340));
        assert_eq!(r.width.get(&Layer::new(66, 0)), None, "66/20 is not 66/0");
        assert_eq!(r.width.get(&l(68)), Some(&140), "bare layer == datatype 0");
        assert!(
            Rules::parse("width 66/xx 150\n").is_err(),
            "bad datatype rejected"
        );
    }

    #[test]
    fn parses_area_and_density() {
        let r = Rules::parse("area 68 20000\ndensity 68 20 70 100000\n").unwrap();
        assert_eq!(r.area.get(&l(68)), Some(&20000));
        let d = r.density.get(&l(68)).unwrap();
        assert_eq!((d.min_pct, d.max_pct, d.window), (20, 70, 100000));
    }

    #[test]
    fn parses_connect_and_antenna() {
        let r = Rules::parse("connect 66 67\nconnect 67 68\nantenna 68 5 400\n").unwrap();
        assert_eq!(r.connect, vec![(l(66), l(67)), (l(67), l(68))]);
        assert_eq!(r.antenna.len(), 1);
        let a = r.antenna[0];
        assert_eq!((a.conductor, a.gate), (l(68), l(5)));
        assert!((a.max_ratio - 400.0).abs() < 1e-9);
    }

    #[test]
    fn parses_enclosure_and_fill() {
        let r = Rules::parse("enclosure 68 66 40\nfill 68 30 1000 50 60\n").unwrap();
        assert_eq!(r.enclosure.len(), 1);
        assert_eq!(
            (
                r.enclosure[0].outer,
                r.enclosure[0].inner,
                r.enclosure[0].min
            ),
            (l(68), l(66), 40)
        );
        assert_eq!(r.fill.len(), 1);
        let f = r.fill[0];
        assert_eq!(
            (f.layer, f.target_pct, f.window, f.size, f.gap),
            (l(68), 30, 1000, 50, 60)
        );
    }

    #[test]
    fn parses_span() {
        let r = Rules::parse("span 66 68\nspan 25 34\n").unwrap();
        assert_eq!(r.span.len(), 2);
        assert_eq!((r.span[0].cut, r.span[0].metal), (l(66), l(68)));
        assert_eq!((r.span[1].cut, r.span[1].metal), (l(25), l(34)));
    }

    #[test]
    fn parses_venc() {
        let r = Rules::parse("venc 19 21 20 8\n").unwrap();
        assert_eq!(r.venc.len(), 1);
        let e = r.venc[0];
        assert_eq!((e.outer, e.inner, e.major, e.minor), (l(19), l(21), 20, 8));
    }

    #[test]
    fn parses_grid() {
        let r = Rules::parse("grid 40 1 96\ngrid 50 96 1\n").unwrap();
        assert_eq!(r.grid.len(), 2);
        assert_eq!(
            (r.grid[0].layer, r.grid[0].xpitch, r.grid[0].ypitch),
            (l(40), 1, 96)
        );
        assert_eq!(
            (r.grid[1].layer, r.grid[1].xpitch, r.grid[1].ypitch),
            (l(50), 96, 1)
        );
    }

    #[test]
    fn parses_track() {
        let r = Rules::parse("track 40 96 192 48\n").unwrap();
        assert_eq!(r.track.len(), 1);
        let t = r.track[0];
        assert_eq!((t.layer, t.width, t.pitch, t.offset), (l(40), 96, 192, 48));
    }

    #[test]
    fn rejects_garbage() {
        assert!(Rules::parse("width met1 170\n").is_err()); // non-numeric layer
        assert!(Rules::parse("# only comments\n").is_err()); // no rules
        assert!(Rules::parse("density 68 70 20 1000\n").is_err()); // min% > max%
        assert!(Rules::parse("density 68 20 70 0\n").is_err()); // window <= 0
        assert!(Rules::parse("density 68 20 70\n").is_err()); // missing window
        assert!(Rules::parse("antenna 68 5 0\n").is_err()); // ratio must be > 0
        assert!(Rules::parse("antenna 68 5\n").is_err()); // missing ratio
        assert!(Rules::parse("connect 66\n").is_err()); // connect needs two layers
        assert!(Rules::parse("enclosure 68 66\n").is_err()); // missing min
        assert!(Rules::parse("span 66\n").is_err()); // span needs two layers
        assert!(Rules::parse("venc 19 21 20\n").is_err()); // venc needs major and minor
        assert!(Rules::parse("venc 19 21 8 20\n").is_err()); // major must be >= minor
        assert!(Rules::parse("grid 40 1\n").is_err()); // grid needs both pitches
        assert!(Rules::parse("grid 40 0 96\n").is_err()); // pitch must be > 0
        assert!(Rules::parse("track 40 96 192\n").is_err()); // track needs offset
        assert!(Rules::parse("track 40 96 0 48\n").is_err()); // pitch must be > 0
        assert!(Rules::parse("fill 68 30 1000 50\n").is_err()); // missing gap
        assert!(Rules::parse("fill 68 200 1000 50 60\n").is_err()); // target% > 100
        assert!(Rules::parse("fill 68 30 0 50 60\n").is_err()); // window 0
    }
}
