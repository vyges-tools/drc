//! vyges-drc CLI.
//!
//!   vyges-drc check GDS --rules DECK [--top CELL] [-o OUT] [--json] [--fail-on-violation]
//!   vyges-drc demo  [--json]                                     built-in layout
//!
//! Exit codes: 0 clean · 1 runtime error · 2 usage · 3 violations found
//! (only with --fail-on-violation).

use std::process::exit;

use vyges_drc::drc::{self, Violation};
use vyges_drc::layout::gds::Library;
use vyges_drc::rules::Rules;

const USAGE: &str = "\
vyges-drc — geometric design-rule check (GDS/OASIS + rule deck -> violations)

usage:
  vyges-drc check GDS --rules DECK [--top CELL] [-o OUT] [--json] [--fail-on-violation]
  vyges-drc fill  GDS --rules DECK [--top CELL] -o OUT.gds     # metal-fill generator
  vyges-drc demo  [--json]

The input layout may be GDSII (.gds) or OASIS (.oas/.oasis) — picked by extension.

flags:
  --rules DECK          the .drc rule deck (required for `check` / `fill`)
  --pdk NAME            resolve the deck from pdk-store (drc_deck) instead of --rules
  --top CELL            top cell to flatten (default: the sole cell)
  -o FILE               write the report (or, for `fill`, the filled GDS) to FILE
  --json                machine-readable JSON instead of text
  --fail-on-violation   exit 3 when any violation is found (CI gate)
  --describe            print a machine-readable JSON description of the command
  -h, --help · -V, --version
";

/// Machine-readable description of `vyges-drc check` for tooling that drives the
/// command programmatically (parameters, how to build the invocation, output).
const DESCRIBE: &str = r#"{
  "name": "drc",
  "summary": "geometric design-rule check (GDS/OASIS layout + rule deck)",
  "invocation": {
    "args_template": ["check", "{gds}", "--rules", "{deck}"],
    "optional": [ { "arg": "top", "flag": "--top" } ],
    "emits_json": true
  },
  "inputs": {
    "type": "object",
    "required": ["gds", "deck"],
    "properties": {
      "gds":  { "type": "string", "description": "layout file to check (.gds or .oas)" },
      "deck": { "type": "string", "description": "the .drc rule deck" },
      "top":  { "type": "string", "description": "top cell to flatten (default: the sole cell)" }
    }
  },
  "artifacts": [ { "role": "drc_report" } ]
}
"#;

fn render_text(viols: &[Violation], db_unit: f64) -> String {
    let um = |dbu: i64| dbu as f64 * db_unit * 1e6; // DB units -> µm
    let mut s = String::new();
    if viols.is_empty() {
        s.push_str("vyges-drc — CLEAN ✓  (no violations)\n");
        return s;
    }
    s.push_str(&format!("vyges-drc — {} violation(s) ✗\n", viols.len()));
    for v in viols.iter().take(200) {
        let at = format!("({},{})-({},{})", v.a.x0, v.a.y0, v.a.x1, v.a.y1);
        // the measured-vs-bound clause, in the rule's own unit
        let clause = match v.rule {
            "area" => format!("{} dbu² < min {} dbu²", v.value, v.limit),
            "density" if v.value < v.limit => format!("{}% coverage < min {}%", v.value, v.limit),
            "density" => format!("{}% coverage > max {}%", v.value, v.limit),
            // antenna: value / limit are centi-ratio (ratio × 100)
            "antenna" => format!("ratio {:.2} > max {:.2}", v.value as f64 / 100.0, v.limit as f64 / 100.0),
            // enclosure: value < 0 is the "not enclosed at all" sentinel
            "enclosure" if v.value < 0 => format!("not enclosed (need {} margin)", v.limit),
            "enclosure" => format!("enclosure {} dbu < min {}", v.value, v.limit),
            // span: value is the total edge deviation from spanning the metal width
            "span" => format!("cut off metal-width by {} dbu (edges not coincident)", v.value),
            // venc: value < 0 is the "not enclosed by a single outer" sentinel
            "venc" if v.value < 0 => format!("not enclosed by a single outer (need {} on one axis)", v.limit),
            "venc" => format!("enclosure {} dbu < required {} on any single axis", v.value, v.limit),
            // grid: the offending vertex is carried in the location field
            "grid" => "off-grid vertex".to_string(),
            // track: a min-width wire off the routing-track centerline grid
            "track" => "min-width wire off the routing-track centerline".to_string(),
            // corner: a via corner not on the merged outer (metal) boundary
            "corner" => "via corner departs the metal boundary on both edges".to_string(),
            // sep: directional edge-to-edge spacing (tip-to-side / tip-to-tip)
            "sep" => format!("edge spacing {} dbu < min {}", v.value, v.limit),
            // c2c: corner-to-corner spacing
            "c2c" => format!("corner-to-corner {} dbu < min {}", v.value, v.limit),
            // runlen: parallel run too short at tight spacing
            "runlen" => format!("parallel run {} dbu < min {}", v.value, v.limit),
            // width / space: linear DB units, show µm too
            _ => format!("{} dbu ({:.4} µm) < min {}", v.value, um(v.value), v.limit),
        };
        match v.b {
            None if v.rule == "density" => {
                s.push_str(&format!("  density layer {}: {clause} in window {at}\n", v.layer))
            }
            None if v.rule == "antenna" => {
                s.push_str(&format!("  antenna layer {}: {clause} on net {at}\n", v.layer))
            }
            None => s.push_str(&format!("  {} layer {}: {clause} at {at}\n", v.rule, v.layer)),
            Some(b) if v.rule == "span" => s.push_str(&format!(
                "  span layer {}: {clause} — cut {at} on metal ({},{})-({},{})\n",
                v.layer, b.x0, b.y0, b.x1, b.y1
            )),
            Some(b) => s.push_str(&format!(
                "  {} layer {}: {clause} between {at} and ({},{})-({},{})\n",
                v.rule, v.layer, b.x0, b.y0, b.x1, b.y1
            )),
        }
    }
    if viols.len() > 200 {
        s.push_str(&format!("  … {} more\n", viols.len() - 200));
    }
    s
}

fn render_json(viols: &[Violation]) -> String {
    let mut s = String::from("{\n");
    s.push_str(&format!("  \"clean\": {},\n", viols.is_empty()));
    s.push_str(&format!("  \"violations\": {},\n", viols.len()));
    s.push_str("  \"items\": [\n");
    for (i, v) in viols.iter().enumerate() {
        let comma = if i + 1 < viols.len() { "," } else { "" };
        s.push_str(&format!(
            "    {{\"rule\": \"{}\", \"layer\": {}, \"value\": {}, \"limit\": {}}}{}\n",
            v.rule, v.layer, v.value, v.limit, comma
        ));
    }
    s.push_str("  ]\n}\n");
    s
}

fn opt(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1).cloned())
}

/// Resolve a PDK collateral key (e.g. `drc_deck`) to a concrete path via the
/// installed `vyges-pdk-store` resolver — the PDK adapter. Prefers the sibling
/// binary next to this one, else falls back to PATH. Returns None if unavailable.
/// Resolve a PDK collateral key (e.g. `drc_deck`) via the shared foundation
/// resolver (the `pdk-store` adapter, with `$VYGES_PLUGIN` fallback + detailed
/// errors). See `vyges_layout::pdk::resolve`.
fn pdk_resolve(pdk: &str, key: &str) -> Result<String, String> {
    vyges_layout::pdk::resolve(pdk, key, None)
}

/// The DRC deck: from `--rules DECK`, else resolved from `--pdk NAME` (drc_deck).
/// Exits with a clear message when `--pdk` is given but cannot be resolved.
fn deck_arg(args: &[String]) -> Option<String> {
    if let Some(r) = opt(args, "--rules") {
        return Some(r);
    }
    if let Some(p) = opt(args, "--pdk") {
        match pdk_resolve(&p, "drc_deck") {
            Ok(d) => return Some(d),
            Err(e) => {
                eprintln!("error: {e}");
                exit(2);
            }
        }
    }
    None
}

/// Emit the vyges-events causal trail — one event per DRC violation + a completion event.
/// Written to stderr (the default sink) so it never mixes with the report (stdout / -o).
/// `code` (DRC-<RULE>) is the clustering key; `objects` (layer) is the cross-stage co-ref key.
fn emit_events(viols: &[Violation]) {
    use vyges_events::{emit, Event, Severity};
    for v in viols {
        emit(
            &Event::new(
                "vyges-drc",
                Severity::Warn,
                format!(
                    "{} violation on layer {}: value {} vs limit {}",
                    v.rule, v.layer, v.value, v.limit
                ),
            )
            .with_code(format!("DRC-{}", v.rule.to_uppercase()))
            .with_objects(vec![format!("layer:{}", v.layer)]),
        );
    }
    emit(
        &Event::new(
            "vyges-drc",
            if viols.is_empty() { Severity::Info } else { Severity::Warn },
            format!("drc check complete: {} violation(s)", viols.len()),
        )
        .with_code("DRC-DONE"),
    );
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "-h" || a == "--help") || args.is_empty() {
        print!("{USAGE}");
        return;
    }
    if args.iter().any(|a| a == "-V" || a == "--version") {
        println!("vyges-drc {}", vyges_drc::VERSION);
        return;
    }
    if args.iter().any(|a| a == "--describe") {
        print!("{DESCRIBE}");
        return;
    }
    let json = args.iter().any(|a| a == "--json");
    let fail_on = args.iter().any(|a| a == "--fail-on-violation");

    // `fill` is a generator (GDS in -> filled GDS out), not a report — handle early.
    if args[0] == "fill" {
        let Some(gds) = args.get(1).filter(|a| !a.starts_with('-')) else {
            eprintln!("error: `fill` needs a GDS path\n{USAGE}");
            exit(2);
        };
        let Some(deck) = deck_arg(&args) else {
            eprintln!("error: `fill` needs --rules DECK or --pdk NAME\n{USAGE}");
            exit(2);
        };
        let Some(out) = opt(&args, "-o") else {
            eprintln!("error: `fill` needs -o OUT.gds\n{USAGE}");
            exit(2);
        };
        let lib = Library::load_any(gds).unwrap_or_else(|e| {
            eprintln!("error: {gds}: {e}");
            exit(1);
        });
        let rules = Rules::load(&deck).unwrap_or_else(|e| {
            eprintln!("error: {e}");
            exit(1);
        });
        if rules.fill.is_empty() {
            eprintln!("error: the deck defines no `fill` rules");
            exit(2);
        }
        let (filled, n) = drc::fill_library(&lib, opt(&args, "--top").as_deref(), &rules)
            .unwrap_or_else(|e| {
                eprintln!("error: {e}");
                exit(1);
            });
        filled.save(&out).unwrap_or_else(|e| {
            eprintln!("error: {out}: {e}");
            exit(1);
        });
        if !json {
            eprintln!("vyges-drc fill — added {n} fill shape(s) → {out}");
        }
        return;
    }

    let (lib, rules, top) = match args[0].as_str() {
        "check" => {
            let Some(gds) = args.get(1).filter(|a| !a.starts_with('-')) else {
                eprintln!("error: `check` needs a GDS path\n{USAGE}");
                exit(2);
            };
            let Some(deck) = deck_arg(&args) else {
                eprintln!("error: `check` needs --rules DECK or --pdk NAME\n{USAGE}");
                exit(2);
            };
            let lib = Library::load_any(gds).unwrap_or_else(|e| {
                eprintln!("error: {gds}: {e}");
                exit(1);
            });
            let rules = Rules::load(&deck).unwrap_or_else(|e| {
                eprintln!("error: {e}");
                exit(1);
            });
            (lib, rules, opt(&args, "--top"))
        }
        "demo" => {
            use vyges_drc::layout::gds::{Cell, Element};
            let bnd = |layer, x0, y0, x1, y1| Element::Boundary {
                layer,
                datatype: 0,
                pts: vec![(x0, y0), (x1, y0), (x1, y1), (x0, y1), (x0, y0)],
            };
            // a sampler hitting every rule class: a 50-wide poly (width<100), two
            // met1 wires 60 apart (space<100), a 40×40 pad (area<10000), and a sparse
            // layer-70 fill (20% coverage < min 50%).
            let lib = Library {
                cells: vec![Cell {
                    name: "demo".into(),
                    elements: vec![
                        bnd(66, 0, 0, 50, 400),
                        bnd(68, 0, 0, 100, 100),
                        bnd(68, 160, 0, 260, 100),
                        bnd(72, 0, 0, 40, 40),
                        bnd(70, 0, 500, 100, 600),
                        bnd(70, 900, 500, 1000, 600),
                        // antenna: a small gate (layer 5) tied to a long metal (layer 74)
                        bnd(5, 0, 700, 10, 710),
                        bnd(74, 0, 700, 1000, 720),
                        // enclosure: inner (77) only 10 inside its outer (76), min 40
                        bnd(76, 0, 800, 200, 1000),
                        bnd(77, 10, 810, 190, 990),
                    ],
                }],
                ..Library::default()
            };
            let deck = "width 66 100\nspace 68 100\narea 72 10000\ndensity 70 50 90 1000\n\
                        connect 5 74\nantenna 74 5 100\nenclosure 76 77 40\n";
            (lib, Rules::parse(deck).unwrap(), None)
        }
        other => {
            eprintln!("error: unknown command {other:?}\n{USAGE}");
            exit(2);
        }
    };

    let viols = drc::check_library(&lib, top.as_deref(), &rules).unwrap_or_else(|e| {
        eprintln!("error: {e}");
        exit(1);
    });
    emit_events(&viols); // vyges-events causal trail on stderr; the report goes to stdout / -o
    let text = if json { render_json(&viols) } else { render_text(&viols, lib.db_unit) };
    match opt(&args, "-o") {
        Some(path) => {
            if let Err(e) = std::fs::write(&path, &text) {
                eprintln!("error: {path}: {e}");
                exit(1);
            }
        }
        None => print!("{text}"),
    }
    if fail_on && !viols.is_empty() {
        exit(3);
    }
}
