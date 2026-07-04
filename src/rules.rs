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
//! Layer **names** (a `layer <name> <num>` mapping) and datatype qualification are
//! depth items; v0 keys on the raw GDS layer number, which is unambiguous and PDK-
//! independent.

use std::collections::BTreeMap;

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
    pub conductor: i16,
    pub gate: i16,
    pub max_ratio: f64,
}

/// An enclosure rule: every `inner`-layer shape must sit inside an `outer`-layer
/// shape with at least `min` DB units of margin on every side (e.g. metal must
/// enclose a via/cut).
#[derive(Debug, Clone, Copy)]
pub struct Enclosure {
    pub outer: i16,
    pub inner: i16,
    pub min: i64,
}

/// A via-span rule: every `cut`-layer shape sitting on a `metal`-layer shape must
/// span the metal's full width (its shorter dimension), edges coincident on both
/// width-sides. A cut narrower than the metal, shifted, or protruding past the metal
/// edge violates — the generic form of an advanced-node "via lands on the full wire
/// width" rule.
#[derive(Debug, Clone, Copy)]
pub struct Span {
    pub cut: i16,
    pub metal: i16,
}

/// An asymmetric via-enclosure rule: every `inner`-layer shape must be enclosed by a
/// single `outer`-layer shape such that, on at least one axis, the enclosure on **both**
/// opposite sides is ≥ `minor` and on **at least one** side is ≥ `major`. This is the
/// generic advanced-node "via line-end / side" enclosure — a large enclosure along the
/// routing direction and a small one across it, required on only one axis. An inner not
/// enclosed by any single outer shape, or meeting the margins on neither axis, violates.
#[derive(Debug, Clone, Copy)]
pub struct Venc {
    pub outer: i16,
    pub inner: i16,
    pub major: i64,
    pub minor: i64,
}

/// A manufacturing-grid rule: every shape vertex on the layer must lie on the grid — its
/// x a multiple of `xpitch` and its y a multiple of `ypitch` (DB units). A pitch of 1
/// leaves that axis unconstrained. Flags each distinct off-grid vertex on the layer.
#[derive(Debug, Clone, Copy)]
pub struct Grid {
    pub layer: i16,
    pub xpitch: i64,
    pub ypitch: i64,
}

/// A routing-track rule: a **minimum-width** wire (short dimension equal to `width`) must
/// be centered on the routing-track grid — its centerline, on the width axis, a multiple
/// of `pitch` offset by `offset` (DB units). The generic advanced-node "min-width tracks
/// lie on the routing grid" rule.
#[derive(Debug, Clone, Copy)]
pub struct Track {
    pub layer: i16,
    pub width: i64,
    pub pitch: i64,
    pub offset: i64,
}

/// A metal-fill rule (drives the `fill` generator, not the checker): top up
/// `layer` coverage to at least `target_pct` per square `window`, by tiling
/// `size`-square fill shapes that keep `gap` clearance from existing geometry.
#[derive(Debug, Clone, Copy)]
pub struct Fill {
    pub layer: i16,
    pub target_pct: i64,
    pub window: i64,
    pub size: i64,
    pub gap: i64,
}

#[derive(Debug, Clone, Default)]
pub struct Rules {
    /// layer → minimum width (DB units).
    pub width: BTreeMap<i16, i64>,
    /// layer → minimum spacing (DB units).
    pub space: BTreeMap<i16, i64>,
    /// layer → minimum polygon area (DB units²).
    pub area: BTreeMap<i16, i64>,
    /// layer → windowed metal-density bounds.
    pub density: BTreeMap<i16, Density>,
    /// layer pairs that electrically connect where they overlap (vias / contacts).
    /// Same-layer overlap/touch always connects; cross-layer needs a `connect` rule.
    pub connect: Vec<(i16, i16)>,
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
    pub fn parse(text: &str) -> Result<Rules, RulesError> {
        let mut r = Rules::default();
        for (n, raw) in text.lines().enumerate() {
            let line = raw.split('#').next().unwrap_or("");
            let toks: Vec<&str> = line.split_whitespace().collect();
            if toks.is_empty() {
                continue;
            }
            let err = |what: &str| RulesError(format!("line {}: {what}: {:?}", n + 1, raw.trim()));
            let layer: i16 = toks
                .get(1)
                .and_then(|s| s.parse().ok())
                .ok_or_else(|| err("expected `<rule> <layer> ...`"))?;
            // parse the k-th argument (after `<rule> <layer>`) as an integer
            let arg = |k: usize, what: &str| -> Result<i64, RulesError> {
                toks.get(2 + k).and_then(|s| s.parse().ok()).ok_or_else(|| err(what))
            };
            match toks[0].to_ascii_lowercase().as_str() {
                "width" => {
                    r.width.insert(layer, arg(0, "expected an integer minimum width (DB units)")?);
                }
                "space" | "spacing" => {
                    r.space.insert(layer, arg(0, "expected an integer minimum spacing (DB units)")?);
                }
                "area" => {
                    r.area.insert(layer, arg(0, "expected an integer minimum area (DB units²)")?);
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
                    let b: i16 = toks
                        .get(2)
                        .and_then(|s| s.parse().ok())
                        .ok_or_else(|| err("connect: expected `<layerA> <layerB>`"))?;
                    r.connect.push((layer, b));
                }
                "antenna" => {
                    // `antenna <conductor_layer> <gate_layer> <max_ratio>`
                    let gate: i16 = toks
                        .get(2)
                        .and_then(|s| s.parse().ok())
                        .ok_or_else(|| err("antenna: expected `<conductor> <gate> <max_ratio>`"))?;
                    let max_ratio: f64 = toks
                        .get(3)
                        .and_then(|s| s.parse().ok())
                        .ok_or_else(|| err("antenna: expected a numeric `<max_ratio>`"))?;
                    if !max_ratio.is_finite() || max_ratio <= 0.0 {
                        return Err(err("antenna max_ratio must be a finite number > 0"));
                    }
                    r.antenna.push(Antenna { conductor: layer, gate, max_ratio });
                }
                "enclosure" | "enc" => {
                    // `enclosure <outer> <inner> <min>`
                    let inner: i16 = toks
                        .get(2)
                        .and_then(|s| s.parse().ok())
                        .ok_or_else(|| err("enclosure: expected `<outer> <inner> <min>`"))?;
                    let min: i64 = toks
                        .get(3)
                        .and_then(|s| s.parse().ok())
                        .ok_or_else(|| err("enclosure: expected an integer `<min>` (DB units)"))?;
                    r.enclosure.push(Enclosure { outer: layer, inner, min });
                }
                "span" => {
                    // `span <cut_layer> <metal_layer>`
                    let metal: i16 = toks
                        .get(2)
                        .and_then(|s| s.parse().ok())
                        .ok_or_else(|| err("span: expected `<cut_layer> <metal_layer>`"))?;
                    r.span.push(Span { cut: layer, metal });
                }
                "venc" => {
                    // `venc <outer> <inner> <major> <minor>`
                    let inner: i16 = toks
                        .get(2)
                        .and_then(|s| s.parse().ok())
                        .ok_or_else(|| err("venc: expected `<outer> <inner> <major> <minor>`"))?;
                    let major = arg(1, "venc: expected an integer `<major>` (DB units)")?;
                    let minor = arg(2, "venc: expected an integer `<minor>` (DB units)")?;
                    if minor < 0 || major < minor {
                        return Err(err("venc: need major ≥ minor ≥ 0"));
                    }
                    r.venc.push(Venc { outer: layer, inner, major, minor });
                }
                "grid" => {
                    // `grid <layer> <xpitch> <ypitch>`
                    let xpitch = arg(0, "grid: expected `<layer> <xpitch> <ypitch>`")?;
                    let ypitch = arg(1, "grid: expected `<layer> <xpitch> <ypitch>`")?;
                    if xpitch <= 0 || ypitch <= 0 {
                        return Err(err("grid: xpitch and ypitch must be > 0"));
                    }
                    r.grid.push(Grid { layer, xpitch, ypitch });
                }
                "track" => {
                    // `track <layer> <width> <pitch> <offset>`
                    let width = arg(0, "track: expected `<layer> <width> <pitch> <offset>`")?;
                    let pitch = arg(1, "track: expected `<layer> <width> <pitch> <offset>`")?;
                    let offset = arg(2, "track: expected `<layer> <width> <pitch> <offset>`")?;
                    if width <= 0 || pitch <= 0 {
                        return Err(err("track: width and pitch must be > 0"));
                    }
                    r.track.push(Track { layer, width, pitch, offset });
                }
                "fill" => {
                    // `fill <layer> <target_pct> <window> <size> <gap>`
                    let f = Fill {
                        layer,
                        target_pct: arg(0, "fill: expected `<layer> <target%> <window> <size> <gap>`")?,
                        window: arg(1, "fill: expected `<layer> <target%> <window> <size> <gap>`")?,
                        size: arg(2, "fill: expected `<layer> <target%> <window> <size> <gap>`")?,
                        gap: arg(3, "fill: expected `<layer> <target%> <window> <size> <gap>`")?,
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

    #[test]
    fn parses_width_and_space() {
        let r = Rules::parse("# deck\nwidth 66 170\nspace 68 140\nspacing 66 150\n").unwrap();
        assert_eq!(r.width.get(&66), Some(&170));
        assert_eq!(r.space.get(&68), Some(&140));
        assert_eq!(r.space.get(&66), Some(&150));
    }

    #[test]
    fn parses_area_and_density() {
        let r = Rules::parse("area 68 20000\ndensity 68 20 70 100000\n").unwrap();
        assert_eq!(r.area.get(&68), Some(&20000));
        let d = r.density.get(&68).unwrap();
        assert_eq!((d.min_pct, d.max_pct, d.window), (20, 70, 100000));
    }

    #[test]
    fn parses_connect_and_antenna() {
        let r = Rules::parse("connect 66 67\nconnect 67 68\nantenna 68 5 400\n").unwrap();
        assert_eq!(r.connect, vec![(66, 67), (67, 68)]);
        assert_eq!(r.antenna.len(), 1);
        let a = r.antenna[0];
        assert_eq!((a.conductor, a.gate), (68, 5));
        assert!((a.max_ratio - 400.0).abs() < 1e-9);
    }

    #[test]
    fn parses_enclosure_and_fill() {
        let r = Rules::parse("enclosure 68 66 40\nfill 68 30 1000 50 60\n").unwrap();
        assert_eq!(r.enclosure.len(), 1);
        assert_eq!((r.enclosure[0].outer, r.enclosure[0].inner, r.enclosure[0].min), (68, 66, 40));
        assert_eq!(r.fill.len(), 1);
        let f = r.fill[0];
        assert_eq!((f.layer, f.target_pct, f.window, f.size, f.gap), (68, 30, 1000, 50, 60));
    }

    #[test]
    fn parses_span() {
        let r = Rules::parse("span 66 68\nspan 25 34\n").unwrap();
        assert_eq!(r.span.len(), 2);
        assert_eq!((r.span[0].cut, r.span[0].metal), (66, 68));
        assert_eq!((r.span[1].cut, r.span[1].metal), (25, 34));
    }

    #[test]
    fn parses_venc() {
        let r = Rules::parse("venc 19 21 20 8\n").unwrap();
        assert_eq!(r.venc.len(), 1);
        let e = r.venc[0];
        assert_eq!((e.outer, e.inner, e.major, e.minor), (19, 21, 20, 8));
    }

    #[test]
    fn parses_grid() {
        let r = Rules::parse("grid 40 1 96\ngrid 50 96 1\n").unwrap();
        assert_eq!(r.grid.len(), 2);
        assert_eq!((r.grid[0].layer, r.grid[0].xpitch, r.grid[0].ypitch), (40, 1, 96));
        assert_eq!((r.grid[1].layer, r.grid[1].xpitch, r.grid[1].ypitch), (50, 96, 1));
    }

    #[test]
    fn parses_track() {
        let r = Rules::parse("track 40 96 192 48\n").unwrap();
        assert_eq!(r.track.len(), 1);
        let t = r.track[0];
        assert_eq!((t.layer, t.width, t.pitch, t.offset), (40, 96, 192, 48));
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
