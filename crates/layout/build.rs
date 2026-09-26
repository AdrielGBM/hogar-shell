//! Lifts the doc comments off the layout model so `hogar-shell config schema layout` can annotate the file it prints.
//!
//! The same pipeline the config's own schema goes through, and deliberately the same scanner (`hogar-shell-doc-scanner`): one answer to "what does this key mean" for both files, rather than a second copy of a scanner that would drift from the first (F-10.8).
//!
//! `model.rs` alone, because that is the *written* form — what a layout file holds. `resolve.rs`'s answered form is what the shell decided, which no file ever says and no reference has a key for.

use std::path::Path;

fn main() {
    let sources = doc_scanner::rust_files(&[Path::new("src/model.rs")]);
    doc_scanner::emit(&sources, "LAYOUT", "layout_docs.rs");
}
