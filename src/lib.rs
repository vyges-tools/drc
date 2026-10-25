//! vyges-drc — geometric **design-rule check**.
//!
//! A routed/laid-out layout (**GDSII** or **OASIS**) + a per-layer **rule deck**
//! (`.drc`) in, a list of **violations** out. The physical-verification sibling of
//! `vyges-lvs`: where LVS asks "does the layout implement the schematic?", DRC asks
//! "does the layout obey the foundry's geometry rules?". Both ride the same clean-room
//! **vyges-layout** GDS/OASIS / boolean / flatten kernel — one toolset, one language.
//!
//! Covers fourteen rule classes — minimum **width** and **spacing**, minimum **area**,
//! windowed metal **density**, per-net **antenna** ratio (union-find connectivity from
//! `connect` rules), **enclosure** / **venc** (via enclosure), **span** and **corner**
//! (via matches the metal), **sep** / **c2c** / **runlen** (directional edge spacing),
//! and **grid** / **track** (manufacturing / routing grid) — plus a **metal-fill
//! generator** (`fill_library`). Width/spacing and the enclosure family measure against
//! the **merged** (unioned) layer geometry, and every rule targets a `(layer, datatype)`
//! pair so shapes sharing a GDS layer number but differing in datatype stay distinct.
//!
//! Boundary: files in / report out (GDSII or OASIS + `.drc` in; text or `--json`
//! violations out, with a CI exit code). Pure std beyond the geometry kernel.

// The crate's own name inside it: the command-line entry ([`cli_main`]) is built into the
// library too, and its `vyges_drc::…` paths resolve here.
extern crate self as vyges_drc;

pub use vyges_layout as layout;

pub mod cluster;
pub mod drc;
pub mod rules;
pub mod views;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const COPYRIGHT: &str = "© 2026 Vyges. All Rights Reserved.  https://vyges.com";

/// The `vyges-drc` command-line entry (the binary's `main`), built into the library too so a
/// suite binary can link every engine and run the one it is invoked as.
#[cfg(feature = "suite")]
#[doc(hidden)]
#[path = "main.rs"]
pub mod cli_main;
