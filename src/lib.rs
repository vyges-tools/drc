//! vyges-drc — geometric **design-rule check**.
//!
//! A routed/laid-out **GDS** + a per-layer **rule deck** (`.drc`) in, a list of
//! **violations** out. The physical-verification sibling of `vyges-lvs`: where LVS
//! asks "does the layout implement the schematic?", DRC asks "does the layout obey
//! the foundry's geometry rules?". Both ride the same clean-room **vyges-layout**
//! GDS / boolean / flatten kernel — one toolset, one language.
//!
//! v0 covers four rule classes — minimum **width**, minimum **spacing**, minimum
//! **area**, and windowed metal **density** — on flattened Manhattan geometry.
//! Enclosure / antenna and same-layer pre-merge are the depth passes (see `drc.rs`).
//!
//! Boundary: files in / report out (GDS + `.drc` in; text or `--json` violations
//! out, with a CI exit code). Pure std beyond the geometry kernel.

pub use vyges_layout as layout;

pub mod drc;
pub mod rules;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const COPYRIGHT: &str = "© 2026 Vyges. All Rights Reserved.  https://vyges.com";
