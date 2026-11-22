//! The `mask-check` command end to end: a layout on two mask datatypes, a rule file, the exit
//! status, the JSON report and the results database.
#![cfg(unix)]

use std::process::Command;

use vyges_drc::layout::gds::{Cell, Element, Library};

fn bx(dt: i16, x0: i32, y0: i32, x1: i32, y1: i32) -> Element {
    Element::Boundary { layer: 20, datatype: dt, pts: vec![(x0, y0), (x1, y0), (x1, y1), (x0, y1), (x0, y0)] }
}

fn write(dir: &std::path::Path, els: Vec<Element>) -> String {
    let lib = Library {
        name: "lib".into(),
        user_unit: 0.001,
        db_unit: 1e-9,
        cells: vec![Cell { name: "blk".into(), elements: els }],
    };
    let p = dir.join("blk.gds");
    lib.save(p.to_str().expect("utf-8 temp path")).unwrap();
    p.to_str().unwrap().to_string()
}

fn run(args: &[&str]) -> (i32, String) {
    let o = Command::new(env!("CARGO_BIN_EXE_vyges-drc")).args(args).output().unwrap();
    (o.status.code().unwrap_or(-1), String::from_utf8_lossy(&o.stdout).into_owned())
}

/// Two mask-1 wires 50 apart break a same-mask spacing of 60: exit 3 with
/// --fail-on-violation, the rule named in the JSON with its count, the not-checked rule listed
/// as such, and the results database holding the one edge pair. On mask 2 the same pair is
/// clean (exit 0, status clean).
#[test]
fn mask_check_names_each_rule() {
    let dir = std::env::temp_dir().join(format!("vyges_drc_mask_check_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let rules = dir.join("r.mprules");
    std::fs::write(&rules, "space R.S.1_A 20/1 60\nspace2 R.S.2 20/1 20/2 32\nnotchecked R.C.4 stitch shape\n").unwrap();
    let rdb = dir.join("out.rdb");
    let gds = write(&dir, vec![bx(1, 0, 0, 1000, 30), bx(1, 0, 80, 1000, 110)]);
    let (rc, out) = run(&["mask-check", &gds, "--rules", rules.to_str().unwrap(), "--json", "--fail-on-violation", "--rdb", rdb.to_str().unwrap()]);
    assert_eq!(rc, 3, "{out}");
    assert!(out.contains("\"status\": \"results\""), "{out}");
    assert!(out.contains("{\"rule\": \"R.S.1_A\", \"kind\": \"space\", \"checked\": true, \"limit\": 60, \"count\": 1"), "{out}");
    assert!(out.contains("\"rule\": \"R.C.4\", \"kind\": \"notchecked\", \"checked\": false"), "{out}");
    let db = std::fs::read_to_string(&rdb).unwrap();
    assert!(db.starts_with("blk 1000\nR.S.1_A\n1 1 1\n"), "{db}");
    assert!(db.contains("R.S.2\n0 0 1\n"), "{db}");
    let gds = write(&dir, vec![bx(2, 0, 0, 1000, 30), bx(2, 0, 80, 1000, 110)]);
    let (rc, out) = run(&["mask-check", &gds, "--rules", rules.to_str().unwrap(), "--json", "--fail-on-violation"]);
    assert_eq!(rc, 0, "{out}");
    assert!(out.contains("\"status\": \"clean\""), "{out}");
    // a bad rule file is refused with its line
    std::fs::write(&rules, "space R 20/1\n").unwrap();
    let o = Command::new(env!("CARGO_BIN_EXE_vyges-drc")).args(["mask-check", &gds, "--rules", rules.to_str().unwrap()]).output().unwrap();
    assert_eq!(o.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&o.stderr).contains("line 1"));
    let _ = std::fs::remove_dir_all(dir);
}
