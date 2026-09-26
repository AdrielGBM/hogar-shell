//! Lifts the doc comments off the config types so `hogar-shell config schema` can annotate the defaults it prints.
//!
//! Generated rather than hand-maintained because a hand-written reference is a second copy of the truth, and the copy is always the one that goes stale. The scanner itself is `hogar-shell-doc-scanner`, shared with the layout crate's own build script: it used to live here and read this crate alone, which made every doc comment that moved into `crates/layout` invisible to it without a word (F-10.8).
//!
//! The sources are `Config` itself and one file per area under `sections/`, listed by reading the directory rather than by naming the files, so a new area is documented without touching this.

use std::path::Path;

fn main() {
    let sources = doc_scanner::rust_files(&[Path::new("src/config.rs"), Path::new("src/sections")]);
    doc_scanner::emit(&sources, "CONFIG", "config_docs.rs");
}
