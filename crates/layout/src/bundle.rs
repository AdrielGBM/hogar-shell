//! Bundles: a layout with everything it needs to draw on another installation, as one plain directory.
//!
//! ```text
//! <bundle>/manifest.toml          name, and optionally description and author; no version (DEC-29)
//! <bundle>/layouts/<name>.toml    the layout, and each layout of its `extends` chain but the built-in one
//! <bundle>/components/<name>.toml every komponent those layouts draw
//! <bundle>/assets/<file>          every picture they name, which they name as `assets/<file>`
//! ```
//!
//! A directory rather than an archive: nothing in the workspace reads one, and a directory is what a person can read, diff and audit before importing it — which is the point of a trust prompt. A bundle carries what a layout file holds, its sources and actions with it (F-10.53); rules, `[automation]` limits and variables are the installation's and stay behind. Paths are the only thing rewritten on the way: out to `assets/<file>`, back in to the directory the importing shell keeps that bundle's pictures in.
//!
//! **A bundle is somebody else's directory**, so it is read as plain files only: a symbolic link anywhere in it, a FIFO, a socket or a device is refused rather than followed or opened, every file has a size it may not pass, every directory a number of entries, and only the pictures its layouts name are read at all. A link would let it carry whatever the importing user can read — a key, a password store — into a directory the next export shares; a FIFO would hold the shell waiting on its other end. Every directory of it is opened once and everything under it relative to that ([`util::beneath::PlainDir`]), so a directory swapped for a link after it was looked at redirects nothing.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use config::fingerprint::{LAYOUT_FILE_LIMIT, Links, read_regular};
use serde::{Deserialize, Serialize};
use util::beneath::{Entry, PlainDir};
use util::report::{Finding, Message, Report};
use util::text::{is_bidi_control, shown};

use crate::library::{Library, is_komponent_name, komponent_path};
use crate::model::*;
use crate::names::{unreadable_in_komponent, unreadable_in_layout};
use crate::ops::sites_mut;
use crate::resolve::{chain_of, layout_path};
use crate::store::BUILT_IN;
use crate::trust::{digest, komponent_chains, layout_chains};

pub const MANIFEST: &str = "manifest.toml";
pub const LAYOUTS: &str = "layouts";
pub const COMPONENTS: &str = "components";
pub const ASSETS: &str = "assets";

/// The most one picture may hold: an 8K wallpaper saved as PNG is a few tens of megabytes, so this carries any picture a screen can show, and keeps one file from filling the importing machine's disk.
pub const ASSET_LIMIT: u64 = 64 * 1024 * 1024;

/// The most every picture of one bundle may hold together: four of the largest pictures there are.
pub const ASSETS_LIMIT: u64 = 4 * ASSET_LIMIT;

/// The most entries `layouts/`, `components/` or the pictures a bundle names may each hold: far more than a shared layout has, and few enough that listing them costs nothing.
pub const ENTRIES_LIMIT: usize = 256;

/// What a bundle says about itself, in `manifest.toml`.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    /// What the bundle is called: what `layout trust` names it by and what its pictures are kept under once imported. Letters, digits, `-` and `_`.
    pub name: String,
    /// What it is, in a sentence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Who made it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
}

/// A bundle in memory: what it holds, ready to write out or to import.
#[derive(Clone, Debug, PartialEq)]
pub struct Bundle {
    pub manifest: Manifest,
    pub layouts: BTreeMap<LayoutId, Layout>,
    pub komponents: BTreeMap<KomponentId, Komponent>,
    /// Each picture it carries, by its file name under `assets/`, with where its bytes are read from: only the pictures its layouts name.
    pub assets: BTreeMap<String, Asset>,
    /// The text of each file it was read from, by the name a finding gives that file (`layouts/<name>.toml`), so what is wrong in it can be pointed at; empty for one made in memory.
    pub texts: BTreeMap<String, String>,
}

/// Where the bytes of one picture of a bundle are.
/// Where the bytes of one picture of a bundle are.
#[derive(Clone, Debug)]
pub enum Asset {
    /// A picture of the user's own, at this path: what an export carries, read when the bundle is written.
    At(PathBuf),
    /// The picture of this name in the `assets/` directory of a bundle that was read, opened as it was when it was read.
    In(Arc<PlainDir>, String),
}

impl Asset {
    /// The picture's bytes: a plain file of at most [`ASSET_LIMIT`], never read through a link.
    pub fn read(&self) -> Result<Vec<u8>, Message> {
        match self {
            Asset::At(path) => read_plain(path, ASSET_LIMIT),
            Asset::In(dir, name) => dir
                .read(name, ASSET_LIMIT)
                .map_err(|why| Message::verbatim(format!("{}: {why}", dir.join(name).display()))),
        }
    }
}

impl PartialEq for Asset {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Asset::At(one), Asset::At(other)) => one == other,
            (Asset::In(dir, name), Asset::In(other_dir, other_name)) => {
                dir.path() == other_dir.path() && name == other_name
            }
            _ => false,
        }
    }
}

/// `layout` and what it needs to draw elsewhere: the layouts its `extends` chain names but the built-in one, the komponents its groups draw, and every picture any of those layouts names, copied in and named as `assets/<file>`. A picture that is not a plain file — missing, a link, too large — a layout it extends or a komponent it draws that the store does not hold, and the built-in layout itself, are each a finding, and nothing is made.
pub fn export(id: &LayoutId, library: &Library) -> Result<Bundle, Report> {
    let mut report = Report::default();
    if id.as_str() == BUILT_IN {
        report.error(Finding::new(
            layout_path(id),
            "",
            util::message!("finding.bundle_built_in", id = id),
        ));
        return Err(report);
    }
    let Some(layout) = library.layout(id) else {
        report.error(Finding::new(
            layout_path(id),
            "",
            util::message!("finding.unknown_layout", id = id),
        ));
        return Err(report);
    };
    let mut layouts: BTreeMap<LayoutId, Layout> = chain_of(layout, library, &mut report)
        .into_iter()
        .filter(|level| level.id.as_str() != BUILT_IN)
        .map(|level| (level.id.clone(), level.clone()))
        .collect();
    if !is_komponent_name(id.as_str()) {
        report.error(Finding::new(
            layout_path(id),
            "",
            util::message!("finding.bundle_name", name = id),
        ));
    }
    for carried in layouts
        .keys()
        .filter(|it| *it != id && !is_komponent_name(it.as_str()))
    {
        report.error(Finding::new(
            layout_path(carried),
            "",
            util::message!("finding.bundle_file_name", name = carried),
        ));
    }
    let mut komponents = BTreeMap::new();
    for used in crate::components::komponents_of(layout, library) {
        if !is_komponent_name(used.as_str()) {
            report.error(Finding::new(
                komponent_path(&used),
                "",
                util::message!("finding.bundle_file_name", name = &used),
            ));
        }
        match library.komponent(&used) {
            Some(komponent) => {
                komponents.insert(used, komponent.clone());
            }
            None => report.error(Finding::new(
                komponent_path(&used),
                "",
                util::message!(
                    "finding.unknown_komponent",
                    komponent = &used,
                    file = komponent_path(&used)
                ),
            )),
        }
    }

    let mut assets: BTreeMap<String, Asset> = BTreeMap::new();
    let mut named: BTreeMap<PathBuf, String> = BTreeMap::new();
    for layout in layouts.values_mut() {
        let file = layout_path(&layout.id);
        for (key, path) in asset_paths_mut(layout) {
            if path.is_empty() {
                continue;
            }
            let on_disk = util::paths::expand_tilde(Path::new(path.as_str()));
            if let Some(why) = not_plain(&on_disk, ASSET_LIMIT) {
                report.error(Finding::new(&file, key, why));
                continue;
            }
            let name = match named.get(&on_disk) {
                Some(name) => name.clone(),
                None => {
                    let name = free_asset_name(&on_disk, &assets);
                    assets.insert(name.clone(), Asset::At(on_disk.clone()));
                    named.insert(on_disk, name.clone());
                    name
                }
            };
            *path = format!("{ASSETS}/{name}");
        }
    }
    if !report.errors.is_empty() {
        return Err(report);
    }
    Ok(Bundle {
        manifest: Manifest {
            name: id.to_string(),
            description: None,
            author: None,
        },
        layouts,
        komponents,
        assets,
        texts: BTreeMap::new(),
    })
}

/// Why the file at `path` cannot be carried in or out of a bundle — missing, a link, not a regular file, more than `limit` bytes — or `None` when it can. Asked without following a link, so a link is named as one rather than read through.
fn not_plain(path: &Path, limit: u64) -> Option<Message> {
    let shown_path = path.display().to_string();
    match std::fs::symlink_metadata(path) {
        Err(_) => Some(util::message!(
            "finding.bundle_asset_missing",
            path = &shown_path
        )),
        Ok(meta) if meta.file_type().is_symlink() => {
            Some(util::message!("finding.bundle_link", path = &shown_path))
        }
        Ok(meta) if !meta.is_file() => Some(util::message!(
            "finding.bundle_not_plain",
            path = &shown_path
        )),
        Ok(meta) if meta.len() > limit => Some(util::message!(
            "finding.bundle_too_large",
            path = &shown_path,
            limit = limit
        )),
        Ok(_) => None,
    }
}

/// Reads the plain file at `path` of a bundle, refusing what [`not_plain`] refuses, and refusing it again at the open in case the file was swapped since it was looked at: why not, said of `path`.
fn read_plain(path: &Path, limit: u64) -> Result<Vec<u8>, Message> {
    if let Some(why) = not_plain(path, limit) {
        return Err(why);
    }
    read_regular(path, limit, Links::Refuse).map_err(|why| Message::verbatim(why.to_string()))
}

/// A name under `assets/` for the picture at `path` that no other picture of the bundle has: its own file name, numbered where two pictures share one.
fn free_asset_name(path: &Path, taken: &BTreeMap<String, Asset>) -> String {
    let file = path
        .file_name()
        .and_then(|it| it.to_str())
        .filter(|it| is_asset_name(it))
        .unwrap_or("asset")
        .to_string();
    if !taken.contains_key(&file) {
        return file;
    }
    let (stem, extension) = match file.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => (stem, format!(".{extension}")),
        _ => (file.as_str(), String::new()),
    };
    (2..)
        .map(|nth| format!("{stem}-{nth}{extension}"))
        .find(|name| !taken.contains_key(name))
        .expect("the counting runs out long after the names do")
}

/// Whether `name` can be a picture's file name under `assets/`: one plain file name, no directory and nothing hidden.
pub fn is_asset_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_alphanumeric() || "-_.+@ ".contains(c))
}

impl Bundle {
    /// Writes the bundle out as the directory `dir`, which must not hold anything yet: a bundle is made whole, never merged into one that is there. Each picture is read as a plain file, never through a link.
    pub fn write(&self, dir: &Path) -> Result<(), Report> {
        let failed = |path: &Path, why: String| {
            let mut report = Report::default();
            report.error(Finding::new(path, "", Message::verbatim(why)));
            report
        };
        if dir.exists()
            && std::fs::read_dir(dir).map_or(true, |mut entries| entries.next().is_some())
        {
            let mut report = Report::default();
            report.error(Finding::new(
                dir,
                "",
                util::message!("finding.bundle_exists", path = dir.display()),
            ));
            return Err(report);
        }
        let mut files: Vec<(PathBuf, Vec<u8>)> = Vec::new();
        let manifest = toml::to_string_pretty(&self.manifest)
            .map_err(|why| failed(&dir.join(MANIFEST), why.to_string()))?;
        files.push((dir.join(MANIFEST), manifest.into_bytes()));
        for (id, layout) in &self.layouts {
            let path = dir.join(LAYOUTS).join(format!("{id}.toml"));
            let text =
                toml::to_string_pretty(layout).map_err(|why| failed(&path, why.to_string()))?;
            files.push((path, text.into_bytes()));
        }
        for (id, komponent) in &self.komponents {
            let path = dir.join(COMPONENTS).join(format!("{id}.toml"));
            let text =
                toml::to_string_pretty(komponent).map_err(|why| failed(&path, why.to_string()))?;
            files.push((path, text.into_bytes()));
        }
        for (name, from) in &self.assets {
            let bytes = from.read().map_err(|why| {
                let mut report = Report::default();
                report.error(Finding::new(dir.join(ASSETS).join(name), "", why));
                report
            })?;
            files.push((dir.join(ASSETS).join(name), bytes));
        }
        for (path, bytes) in files {
            util::fs::write_atomic(&path, &bytes).map_err(|why| failed(&path, why.to_string()))?;
        }
        Ok(())
    }

    /// Every file of the bundle by the name a finding gives it: `layouts/<name>.toml` and `components/<name>.toml`.
    pub fn files(&self) -> impl Iterator<Item = String> + '_ {
        self.layouts
            .keys()
            .map(layout_path)
            .chain(self.komponents.keys().map(komponent_path))
    }

    /// What makes this bundle the bundle it is, as one name the same in every build: its name and the names of the layouts and komponents it writes, which are what an import claims and what every answer about it is keyed by. Recorded at its first import, so a later bundle that borrows its name but writes other files is told apart from a reimport of the same one.
    ///
    /// Nothing else is part of it. What the files hold, the pictures, the description and the author are what an update changes — a changed command asks again for itself alone (DEC-30) — and nothing in a bundle proves who made it, so an author line would only ever refuse a corrected one.
    pub fn identity(&self) -> String {
        let files: Vec<String> = self.files().collect();
        let parts: Vec<&[u8]> = std::iter::once(self.manifest.name.as_bytes())
            .chain(files.iter().map(String::as_bytes))
            .collect();
        digest(&parts, 32)
    }

    /// The bundle with every picture its layouts name as `assets/<file>` named instead at `<dir>/<file>`: where an importing shell keeps it.
    pub fn installed_at(mut self, dir: &Path) -> Self {
        for layout in self.layouts.values_mut() {
            for (_, path) in asset_paths_mut(layout) {
                if let Some(name) = asset_named(path) {
                    *path = dir.join(name).display().to_string();
                }
            }
        }
        self
    }
}

/// The file under `assets/` that `path` names, for a path a bundle's layout writes.
fn asset_named(path: &str) -> Option<&str> {
    path.strip_prefix(ASSETS)?.strip_prefix('/')
}

/// Reads the bundle in the directory `dir`, saying where each thing wrong with it is: a manifest that is missing, does not parse or has a key it does not know (a `version` is one: DEC-29), anything in it that is not a plain file (a link, a FIFO, a device) or is larger than a file of its kind may be, a directory with too many entries, a file name that cannot name a layout or a komponent, a file that does not parse, an id, a name or a key holding a character a person cannot read as itself ([`crate::names`]), a command, an address or an action line holding a character that changes the direction text is shown in, and a picture its layouts name that it does not carry. Only the pictures its layouts name are looked at, and everything is read relative to a directory opened once, never through a link ([`PlainDir`]). What its layouts and komponents say is checked by whoever imports it, against what that shell has.
pub fn read(dir: &Path) -> Result<Bundle, Report> {
    let mut report = Report::default();
    let root = match PlainDir::open(dir) {
        Ok(root) => root,
        Err(why) => {
            report.error(Finding::new(
                dir,
                "",
                Message::verbatim(format!("{}: {why}", dir.display())),
            ));
            return Err(report);
        }
    };
    let manifest_path = root.join(MANIFEST);
    let manifest = match root.entry(MANIFEST) {
        Ok(Entry::Missing) => {
            report.error(Finding::new(
                &manifest_path,
                "",
                util::message!("finding.bundle_no_manifest", path = dir.display()),
            ));
            None
        }
        _ => match read_text(&root, MANIFEST) {
            Ok(text) => match toml::from_str::<Manifest>(&text) {
                Ok(manifest) => Some(manifest),
                Err(why) => {
                    report.error(Finding::new(
                        &manifest_path,
                        "",
                        Message::verbatim(why.to_string()),
                    ));
                    None
                }
            },
            Err(why) => {
                report.error(Finding::new(&manifest_path, "", why));
                None
            }
        },
    };
    if let Some(manifest) = &manifest
        && !is_komponent_name(&manifest.name)
    {
        report.error(Finding::new(
            &manifest_path,
            "name",
            util::message!("finding.bundle_name", name = &manifest.name),
        ));
    }

    let mut texts = BTreeMap::new();
    let mut layouts = BTreeMap::new();
    for (path, stem, text) in toml_files(&root, LAYOUTS, &mut report) {
        if stem == BUILT_IN {
            report.error(Finding::new(
                &path,
                "",
                util::message!("finding.bundle_built_in_name", id = &stem),
            ));
            continue;
        }
        match toml::from_str::<Layout>(&text) {
            Ok(mut layout) => {
                layout.id = LayoutId::new(&stem);
                texts.insert(layout_path(&layout.id), text);
                layouts.insert(layout.id.clone(), layout);
            }
            Err(why) => report.error(Finding::new(&path, "", Message::verbatim(why.to_string()))),
        }
    }
    let mut komponents = BTreeMap::new();
    for (path, stem, text) in toml_files(&root, COMPONENTS, &mut report) {
        match toml::from_str::<Komponent>(&text) {
            Ok(komponent) => {
                let id = KomponentId::new(&stem);
                texts.insert(komponent_path(&id), text);
                komponents.insert(id, komponent);
            }
            Err(why) => report.error(
                crate::validate::retired_in_komponent(&text, &path)
                    .unwrap_or_else(|| Finding::new(&path, "", Message::verbatim(why.to_string()))),
            ),
        }
    }
    if layouts.is_empty() && report.errors.is_empty() {
        report.error(Finding::new(
            dir.join(LAYOUTS),
            "",
            util::message!("finding.bundle_empty"),
        ));
    }
    for layout in layouts.values() {
        let file = dir.join(layout_path(&layout.id));
        unreadable_in_layout(layout, &file.display().to_string(), &mut report);
        refuse_bidi(&file, layout_texts(layout), &mut report);
    }
    for (id, komponent) in &komponents {
        let file = dir.join(komponent_path(id));
        unreadable_in_komponent(komponent, &file.display().to_string(), &mut report);
        let texts = komponent_chains(komponent)
            .into_iter()
            .flat_map(|(key, action)| {
                action
                    .0
                    .iter()
                    .map(move |line| (key.clone(), line.as_str()))
            })
            .collect();
        refuse_bidi(&file, texts, &mut report);
    }

    let assets = named_assets(&root, &mut layouts, &mut report);
    match (manifest, report.errors.is_empty()) {
        (Some(manifest), true) => Ok(Bundle {
            manifest,
            layouts,
            komponents,
            assets,
            texts,
        }),
        _ => Err(report),
    }
}

/// Why `name` in `dir` cannot be read as a file of at most `limit` bytes — missing, a link, a directory, a FIFO or a device, too large — said of where it is, or `None` where it can.
fn not_plain_in(dir: &PlainDir, name: &str, limit: u64) -> Option<Message> {
    let path = dir.join(name).display().to_string();
    match dir.entry(name) {
        Err(why) => Some(Message::verbatim(format!("{path}: {why}"))),
        Ok(Entry::Missing) => Some(util::message!("finding.bundle_asset_missing", path = &path)),
        Ok(Entry::Link) => Some(util::message!("finding.bundle_link", path = &path)),
        Ok(Entry::Directory | Entry::Other) => {
            Some(util::message!("finding.bundle_not_plain", path = &path))
        }
        Ok(Entry::File { len }) if len > limit => Some(util::message!(
            "finding.bundle_too_large",
            path = &path,
            limit = limit
        )),
        Ok(Entry::File { .. }) => None,
    }
}

/// The manifest, a layout or a komponent `name` in `dir`, as text: a plain file no larger than [`LAYOUT_FILE_LIMIT`], refused at the open if it is a link.
fn read_text(dir: &PlainDir, name: &str) -> Result<String, Message> {
    if let Some(why) = not_plain_in(dir, name, LAYOUT_FILE_LIMIT) {
        return Err(why);
    }
    let path = dir.join(name);
    let bytes = dir
        .read(name, LAYOUT_FILE_LIMIT)
        .map_err(|why| Message::verbatim(format!("{}: {why}", path.display())))?;
    String::from_utf8(bytes)
        .map_err(|_| util::message!("finding.bundle_not_text", path = path.display()))
}

/// The directory `name` in `root`: `None` where there is none, and a finding where it is a link or anything else that is not a directory.
fn subdir(root: &PlainDir, name: &str, report: &mut Report) -> Option<PlainDir> {
    let path = root.join(name);
    let refused = match root.entry(name) {
        Ok(Entry::Missing) => return None,
        Ok(Entry::Directory) => match root.dir(name) {
            Ok(dir) => return Some(dir),
            Err(why) => Message::verbatim(format!("{}: {why}", path.display())),
        },
        Ok(Entry::Link) => util::message!("finding.bundle_link", path = path.display()),
        Ok(Entry::File { .. } | Entry::Other) => {
            util::message!("finding.bundle_not_plain", path = path.display())
        }
        Err(why) => Message::verbatim(format!("{}: {why}", path.display())),
    };
    report.error(Finding::new(&path, "", refused));
    None
}

/// Every picture `layouts` name, each a plain file directly under `assets/` no larger than [`ASSET_LIMIT`], all of them together no larger than [`ASSETS_LIMIT`]; a finding where a layout names one the bundle does not carry as such. Nothing else under `assets/` is looked at.
fn named_assets(
    root: &PlainDir,
    layouts: &mut BTreeMap<LayoutId, Layout>,
    report: &mut Report,
) -> BTreeMap<String, Asset> {
    let assets_dir = subdir(root, ASSETS, report).map(Arc::new);
    let at = root.join(ASSETS);
    let mut assets = BTreeMap::new();
    let mut total: u64 = 0;
    for layout in layouts.values_mut() {
        let file = root.join(layout_path(&layout.id));
        for (key, path) in asset_paths_mut(layout) {
            if path.is_empty() {
                continue;
            }
            let named = asset_named(path).filter(|name| is_asset_name(name));
            let (Some(name), Some(dir)) = (named, &assets_dir) else {
                report.error(Finding::new(
                    &file,
                    key,
                    util::message!("finding.bundle_asset_path", path = path.as_str()),
                ));
                continue;
            };
            if assets.contains_key(name) {
                continue;
            }
            match dir.entry(name) {
                Ok(Entry::File { len }) if len <= ASSET_LIMIT => {
                    total += len;
                    assets.insert(
                        name.to_string(),
                        Asset::In(Arc::clone(dir), name.to_string()),
                    );
                }
                Ok(Entry::Missing) => report.error(Finding::new(
                    &file,
                    key,
                    util::message!("finding.bundle_asset_path", path = path.as_str()),
                )),
                _ => {
                    let why = not_plain_in(dir, name, ASSET_LIMIT).unwrap_or_else(|| {
                        util::message!("finding.bundle_not_plain", path = path.as_str())
                    });
                    report.error(Finding::new(&file, key, why));
                }
            }
        }
    }
    if assets.len() > ENTRIES_LIMIT {
        report.error(Finding::new(
            &at,
            "",
            util::message!(
                "finding.bundle_too_many",
                path = at.display(),
                limit = ENTRIES_LIMIT
            ),
        ));
    }
    if total > ASSETS_LIMIT {
        report.error(Finding::new(
            &at,
            "",
            util::message!(
                "finding.bundle_too_large",
                path = at.display(),
                limit = ASSETS_LIMIT
            ),
        ));
    }
    assets
}

/// Every command, address and action line `layout` writes, with the key it is written at.
fn layout_texts(layout: &Layout) -> Vec<(String, &str)> {
    let mut texts: Vec<(String, &str)> = layout
        .sources
        .iter()
        .flat_map(|(name, source)| {
            [
                source.cmd().map(|cmd| (format!("sources.{name}.cmd"), cmd)),
                source.url().map(|url| (format!("sources.{name}.url"), url)),
            ]
            .into_iter()
            .flatten()
        })
        .collect();
    for (key, action) in layout_chains(layout) {
        texts.extend(action.0.iter().map(|line| (key.clone(), line.as_str())));
    }
    texts
}

/// A finding at each of `texts` that holds a character overriding or isolating the direction text is shown in: a command or an address has no use for one, and with one it reads, in the trust prompt as anywhere, as something other than what runs.
fn refuse_bidi(file: &Path, texts: Vec<(String, &str)>, report: &mut Report) {
    for (key, text) in texts {
        if let Some(c) = text.chars().find(|c| is_bidi_control(*c)) {
            report.error(Finding::new(
                file,
                key,
                util::message!(
                    "finding.bundle_bidi",
                    text = shown(text),
                    code = format!("{:04X}", u32::from(c))
                ),
            ));
        }
    }
}

/// Every `.toml` file in the directory `name` of `root` with its path, stem and text: plain files only, never through a link, each no larger than a layout file may be, no more than [`ENTRIES_LIMIT`] of them. A directory that is a link, an entry that is not a plain file or whose stem cannot name a file of the shell's, and one that cannot be read, are reported instead.
fn toml_files(root: &PlainDir, name: &str, report: &mut Report) -> Vec<(PathBuf, String, String)> {
    let Some(dir) = subdir(root, name, report) else {
        return Vec::new();
    };
    let listed = match dir.names(ENTRIES_LIMIT) {
        Ok(Some(names)) => names,
        Ok(None) => {
            report.error(Finding::new(
                dir.path(),
                "",
                util::message!(
                    "finding.bundle_too_many",
                    path = dir.path().display(),
                    limit = ENTRIES_LIMIT
                ),
            ));
            return Vec::new();
        }
        Err(why) => {
            report.error(Finding::new(
                dir.path(),
                "",
                Message::verbatim(format!("{}: {why}", dir.path().display())),
            ));
            return Vec::new();
        }
    };
    let mut names: Vec<OsString> = listed
        .into_iter()
        .filter(|name| Path::new(name).extension().and_then(|it| it.to_str()) == Some("toml"))
        .collect();
    names.sort();
    let mut found = Vec::new();
    for file in names {
        let path = dir.join(&file);
        let stem = Path::new(&file)
            .file_stem()
            .and_then(|it| it.to_str())
            .unwrap_or_default()
            .to_string();
        if !is_komponent_name(&stem) {
            report.error(Finding::new(
                &path,
                "",
                util::message!("finding.bundle_file_name", name = &stem),
            ));
            continue;
        }
        match read_text(&dir, &format!("{stem}.toml")) {
            Ok(text) => found.push((path, stem, text)),
            Err(why) => report.error(Finding::new(&path, "", why)),
        }
    }
    found
}

/// Every key of `layout` that holds a path to a picture, with the path: a wallpaper region's `source` and a texture's `image`, at every level of the file.
pub fn asset_paths_mut(layout: &mut Layout) -> Vec<(String, &mut String)> {
    let mut paths = Vec::new();
    for (site, layer) in sites_mut(layout) {
        for area in &mut layer.areas {
            let key = format!("{site}.areas.{}", area.id);
            match &mut area.kind {
                Some(AreaKind::WallpaperRegion {
                    source: Some(path), ..
                }) => paths.push((format!("{key}.source"), path)),
                Some(AreaKind::Texture {
                    image: Some(path), ..
                }) => paths.push((format!("{key}.image"), path)),
                _ => {}
            }
        }
    }
    paths
}

/// What a layout or a komponent holds, as one name that is the same in every build: a SHA-256 of what the file written from it holds, which is how a reimport tells a file the bundle wrote and nobody touched from one somebody changed.
pub fn fingerprint<T: Serialize>(written: &T) -> Result<String, toml::ser::Error> {
    let text = toml::to_string_pretty(written)?;
    Ok(digest(&[text.as_bytes()], 32))
}
