//! Lifts the doc comments off this workspace's own source into the tables a generated schema annotates from.
//!
//! Two schemas are generated from doc comments — `config.toml`'s and a layout file's — and both are generated for the same reason: a hand-written reference is a second copy of the truth, and the copy is always the one that goes stale. One scanner between them for the same reason again. It lived in `crates/config/build.rs` and read that crate's `src/` alone, which made every doc comment on a layout type invisible to it *silently*, since an item it does not recognise is simply not annotated (F-10.8).
//!
//! **A scanner rather than a parser.** The input is this workspace's own source in a known shape — a `///` run, then `pub struct X {`, `pub enum X {`, a struct variant or ` pub field:` — so matching that shape costs a few dozen lines instead of a `syn` dependency in every build script.
//!
//! **Nothing here can fail a build.** Anything it does not recognise is not annotated, and a missed comment costs one bare key in a printed schema. That is the right trade for documentation, and it is also why the pipeline needs a test at the far end: its failure mode is silence, so only a test that adds a comment and looks for it can tell working from quiet (T-9.2).

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// Every Rust file in `paths`, with a directory expanded into the files it holds, sorted so the generated table's order is stable — it is the order a schema prints in.
///
/// Each path is also registered with Cargo, so editing a documented type regenerates the table. A build script that scans a *sibling* crate must register that crate's files too, which is the one thing a caller cannot forget here because it is not a separate step.
pub fn rust_files(paths: &[&Path]) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for path in paths {
        println!("cargo:rerun-if-changed={}", path.display());
        if path.is_dir() {
            let Ok(entries) = std::fs::read_dir(path) else {
                continue;
            };
            let mut files: Vec<PathBuf> = entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| path.extension().is_some_and(|it| it == "rs"))
                .collect();
            // Directory order is not stable, and the generated table's order is what the schema prints in.
            files.sort();
            found.extend(files);
        } else {
            found.push(path.to_path_buf());
        }
    }
    for file in &found {
        println!("cargo:rerun-if-changed={}", file.display());
    }
    found
}

/// Scans `sources` and writes `<OUT_DIR>/<file>` with the tables named after `prefix`.
///
/// The statics are `<PREFIX>_DOCS`, `<PREFIX>_FIELDS`, `<PREFIX>_FIELD_TYPES`, `<PREFIX>_FIELD_RUST` and `<PREFIX>_VARIANTS`, which is what lets one crate `include!` the config's tables and another the layout's without either knowing the other exists.
pub fn emit(sources: &[PathBuf], prefix: &str, file: &str) {
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
    std::fs::write(out.join(file), scan(sources).render(prefix))
        .unwrap_or_else(|why| panic!("writing {file}: {why}"));
}

/// What one pass over the source found.
#[derive(Default)]
pub struct Tables {
    /// `(item, field, doc)`; an empty field is the item's own comment.
    docs: Vec<(String, String, String)>,
    /// `(item, field)` for every field, in declaration order.
    fields: Vec<(String, String)>,
    /// `(item, field, type)` for every field whose type is a plain named one.
    types: Vec<(String, String, String)>,
    /// `(item, field, type)` for every field, with the type exactly as declared.
    rust: Vec<(String, String, String)>,
    /// `(enum, variant)` for every unit variant, spelled the way serde writes it in a file.
    variants: Vec<(String, String)>,
}

/// Lifts the doc comments off `sources`.
///
/// Items are named as the source names them, except that a struct variant of an enum is `Enum::Variant` — which is what a flattened variant's keys have to be looked up by, since in the file they sit beside the keys of the table that flattened them.
pub fn scan(sources: &[PathBuf]) -> Tables {
    let mut text = String::new();
    for source in sources {
        text.push_str(&std::fs::read_to_string(source).unwrap_or_default());
        // The scanner carries an item across lines, so a file must not continue the one before it.
        text.push_str("\n\n");
    }

    let mut tables = Tables::default();
    let mut pending: Vec<String> = Vec::new();
    let mut item = String::new();
    let mut inside_enum: Option<String> = None;
    // What `#[serde(rename)]` calls the next field, if it says.
    let mut rename: Option<String> = None;
    let mut rename_all: Option<String> = None;
    let mut variant_case: Option<String> = None;

    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(doc) = trimmed
            .strip_prefix("/// ")
            .or_else(|| (trimmed == "///").then_some(""))
        {
            pending.push(doc.to_string());
            continue;
        }
        // An attribute sits between the doc comment and the item it documents, so it must not clear the run — and one of them says what the field is called in the file, which is the name a schema looks a key up by.
        if trimmed.starts_with("#[") {
            if let Some(name) = renamed(trimmed) {
                rename = Some(name);
            }
            if let Some(case) = renamed_all(trimmed) {
                rename_all = Some(case);
            }
            continue;
        }
        // A closing brace at the left margin ends the item it closes, which is what keeps a field of the next thing from being recorded against this one.
        if line == "}" {
            item.clear();
            inside_enum = None;
            variant_case = None;
            rename_all = None;
            pending.clear();
            continue;
        }

        if let Some(name) = struct_name(trimmed) {
            inside_enum = None;
            item = name;
            tables.push_doc(&item, "", &pending);
        } else if let Some(name) = enum_name(trimmed) {
            inside_enum = Some(name.clone());
            variant_case = rename_all.take();
            item = name;
            tables.push_doc(&item, "", &pending);
        } else if let Some((owner, variant)) = inside_enum
            .as_deref()
            .and_then(|owner| unit_variant(trimmed).map(|variant| (owner.to_string(), variant)))
        {
            let spelled = rename
                .take()
                .unwrap_or_else(|| spelled(&variant, variant_case.as_deref()));
            tables.variants.push((owner, spelled));
        } else if let Some(variant) = inside_enum
            .as_deref()
            .and_then(|owner| variant_name(trimmed).map(|variant| format!("{owner}::{variant}")))
        {
            item = variant;
            tables.push_doc(&item, "", &pending);
        } else if let Some((variant, fields)) = inside_enum.as_deref().and_then(|owner| {
            on_one_line(trimmed).map(|(name, fields)| (format!("{owner}::{name}"), fields))
        }) {
            // A variant short enough to fit on one line — `Zone { zone: Zone }` — is a variant whose keys a file still writes, and rustfmt will keep putting it back on one line however it is written out here.
            tables.push_doc(&variant, "", &pending);
            for field in fields.split(',') {
                if let Some(name) = field_name(field.trim(), true) {
                    tables.fields.push((variant.clone(), name.clone()));
                    if let Some(declared) = declared_type(field) {
                        tables.rust.push((variant.clone(), name.clone(), declared));
                    }
                    if let Some(kind) = field_type(field) {
                        tables.types.push((variant.clone(), name, kind));
                    }
                }
            }
            item = variant;
        } else if let Some(field) = field_name(trimmed, inside_enum.is_some())
            && !item.is_empty()
        {
            let field = rename.take().unwrap_or(field);
            tables.push_doc(&item, &field, &pending);
            tables.fields.push((item.clone(), field.clone()));
            if let Some(declared) = declared_type(trimmed) {
                tables.rust.push((item.clone(), field.clone(), declared));
            }
            if let Some(kind) = field_type(trimmed) {
                tables.types.push((item.clone(), field, kind));
            }
        }
        pending.clear();
        rename = None;
        rename_all = None;
    }
    tables
}

impl Tables {
    fn push_doc(&mut self, item: &str, field: &str, docs: &[String]) {
        if docs.is_empty() {
            return;
        }
        self.docs
            .push((item.to_string(), field.to_string(), docs.join("\n")));
    }

    /// The tables as Rust, for a build script to write into `OUT_DIR`.
    pub fn render(&self, prefix: &str) -> String {
        let mut docs = String::new();
        for (item, field, doc) in &self.docs {
            let _ = writeln!(docs, "    ({item:?}, {field:?}, {doc:?}),");
        }
        let mut fields = String::new();
        for (item, field) in &self.fields {
            let _ = writeln!(fields, "    ({item:?}, {field:?}),");
        }
        let mut types = String::new();
        for (item, field, kind) in &self.types {
            let _ = writeln!(types, "    ({item:?}, {field:?}, {kind:?}),");
        }
        let mut rust = String::new();
        for (item, field, declared) in &self.rust {
            let _ = writeln!(rust, "    ({item:?}, {field:?}, {declared:?}),");
        }
        let mut variants = String::new();
        for (owner, variant) in &self.variants {
            let _ = writeln!(variants, "    ({owner:?}, {variant:?}),");
        }
        format!(
            "/// `(item, field, doc)`; an empty field is the item's own comment. A struct variant is `Enum::Variant`.\n\
             /// Generated by `build.rs` through `hogar-shell-doc-scanner`.\n\
             pub static {prefix}_DOCS: &[(&str, &str, &str)] = &[\n{docs}];\n\n\
             /// `(item, field)` for every field, in declaration order. What lets a schema list a key serde\n\
             /// left out of the defaults — an `Option` that is `None` serializes to nothing at all, so without\n\
             /// this the reference would silently omit every optional key that carries no doc comment.\n\
             pub static {prefix}_FIELDS: &[(&str, &str)] = &[\n{fields}];\n\n\
             /// `(item, field, type)` for every field, documented or not. What lets a schema annotate a\n\
             /// nested table from the struct that actually backs it.\n\
             pub static {prefix}_FIELD_TYPES: &[(&str, &str, &str)] = &[\n{types}];\n\n\
             /// `(item, field, type)` for every field, with its Rust type as declared: what a control type is\n\
             /// read off, so a `bool` is a switch and an `Option<f32>` a number that may be left unset.\n\
             pub static {prefix}_FIELD_RUST: &[(&str, &str, &str)] = &[\n{rust}];\n\n\
             /// `(enum, variant)` for every unit variant, spelled as serde writes it in a file.\n\
             pub static {prefix}_VARIANTS: &[(&str, &str)] = &[\n{variants}];\n"
        )
    }
}

/// `    pub scale: ScaleConfig,` → `ScaleConfig`. Only the bare name is wanted, so a wrapped type (`HashMap<String, PathBuf>`) yields nothing rather than a guess about which parameter matters — a nested table is only ever a plain struct here.
///
/// `Vec<T>` and `Option<T>` unwrap, because both describe a table by their element: what an `[[idle.stages]]` table means is what an `IdleStage` is, and an `Option<AutoHide>` written out is an `AutoHide`.
fn field_type(line: &str) -> Option<String> {
    let (_, rest) = line.split_once(':')?;
    let mut kind = rest.trim().trim_end_matches(',').trim();
    for wrapper in ["Vec<", "Option<"] {
        if let Some(inner) = kind
            .strip_prefix(wrapper)
            .and_then(|it| it.strip_suffix('>'))
        {
            kind = inner.trim();
        }
    }
    let plain = !kind.is_empty()
        && kind.chars().all(|c| c.is_ascii_alphanumeric())
        && kind.starts_with(|c: char| c.is_ascii_uppercase());
    plain.then(|| kind.to_string())
}

/// `    pub size: Option<u32>,` → `Option<u32>`: the type as written, for whoever needs more than the struct a field names.
fn declared_type(line: &str) -> Option<String> {
    let (_, rest) = line.split_once(':')?;
    let declared = rest.trim().trim_end_matches(',').trim();
    (!declared.is_empty()).then(|| declared.to_string())
}

/// `    Chips,` → `Chips`: a variant that carries nothing, which a file writes as a bare string.
fn unit_variant(line: &str) -> Option<String> {
    let name = line.trim_end_matches(',').split('=').next()?.trim();
    let named = !name.is_empty()
        && name.starts_with(|c: char| c.is_ascii_uppercase())
        && name.chars().all(|c| c.is_ascii_alphanumeric());
    named.then(|| name.to_string())
}

/// `#[serde(rename_all = "snake_case")]` → `snake_case`: how every variant of the enum below it is spelled.
fn renamed_all(line: &str) -> Option<String> {
    let (_, rest) = line.split_once("rename_all = \"")?;
    let (case, _) = rest.split_once('"')?;
    (!case.is_empty()).then(|| case.to_string())
}

/// A variant's name as serde spells it under a `rename_all` rule.
fn spelled(variant: &str, case: Option<&str>) -> String {
    let separated = |separator: char| {
        let mut out = String::new();
        for (index, c) in variant.chars().enumerate() {
            if c.is_ascii_uppercase() && index > 0 {
                out.push(separator);
            }
            out.push(c.to_ascii_lowercase());
        }
        out
    };
    match case {
        Some("lowercase") => variant.to_lowercase(),
        Some("UPPERCASE") => variant.to_uppercase(),
        Some("snake_case") => separated('_'),
        Some("kebab-case") => separated('-'),
        Some("SCREAMING_SNAKE_CASE") => separated('_').to_uppercase(),
        Some("SCREAMING-KEBAB-CASE") => separated('-').to_uppercase(),
        Some("camelCase") => {
            let mut chars = variant.chars();
            chars
                .next()
                .map(|first| first.to_ascii_lowercase().to_string() + chars.as_str())
                .unwrap_or_default()
        }
        _ => variant.to_string(),
    }
}

/// `pub struct Name {` → `Name`. Tuple and unit structs carry no fields worth documenting, so they are skipped.
fn struct_name(line: &str) -> Option<String> {
    named_item(line, "pub struct ")
}

/// `pub enum Name {` → `Name`.
fn enum_name(line: &str) -> Option<String> {
    named_item(line, "pub enum ")
}

fn named_item(line: &str, keyword: &str) -> Option<String> {
    let rest = line.strip_prefix(keyword)?;
    let name = rest.split([' ', '{', '<', '(']).next()?.trim();
    line.ends_with('{').then(|| name.to_string())
}

/// `    Bar {` → `Bar`: a struct variant, whose fields end up flattened beside the keys of whatever holds it. A unit variant (`Volume,`) and a tuple variant (`Solid(Color)`) have no keys, so neither matches.
fn variant_name(line: &str) -> Option<String> {
    let name = line.strip_suffix('{')?.trim();
    let named = !name.is_empty()
        && name.starts_with(|c: char| c.is_ascii_uppercase())
        && name.chars().all(|c| c.is_ascii_alphanumeric());
    named.then(|| name.to_string())
}

/// `#[serde(rename = "match")]` → `match`: what the file calls the field below it, which is the name a schema has to look a key up by. A rename of the whole item (`rename_all`) is not one of these — it changes how a *value* is spelled, and no key is named after it.
fn renamed(line: &str) -> Option<String> {
    let (_, rest) = line.split_once("rename = \"")?;
    let (name, _) = rest.split_once('"')?;
    (!name.is_empty()).then(|| name.to_string())
}

/// `    Zone { zone: Zone },` → `("Zone", "zone: Zone")`: a struct variant short enough that rustfmt keeps it on one line, whose keys a file writes all the same.
fn on_one_line(line: &str) -> Option<(String, String)> {
    let (name, rest) = line.trim_end_matches(',').split_once('{')?;
    let name = name.trim();
    let named = !name.is_empty()
        && name.starts_with(|c: char| c.is_ascii_uppercase())
        && name.chars().all(|c| c.is_ascii_alphanumeric());
    let fields = rest.trim().strip_suffix('}')?.trim().to_string();
    (named && !fields.is_empty()).then_some((name.to_string(), fields))
}

/// `    pub field: Type,` → `field`. A struct's fields must be `pub`, since a private one is not config; a variant's fields are public by being the variant's, so inside an enum the keyword is not asked for.
fn field_name(line: &str, in_enum: bool) -> Option<String> {
    let rest = match line.strip_prefix("pub ") {
        Some(rest) => rest,
        None if in_enum => line,
        None => return None,
    };
    let name = rest.split(':').next()?.trim();
    let named = !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    (named && rest.contains(':')).then(|| name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scanned(name: &str, source: &str) -> Tables {
        // One directory per test: these run on threads of one process, and a shared path would have each reading whatever the last one wrote.
        let dir = std::env::temp_dir().join(format!("doc-scanner-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        let file = dir.join("source.rs");
        std::fs::write(&file, source).expect("a source to scan");
        let tables = scan(&[file]);
        std::fs::remove_dir_all(&dir).ok();
        tables
    }

    fn doc_of(tables: &Tables, item: &str, field: &str) -> Option<String> {
        tables
            .docs
            .iter()
            .find(|(i, f, _)| i == item && f == field)
            .map(|(_, _, doc)| doc.clone())
    }

    #[test]
    fn a_struct_its_own_comment_and_its_fields_are_all_found() {
        let tables = scanned(
            "struct",
            "/// What a thing is.\n\
             #[derive(Debug)]\n\
             pub struct Thing {\n\
             \x20   /// How big.\n\
             \x20   pub size: f32,\n\
             \x20   pub nested: Inner,\n\
             \x20   fn_private: bool,\n\
             }\n",
        );
        assert_eq!(
            doc_of(&tables, "Thing", "").as_deref(),
            Some("What a thing is.")
        );
        assert_eq!(
            doc_of(&tables, "Thing", "size").as_deref(),
            Some("How big.")
        );
        assert_eq!(
            tables.fields,
            [
                ("Thing".to_string(), "size".to_string()),
                ("Thing".to_string(), "nested".to_string())
            ],
            "a private field is not config"
        );
        assert_eq!(
            tables.types,
            [(
                "Thing".to_string(),
                "nested".to_string(),
                "Inner".to_string()
            )],
            "and only a plain named type names a nested table"
        );
    }

    /// The reason this crate exists at all in its second form: a layout file writes an area's geometry *flattened*, so `thickness` sits beside `kind` in the same table and its comment is on a variant of an enum.
    #[test]
    fn a_struct_variant_s_fields_are_found_under_the_variant() {
        let tables = scanned(
            "variant",
            "/// What kind of region.\n\
             pub enum AreaKind {\n\
             \x20   /// A strip along one edge.\n\
             \x20   Bar {\n\
             \x20       /// How thick.\n\
             \x20       thickness: Option<f32>,\n\
             \x20       shape: BarShape,\n\
             \x20   },\n\
             \x20   /// A rectangle placed by hand.\n\
             \x20   Free { rect: Option<Rect> },\n\
             }\n\
             \n\
             pub struct After {\n\
             \x20   pub after: bool,\n\
             }\n",
        );
        assert_eq!(
            doc_of(&tables, "AreaKind::Bar", "").as_deref(),
            Some("A strip along one edge.")
        );
        assert_eq!(
            doc_of(&tables, "AreaKind::Bar", "thickness").as_deref(),
            Some("How thick.")
        );
        assert_eq!(
            tables.types,
            [
                (
                    "AreaKind::Bar".to_string(),
                    "shape".to_string(),
                    "BarShape".to_string()
                ),
                (
                    "AreaKind::Free".to_string(),
                    "rect".to_string(),
                    "Rect".to_string()
                )
            ],
            "an `Option` unwraps to the struct behind it, and a variant short enough for one line still has its keys read"
        );
        assert!(
            tables
                .fields
                .contains(&("After".to_string(), "after".to_string())),
            "and the item after the enum is its own: {:?}",
            tables.fields
        );
    }

    /// The tables are printed as Rust and `include!`d, so what they hold has to survive being written out — a doc comment with a quote or a backslash in it included.
    #[test]
    fn the_printed_tables_are_named_after_their_prefix_and_quote_what_they_hold() {
        let tables = scanned(
            "printed",
            "/// A \"quoted\" word and a \\\\ backslash.\n\
             pub struct Thing {\n\
             \x20   pub size: f32,\n\
             }\n",
        );
        let rendered = tables.render("LAYOUT");
        assert!(rendered.contains("pub static LAYOUT_DOCS"), "{rendered}");
        assert!(rendered.contains("pub static LAYOUT_FIELDS"), "{rendered}");
        assert!(
            rendered.contains("pub static LAYOUT_FIELD_TYPES"),
            "{rendered}"
        );
        assert!(
            rendered.contains(r#"\"quoted\""#),
            "a quote has to come back out escaped: {rendered}"
        );
    }

    #[test]
    fn a_field_keeps_the_type_it_was_declared_with() {
        let tables = scanned(
            "declared",
            "pub struct Thing {\n\
             \x20   pub size: Option<u32>,\n\
             \x20   pub names: HashMap<String, String>,\n\
             \x20   pub on: bool,\n\
             }\n",
        );
        assert_eq!(
            tables.rust,
            [
                ("Thing".into(), "size".into(), "Option<u32>".into()),
                (
                    "Thing".into(),
                    "names".into(),
                    "HashMap<String, String>".into()
                ),
                ("Thing".into(), "on".into(), "bool".into()),
            ]
        );
    }

    /// What an enum field accepts is what serde spells its variants as, so the scanner applies the enum's own `rename_all` rather than guessing.
    #[test]
    fn a_unit_variant_is_spelled_the_way_serde_writes_it() {
        let tables = scanned(
            "variants",
            "#[derive(Deserialize)]\n\
             #[serde(rename_all = \"snake_case\")]\n\
             pub enum Mode {\n\
             \x20   #[default]\n\
             \x20   OneBar,\n\
             \x20   Sections,\n\
             \x20   #[serde(rename = \"pill\")]\n\
             \x20   Chips,\n\
             }\n\
             \n\
             pub enum Plain {\n\
             \x20   Kept,\n\
             \x20   Carrying(u32),\n\
             \x20   Shaped { at: f32 },\n\
             }\n",
        );
        assert_eq!(
            tables.variants,
            [
                ("Mode".into(), "one_bar".into()),
                ("Mode".into(), "sections".into()),
                ("Mode".into(), "pill".into()),
                ("Plain".into(), "Kept".into()),
            ],
            "only variants a file writes as a bare string, each spelled by its own enum's rule"
        );
    }
}
