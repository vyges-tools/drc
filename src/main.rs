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
vyges-drc — geometric design-rule check (GDS + rule deck -> violations)

usage:
  vyges-drc check GDS --rules DECK [--top CELL] [-o OUT] [--json] [--fail-on-violation]
  vyges-drc demo  [--json]

flags:
  --rules DECK          the .drc rule deck (required for `check`)
  --top CELL            top cell to flatten (default: the sole cell)
  -o FILE               write the report to FILE (default: stdout)
  --json                machine-readable JSON instead of text
  --fail-on-violation   exit 3 when any violation is found (CI gate)
  -h, --help · -V, --version
";

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
            // width / space: linear DB units, show µm too
            _ => format!("{} dbu ({:.4} µm) < min {}", v.value, um(v.value), v.limit),
        };
        match v.b {
            None if v.rule == "density" => {
                s.push_str(&format!("  density layer {}: {clause} in window {at}\n", v.layer))
            }
            None => s.push_str(&format!("  {} layer {}: {clause} at {at}\n", v.rule, v.layer)),
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
    let json = args.iter().any(|a| a == "--json");
    let fail_on = args.iter().any(|a| a == "--fail-on-violation");

    let (lib, rules, top) = match args[0].as_str() {
        "check" => {
            let Some(gds) = args.get(1).filter(|a| !a.starts_with('-')) else {
                eprintln!("error: `check` needs a GDS path\n{USAGE}");
                exit(2);
            };
            let Some(deck) = opt(&args, "--rules") else {
                eprintln!("error: `check` needs --rules DECK\n{USAGE}");
                exit(2);
            };
            let lib = Library::load(gds).unwrap_or_else(|e| {
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
                    ],
                }],
                ..Library::default()
            };
            let deck = "width 66 100\nspace 68 100\narea 72 10000\ndensity 70 50 90 1000\n";
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
