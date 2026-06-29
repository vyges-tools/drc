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
                other => return Err(err(&format!("unknown rule {other:?}"))),
            }
        }
        if r.width.is_empty()
            && r.space.is_empty()
            && r.area.is_empty()
            && r.density.is_empty()
            && r.antenna.is_empty()
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
    fn rejects_garbage() {
        assert!(Rules::parse("width met1 170\n").is_err()); // non-numeric layer
        assert!(Rules::parse("# only comments\n").is_err()); // no rules
        assert!(Rules::parse("density 68 70 20 1000\n").is_err()); // min% > max%
        assert!(Rules::parse("density 68 20 70 0\n").is_err()); // window <= 0
        assert!(Rules::parse("density 68 20 70\n").is_err()); // missing window
        assert!(Rules::parse("antenna 68 5 0\n").is_err()); // ratio must be > 0
        assert!(Rules::parse("antenna 68 5\n").is_err()); // missing ratio
        assert!(Rules::parse("connect 66\n").is_err()); // connect needs two layers
    }
}
