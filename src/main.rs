//! vyges-drc CLI.
//!
//!   vyges-drc check GDS --rules DECK [--top CELL] [-o OUT] [--json] [--fail-on-violation]
//!   vyges-drc demo  [--json]                                     built-in layout
//!
//! Exit codes: 0 clean · 1 runtime error · 2 usage · 3 violations found
//! (only with --fail-on-violation).

use std::process::exit;

use vyges_drc::cluster;
use vyges_drc::drc::{self, Violation};
use vyges_drc::layout::gds::Library;
use vyges_drc::rules::Rules;
use vyges_drc::views;

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
  --views DIR    render ranked violation views (PNG) into DIR — diagnostic evidence,
                 not sign-off; capped, and the number dropped is reported
  --describe            print a machine-readable JSON description of the command
  -h, --help · -V, --version
";

/// Machine-readable description of `vyges-drc check` for tooling that drives the
/// command programmatically (parameters, how to build the invocation, output).
const DESCRIBE: &str = r#"{
  "name": "drc",
  "summary": "geometric design-rule check (GDS/OASIS layout + rule deck)",
  "maturity": "structured",
  "provenance_limitations": [
      "input_hash covers the argument vector, not the content of the GDS or rule deck it names.",
      "A rule deck that includes other decks is not enumerated, so those are outside the hash."
  ],
  "invocation": {
    "args_template": ["check", "{gds}", "--rules", "{deck}"],
    "optional": [ { "arg": "top", "flag": "--top" }, { "arg": "out", "flag": "-o" } ],
    "emits_json": true
  },
  "inputs": {
    "type": "object",
    "required": ["gds", "deck"],
    "properties": {
      "gds":  { "type": "string", "description": "layout file to check (.gds or .oas)" },
      "deck": { "type": "string", "description": "the .drc rule deck" },
      "top":  { "type": "string", "description": "top cell to flatten (default: the sole cell)" },
      "out":  { "type": "string", "description": "write the report to FILE instead of stdout" }
    }
  },
  "artifacts": [
    { "role": "drc_report", "field": "report_path" },
    { "role": "drc_view",   "field": "view_paths" }
  ],
  "assertion": {
    "id": "drc-clean",
    "field": "clean",
    "pass_when": { "is_true": true }
  }
}
"#;

/// The grouped view, ahead of the flat list.
///
/// A flat list of thousands of violations hides its own shape: the first screenful is one
/// defect repeated, and the single catastrophic one is somewhere past the truncation. Both
/// orderings are printed because neither answers the other's question — count says what to
/// fix to clear the report, severity says what is most broken.
fn render_clusters(viols: &[Violation]) -> String {
    let by_count = cluster::cluster(viols);
    if by_count.len() < 2 {
        return String::new(); // one group is not a summary, it is the list again
    }
    let row = |c: &cluster::Cluster| {
        let spread = if c.distinct_values == 1 {
            " (one repeated value)".to_string()
        } else {
            format!(" ({} distinct values)", c.distinct_values)
        };
        format!(
            "    {:<9} layer {:<4} {:>6} ×   worst {} vs {}  (misses by {:.0}%){}\n",
            c.rule,
            c.layer,
            c.count,
            c.value,
            c.limit,
            c.severity * 100.0,
            spread
        )
    };
    let mut s = String::from("\n  most occurrences:\n");
    for c in by_count.iter().take(8) {
        s.push_str(&row(c));
    }
    // Only worth printing when it actually differs; an identical list twice is noise.
    let by_sev = cluster::by_severity(&by_count);
    let same = by_sev
        .iter()
        .zip(by_count.iter())
        .all(|(a, b)| a.rule == b.rule && a.layer == b.layer);
    if !same {
        s.push_str("\n  worst misses:\n");
        for c in by_sev.iter().take(8) {
            s.push_str(&row(c));
        }
    }
    s.push('\n');
    s
}

fn render_text(viols: &[Violation], db_unit: f64) -> String {
    let um = |dbu: i64| dbu as f64 * db_unit * 1e6; // DB units -> µm
    let mut s = String::new();
    if viols.is_empty() {
        s.push_str("vyges-drc — CLEAN ✓  (no violations)\n");
        return s;
    }
    s.push_str(&format!("vyges-drc — {} violation(s) ✗\n", viols.len()));
    s.push_str(&render_clusters(viols));
    for v in viols.iter().take(200) {
        let at = format!("({},{})-({},{})", v.a.x0, v.a.y0, v.a.x1, v.a.y1);
        // the measured-vs-bound clause, in the rule's own unit
        let clause = match v.rule {
            "area" => format!("{} dbu² < min {} dbu²", v.value, v.limit),
            "density" if v.value < v.limit => format!("{}% coverage < min {}%", v.value, v.limit),
            "density" => format!("{}% coverage > max {}%", v.value, v.limit),
            // antenna: value / limit are centi-ratio (ratio × 100)
            "antenna" => format!(
                "ratio {:.2} > max {:.2}",
                v.value as f64 / 100.0,
                v.limit as f64 / 100.0
            ),
            // enclosure: value < 0 is the "not enclosed at all" sentinel
            "enclosure" if v.value < 0 => format!("not enclosed (need {} margin)", v.limit),
            "enclosure" => format!("enclosure {} dbu < min {}", v.value, v.limit),
            // span: value is the total edge deviation from spanning the metal width
            "span" => format!(
                "cut off metal-width by {} dbu (edges not coincident)",
                v.value
            ),
            // venc: value < 0 is the "not enclosed by a single outer" sentinel
            "venc" if v.value < 0 => format!(
                "not enclosed by a single outer (need {} on one axis)",
                v.limit
            ),
            "venc" => format!(
                "enclosure {} dbu < required {} on any single axis",
                v.value, v.limit
            ),
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
            None if v.rule == "density" => s.push_str(&format!(
                "  density layer {}: {clause} in window {at}\n",
                v.layer
            )),
            None if v.rule == "antenna" => s.push_str(&format!(
                "  antenna layer {}: {clause} on net {at}\n",
                v.layer
            )),
            None => s.push_str(&format!(
                "  {} layer {}: {clause} at {at}\n",
                v.rule, v.layer
            )),
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
    let clusters = cluster::cluster(viols);
    s.push_str("  \"clusters\": [\n");
    for (i, c) in clusters.iter().enumerate() {
        let comma = if i + 1 < clusters.len() { "," } else { "" };
        s.push_str(&format!(
            "    {{\"rule\": \"{}\", \"layer\": {}, \"count\": {}, \"distinct_values\": {}, \
             \"value\": {}, \"limit\": {}, \"severity\": {:.6}, \
             \"worst\": [{}, {}, {}, {}], \"extent\": [{}, {}, {}, {}]}}{}\n",
            c.rule,
            c.layer,
            c.count,
            c.distinct_values,
            c.value,
            c.limit,
            c.severity,
            c.worst.x0,
            c.worst.y0,
            c.worst.x1,
            c.worst.y1,
            c.extent.x0,
            c.extent.y0,
            c.extent.x1,
            c.extent.y1,
            comma
        ));
    }
    s.push_str("  ],\n");
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
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1).cloned())
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
            if viols.is_empty() {
                Severity::Info
            } else {
                Severity::Warn
            },
            format!("drc check complete: {} violation(s)", viols.len()),
        )
        .with_code("DRC-DONE"),
    );
}

/// Add `"report_path"` to a `--json` payload so the result says where its report landed.
///
/// String surgery rather than a JSON round-trip because this crate is std-only. Inserting
/// after the opening brace keeps every existing field untouched; an empty object gets no
/// trailing comma.
/// Splice the rendered view paths into the JSON payload as `view_paths`.
///
/// An array, in ranking order: the descriptor declares one `drc_view` artifact whose field
/// yields many paths, so the envelope hashes each of them centrally rather than this engine
/// hashing its own output and reporting it pre-digested.
fn with_view_paths(json: &str, views: &[views::View], provenance: &str) -> String {
    let Some(rest) = json.trim_start().strip_prefix('{') else {
        return json.to_string();
    };
    let list = views
        .iter()
        .map(|v| {
            let esc = v.path.replace('\\', "\\\\").replace('"', "\\\"");
            format!("\"{esc}\"")
        })
        .collect::<Vec<_>>()
        .join(", ");
    let sep = if rest.trim_start().starts_with('}') {
        ""
    } else {
        ","
    };
    let esc_prov = provenance.replace('\\', "\\\\").replace('"', "\\\"");
    // The provenance rides even when nothing was rendered: it describes the CHECK — which
    // deck, how many rules, and which layers went unexamined — not the pictures. A machine
    // consumer reading only this JSON needs it either way, and it is the PARTIAL COVERAGE
    // clause that decides whether "clean" means anything.
    if views.is_empty() {
        return format!("{{\"provenance\": \"{esc_prov}\"{sep}{rest}");
    }
    let esc_disc = views::DISCLAIMER.replace('"', "\\\"");
    format!(
        "{{\"view_paths\": [{list}], \"views_disclaimer\": \"{esc_disc}\", \
         \"provenance\": \"{esc_prov}\"{sep}{rest}"
    )
}

fn with_report_path(json: &str, path: Option<&str>) -> String {
    let (Some(p), Some(rest)) = (path, json.trim_start().strip_prefix('{')) else {
        return json.to_string();
    };
    let esc = p.replace('\\', "\\\\").replace('"', "\\\"");
    let sep = if rest.trim_start().starts_with('}') {
        ""
    } else {
        ","
    };
    format!("{{\"report_path\": \"{esc}\"{sep}{rest}")
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

    let (viols, cell) =
        drc::check_library_parts(&lib, top.as_deref(), &rules).unwrap_or_else(|e| {
            eprintln!("error: {e}");
            exit(1);
        });
    emit_events(&viols); // vyges-events causal trail on stderr; the report goes to stdout / -o

    // What the deck did NOT look at. A clean result on an unexamined layer is not evidence of
    // correctness, and a reader not told so will assume otherwise — so this rides with the
    // verdict everywhere the verdict goes, not in a footnote.
    let unchecked = drc::unchecked_layers(&cell, &rules);
    let provenance = views::provenance(&rules, &unchecked);
    if !unchecked.is_empty() {
        eprintln!(
            "warning: {} layer(s) carry geometry no rule examines — this run is PARTIAL",
            unchecked.len()
        );
    }

    // Ranked visual evidence, when asked for. Rendered from the same flattened cell the
    // checks ran on, so a view cannot depict different geometry than the finding it
    // illustrates. Opt-in: it writes files, and a check that silently littered a directory
    // would be a surprise.
    let views = match opt(&args, "--views") {
        Some(dir) if !viols.is_empty() => {
            let clusters = cluster::cluster(&viols);
            match views::render(&cell, &clusters, std::path::Path::new(&dir)) {
                Ok((vs, dropped)) => {
                    eprintln!(
                        "wrote {} view(s) to {dir} — {} {provenance}",
                        vs.len(),
                        views::DISCLAIMER
                    );
                    if dropped > 0 {
                        // Never let a capped set read as the complete one.
                        eprintln!(
                            "note: {dropped} further cluster(s) not rendered (cap is {} views)",
                            views::MAX_VIEWS
                        );
                    }
                    vs
                }
                Err(e) => {
                    // Failing to draw a picture must not fail the check: the verdict stands
                    // on the geometry, not on the illustration.
                    eprintln!("warning: could not render views: {e}");
                    Vec::new()
                }
            }
        }
        _ => Vec::new(),
    };

    let text = if json {
        let j = with_view_paths(&render_json(&viols), &views, &provenance);
        with_report_path(&j, opt(&args, "-o").as_deref())
    } else {
        let mut t = render_text(&viols, lib.db_unit);
        t.push_str(&format!("\n  {provenance}\n"));
        t
    };
    match opt(&args, "-o") {
        Some(path) => {
            if let Err(e) = std::fs::write(&path, &text) {
                eprintln!("error: {path}: {e}");
                exit(1);
            }
            eprintln!("wrote {path}");
            // `-o` writes the report; the machine payload still goes to stdout, so asking
            // for the file does not cost the caller the parsed result.
            if json {
                print!("{text}");
            }
        }
        None => print!("{text}"),
    }
    if fail_on && !viols.is_empty() {
        exit(3);
    }
}
