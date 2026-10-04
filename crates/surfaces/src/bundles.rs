//! Bundles in the running shell: importing one into the layout store, and the trust what each imported file runs is held under (DEC-30).
//!
//! **Provenance is machine state.** `state.json` remembers, per bundle, what made it that bundle ([`layout::bundle::Bundle::identity`]), each file its import wrote and a hash of what it wrote there, and every answer the user gave about what those files run, bound to the exact text it was given for ([`services::state::BundleRecord`]). A file no bundle wrote is the user's own and nothing in it is ever held; a file a bundle wrote stays that bundle's however it is edited, so a command edited into it, or changed in it, asks again for that item alone. Answers belong to the bundle that was answered, and a file that went away takes its bundle's answers about it along the next time an import writes that path, so the file written there, by another bundle or by the same one imported again, starts with none.
//!
//! **Nothing runs before an answer, and an answer is for what was shown.** An import leaves every item pending, and the store's [`layout::Trust`] is what resolution and the source declaration read: a held action line is taken out of its chain, a held source is never declared, so no producer ever starts for it (F-10.55). [`accept`] and [`decline`] take the very items the user was shown and refuse when any of them is no longer what the bundle runs; `layout trust` names an item by an id that is a hash of all of it, and everything at once by a hash of the whole list it printed ([`layout::set_of`]).
//!
//! **An import reads and checks off the driver thread.** The directory is somebody else's, so it is read as plain files of bounded size, and on the shell's one import worker rather than where the windows are drawn, which is also where what its files say is checked against the module and command tables; the pictures are staged beside where they will be kept, and only what has to touch the store — the clashes with what this installation holds, the writes — comes back to the driver thread ([`import`]).

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use layout::bundle::{self, Bundle, Manifest};
use layout::{Item, KomponentId, LayoutId, LayoutStore, Library, Rule, Trust, Verdict};
use services::state::{BundleRecord, ShellState, TrustDecision};
use telar::{ReadSignal, RwSignal, signal};
use util::report::{Finding, Message, Report};

use crate::catalogue::{Descriptors, Tables};

thread_local! {
    static PENDING: RwSignal<bool> = telar::detached(|| signal(false));
    static WORKER: RefCell<Option<mpsc::Sender<(PathBuf, Checking)>>> = const { RefCell::new(None) };
    /// What the import in flight installs with once its bundle is read: at most one at a time.
    static IMPORTING: RefCell<Option<Installing>> = const { RefCell::new(None) };
    static DIALOG_OPENED: Cell<Option<Instant>> = const { Cell::new(None) };
}

/// What the shell remembers of the bundles it imported, as resolution reads it, with `rule` saying which action lines a bundle's file runs unasked. Each record's answers are its own: two records claiming one file leave that file to neither ([`Trust::import`]).
pub fn trust_of(state: &ShellState, rule: Rule) -> Trust {
    let mut trust = Trust::with_rule(rule);
    for (name, record) in &state.bundles {
        for file in record.files.keys() {
            trust.import(file.clone(), name.clone());
        }
        for decision in &record.decisions {
            trust.recall(
                name,
                &decision.file,
                &decision.key,
                &decision.text,
                decision.lock_safe,
                decision.accepted,
            );
        }
    }
    trust
}

/// Makes the store's trust the one machine state holds now, under the rule it already has.
fn refresh(store: &mut LayoutStore) {
    let rule = store.all().trust.rule();
    store.set_trust(trust_of(&services::state::get(), rule));
}

/// Whether any item of an imported bundle waits for the user's answer: what raises the trust dialog. Follows every import, every answer and every edit of an imported file.
pub fn pending() -> ReadSignal<bool> {
    PENDING.with(|pending| pending.read_only())
}

/// Brings [`pending`] in line with `store`, after anything that could have changed what waits.
pub(crate) fn note(store: &LayoutStore) {
    let waiting = bundled(store.all(), &services::state::get())
        .iter()
        .any(|bundle| bundle.pending().next().is_some());
    PENDING.with(|pending| {
        if pending.peek() != waiting {
            pending.set(waiting);
        }
    });
}

/// Takes out of `bundles` every claim on a file of `files` that `store` no longer holds and that is not on disk, with every answer about it, and every record left claiming nothing: a file that went away takes its bundle's answers along, so the next file written at that path — by another bundle, or by the same one imported again — starts with none. Answers the names of the records it forgot.
fn release_gone(
    bundles: &mut BTreeMap<String, BundleRecord>,
    files: &[String],
    store: &LayoutStore,
) -> BTreeSet<String> {
    for file in files.iter().filter(|file| !held_by(store, file)) {
        for record in bundles.values_mut() {
            if record.files.remove(file).is_some() {
                record.decisions.retain(|decision| &decision.file != file);
            }
        }
    }
    let forgotten: BTreeSet<String> = bundles
        .iter()
        .filter(|(_, record)| record.files.is_empty())
        .map(|(name, _)| name.clone())
        .collect();
    bundles.retain(|_, record| !record.files.is_empty());
    forgotten
}

/// The files an import of `bundle` may release ([`release_gone`]): those it writes, and every file the record of its name claims, so a bundle whose files were all deleted is forgotten rather than standing in the way of the next one of that name.
fn releasable(bundle: &Bundle, state: &ShellState) -> Vec<String> {
    let mut files: BTreeSet<String> = bundle.files().collect();
    if let Some(record) = state.bundles.get(&bundle.manifest.name) {
        files.extend(record.files.keys().cloned());
    }
    files.into_iter().collect()
}

/// Deletes the picture directory of the forgotten bundle `name`: only a directory of its own under the bundles directory, never through a link, and nothing for a name that could reach outside it, since record names are read back from `state.json`.
fn remove_assets(name: &str) {
    if !layout::is_komponent_name(name) {
        return;
    }
    let dir = assets_dir(name);
    let is_own_dir = std::fs::symlink_metadata(&dir).is_ok_and(|meta| meta.is_dir());
    if is_own_dir && let Err(why) = std::fs::remove_dir_all(&dir) {
        tracing::warn!(
            "removing the pictures of the forgotten bundle `{name}` at {}: {why}",
            dir.display()
        );
    }
}

/// Whether `store` holds the file a finding names `file`, or it is on disk where the store reads it from.
fn held_by(store: &LayoutStore, file: &str) -> bool {
    match Named::of(file) {
        Some(Named::Layout(id)) => store.get(&id).is_some() || store.path_of(&id).exists(),
        Some(Named::Komponent(id)) => {
            store.komponent(&id).is_some() || store.komponent_path(&id).exists()
        }
        None => false,
    }
}

/// One imported bundle and everything its files run, each with where it stands.
#[derive(Clone, Debug, PartialEq)]
pub struct Bundled {
    pub name: String,
    pub items: Vec<(Item, Verdict)>,
}

impl Bundled {
    /// The items waiting for an answer.
    pub fn pending(&self) -> impl Iterator<Item = &Item> {
        self.items
            .iter()
            .filter(|(_, verdict)| *verdict == Verdict::Pending)
            .map(|(item, _)| item)
    }

    /// Every item, each once.
    pub fn all(&self) -> Vec<Item> {
        self.items.iter().map(|(item, _)| item.clone()).collect()
    }
}

/// Every bundle `state` remembers, in name order, with what its files in `library` run — each source as the chains that declare it leave its promise ([`layout::library_items`]), so what is listed is what resolution holds — and where each item stands under `library`'s trust.
pub fn bundled(library: &Library, state: &ShellState) -> Vec<Bundled> {
    let items = layout::library_items(library);
    state
        .bundles
        .iter()
        .map(|(name, record)| Bundled {
            name: name.clone(),
            items: items
                .iter()
                .filter(|item| record.files.contains_key(&item.file))
                .map(|item| (item.clone(), library.trust.verdict(item)))
                .collect(),
        })
        .collect()
}

/// Every bundle the running shell has imported, with its items: what the trust dialog lists.
pub fn bundles() -> Result<Vec<Bundled>, Message> {
    crate::layouts::read(|store| bundled(store.all(), &services::state::get()))
        .ok_or_else(crate::layouts::no_store)
}

/// What a file a finding names holds: a layout or a komponent, by name.
enum Named {
    Layout(LayoutId),
    Komponent(KomponentId),
}

impl Named {
    fn of(file: &str) -> Option<Self> {
        let stem = |dir: &str| {
            file.strip_prefix(dir)?
                .strip_prefix('/')?
                .strip_suffix(".toml")
        };
        match (stem(bundle::LAYOUTS), stem(bundle::COMPONENTS)) {
            (Some(id), _) => Some(Named::Layout(LayoutId::new(id))),
            (_, Some(id)) => Some(Named::Komponent(KomponentId::new(id))),
            _ => None,
        }
    }
}

/// Accepts exactly `items` of the bundle `bundle`, as they were shown: each runs from then on, at that text. Refused whole, nothing recorded, when any of them is not what the bundle runs now — edited, reimported or answered away since it was shown. Answers what was accepted.
pub fn accept(bundle: &str, items: &[Item]) -> Result<Vec<Item>, Message> {
    answer(bundle, true, |_| Ok(items.to_vec()))
}

/// Declines exactly `items`, on the same terms: each stays held, and is reported as declined rather than as waiting.
pub fn decline(bundle: &str, items: &[Item]) -> Result<Vec<Item>, Message> {
    answer(bundle, false, |_| Ok(items.to_vec()))
}

/// Answers for the item of the bundle `bundle` whose [`Item::id`] is `id`: a hash of its file, key, kind, text and lock promise, so the id answers for exactly what was listed under it, or for nothing.
pub fn answer_id(bundle: &str, id: &str, accepted: bool) -> Result<Vec<Item>, Message> {
    answer(bundle, accepted, |found| {
        let chosen: Vec<Item> = found
            .all()
            .into_iter()
            .filter(|item| item.id() == id)
            .collect();
        match chosen.len() {
            0 => Err(util::message!(
                "finding.trust_no_item",
                bundle = bundle,
                id = id
            )),
            1 => Ok(chosen),
            _ => Err(util::message!(
                "finding.trust_ambiguous",
                bundle = bundle,
                id = id
            )),
        }
    })
}

/// Answers for every item of the bundle `bundle`, provided they are still exactly the items listed under the name `set` ([`layout::set_of`]): what `layout trust <bundle> --all <set>` means, so nothing that came to be run since the listing is answered with it.
pub fn answer_set(bundle: &str, set: &str, accepted: bool) -> Result<Vec<Item>, Message> {
    answer(bundle, accepted, |found| {
        let all = found.all();
        match layout::set_of(&all) == set {
            true => Ok(all),
            false => Err(util::message!("finding.trust_set_changed", bundle = bundle)),
        }
    })
}

/// Records `accepted` for what `choose` picks of the bundle `bundle`, each of which must be among what it runs now, and answers what that was.
fn answer(
    bundle: &str,
    accepted: bool,
    choose: impl FnOnce(&Bundled) -> Result<Vec<Item>, Message>,
) -> Result<Vec<Item>, Message> {
    crate::layouts::change(|store| {
        let listed = bundled(store.all(), &services::state::get());
        let found = named(bundle, &listed)?;
        let chosen = distinct(&choose(found)?);
        let current = found.all();
        if chosen.iter().any(|item| !current.contains(item)) {
            return Err(util::message!("finding.trust_changed", bundle = bundle));
        }
        if let Some(item) = chosen
            .iter()
            .find(|item| store.all().trust.bundle_of(&item.file) != Some(bundle))
        {
            return Err(util::message!(
                "finding.trust_contested",
                file = &item.file,
                bundle = bundle
            ));
        }
        services::state::try_update(|state| {
            let record = state
                .bundles
                .get_mut(bundle)
                .ok_or_else(|| unknown_bundle(bundle, &listed))?;
            for item in &chosen {
                record.decisions.retain(|held| !same_text(held, item));
                record.decisions.push(TrustDecision {
                    file: item.file.clone(),
                    key: item.key.clone(),
                    text: item.text.clone(),
                    lock_safe: item.lock_safe,
                    accepted,
                });
            }
            forget_stale(record, &current);
            Ok(())
        })?;
        refresh(store);
        Ok(chosen)
    })
}

/// Each of `items` once: the same text at the same key of the same file, written twice in one chain, is one answer.
fn distinct(items: &[Item]) -> Vec<Item> {
    let mut seen: Vec<Item> = Vec::new();
    for item in items {
        if !seen.contains(item) {
            seen.push(item.clone());
        }
    }
    seen
}

fn same_text(decision: &TrustDecision, item: &Item) -> bool {
    decision.file == item.file
        && decision.key == item.key
        && decision.text == item.text
        && decision.lock_safe == item.lock_safe
}

/// Drops every answer no item of the bundle has at its text any more: an edit or a reimport made it about something that is no longer there.
fn forget_stale(record: &mut BundleRecord, current: &[Item]) {
    record
        .decisions
        .retain(|decision| current.iter().any(|item| same_text(decision, item)));
    record.decisions.sort();
    record.decisions.dedup();
}

/// The bundle called `bundle` among `listed`, or why there is none, naming the ones there are.
pub fn named<'a>(bundle: &str, listed: &'a [Bundled]) -> Result<&'a Bundled, Message> {
    listed
        .iter()
        .find(|held| held.name == bundle)
        .ok_or_else(|| unknown_bundle(bundle, listed))
}

fn unknown_bundle(bundle: &str, listed: &[Bundled]) -> Message {
    let names: Vec<&str> = listed.iter().map(|held| held.name.as_str()).collect();
    match names.is_empty() {
        true => util::message!("finding.no_bundles", bundle = bundle),
        false => util::message!(
            "finding.unknown_bundle",
            bundle = bundle,
            names = names.join(", ")
        ),
    }
}

/// Where an imported bundle's pictures are kept: a directory of its own under the shell's data directory, named after it.
pub fn assets_dir(bundle: &str) -> PathBuf {
    bundles_dir().join(bundle)
}

fn bundles_dir() -> PathBuf {
    util::paths::data_dir().join("bundles")
}

/// What an import did.
#[derive(Clone, Debug, PartialEq)]
pub struct Imported {
    pub manifest: Manifest,
    /// Each file it wrote, as a finding names it.
    pub files: Vec<String>,
    /// How many pictures it copied in.
    pub assets: usize,
    /// Everything the bundle's files run, with where each stands: an import answers nothing, so all of it is pending unless a reimport found the same text answered before.
    pub items: Vec<(Item, Verdict)>,
    /// What is worth saying about the bundle that did not stop it.
    pub warnings: Report,
}

/// What checking a bundle against this installation needs, as the import worker can hold it: the module and command tables and the config, taken where the import starts.
#[derive(Clone)]
pub struct Checking {
    tables: Tables,
    config: config::Config,
}

impl Checking {
    pub fn new(catalogue: &Descriptors, config: &config::Config) -> Self {
        Self {
            tables: catalogue.tables(),
            config: config.clone(),
        }
    }
}

/// A bundle read and checked, and its pictures staged beside where they will be kept, waiting for the driver thread to install it ([`install`]).
pub struct Loaded {
    dir: PathBuf,
    bundle: Bundle,
    staged: Staged,
    warnings: Report,
}

/// A directory of pictures copied in for a bundle not installed yet: removed with everything in it unless the install keeps it.
struct Staged {
    dir: PathBuf,
    kept: bool,
}

impl Drop for Staged {
    fn drop(&mut self) {
        if !self.kept {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
}

/// Reads the bundle in `dir`, checks what its files say against `checking`, and copies the pictures its layouts name to a staging directory beside [`assets_dir`]: all the reading and checking an import does, none of it touching the store, which is what lets it run on the import worker. A picture is read as a plain file of bounded size, never through a link ([`layout::bundle::read`] says which files are read and why).
pub fn load(dir: &Path, checking: &Checking) -> Result<Loaded, Report> {
    let bundle = bundle::read(dir)?;
    let warnings = checked(
        &bundle,
        dir,
        &checking.tables.clone().into(),
        &checking.config,
    );
    if !warnings.errors.is_empty() {
        return Err(warnings);
    }
    sweep_abandoned();
    let staged = Staged {
        dir: bundles_dir().join(format!("{INCOMING}{}", unique())),
        kept: false,
    };
    std::fs::create_dir_all(&staged.dir).map_err(|why| {
        refused(
            &staged.dir,
            Message::verbatim(format!("{}: {why}", staged.dir.display())),
        )
    })?;
    for (name, from) in &bundle.assets {
        let to = staged.dir.join(name);
        let bytes = from
            .read()
            .map_err(|why| refused(&dir.join(bundle::ASSETS).join(name), why))?;
        util::fs::write_atomic(&to, &bytes)
            .map_err(|why| refused(&to, Message::verbatim(format!("{}: {why}", to.display()))))?;
    }
    Ok(Loaded {
        dir: dir.to_path_buf(),
        bundle,
        staged,
        warnings,
    })
}

/// `<pid>-<n>`, a name no other directory this process or another live one makes under the bundles directory has.
fn unique() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    format!(
        "{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

/// What a staging directory's name starts with, before the id of the process staging into it.
const INCOMING: &str = ".incoming-";

/// What the name of a picture directory an install set aside starts with, before the id of the process that did: it goes back if the install fails, and is deleted once it holds.
const OUTGOING: &str = ".outgoing-";

/// Deletes every staging or set-aside directory left by a process that is gone — a shell killed in the middle of an import never ran [`Staged`]'s drop nor finished its install — leaving those of a live process, such as another compositor's shell sharing the data directory. Never through a link.
fn sweep_abandoned() {
    let Ok(entries) = std::fs::read_dir(bundles_dir()) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid) = name
            .to_str()
            .and_then(|it| {
                it.strip_prefix(INCOMING)
                    .or_else(|| it.strip_prefix(OUTGOING))
            })
            .and_then(|it| it.split_once('-'))
            .and_then(|(pid, _)| pid.parse::<u32>().ok())
        else {
            continue;
        };
        let alive = pid == std::process::id() || Path::new("/proc").join(pid.to_string()).exists();
        let is_dir = entry.file_type().is_ok_and(|kind| kind.is_dir());
        if !alive && is_dir {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

fn refused(at: &Path, message: Message) -> Report {
    let mut report = Report::default();
    report.error(Finding::new(at, "", message));
    report
}

/// Installs a bundle [`load`] read and checked into the running shell.
///
/// What its files say was checked on the worker ([`checked`]): each layout, with the komponents it draws, validates against the built-in layout and what the bundle itself holds — a bundle that needs anything of this installation's is not one that can be imported, and one that names a module or a key this shell does not have fails like any layout would, with the findings saying where (DEC-29). What is left needs the store and machine state, so it runs here. A name this installation already uses is refused, so nothing of the user's is overwritten: a bundle record of that name made by a different bundle ([`layout::bundle::Bundle::identity`]), a file another bundle wrote, a picture directory no record accounts for, and a file of the same name the same bundle did not write unchanged. A reimport of the same bundle replaces its own files nobody has changed since, and its pictures.
///
/// Then the staged pictures take the place of the bundle's picture directory, what the import writes is recorded as the bundle's, and the layouts and komponents are put in the store and written, with every picture named where it now is ([`LayoutStore::put_bundle`]). The record comes first, since a file the store held unrecorded would be drawn as the user's own, its commands running; so it is taken back to exactly what was written when a write fails — every file failing puts the record and the pictures back as they were — and state never claims a file nobody wrote. A record the import forgot, every file it claimed gone, takes its picture directory with it once the install holds. What its files run is left pending.
pub fn install(loaded: Loaded) -> Result<Imported, Report> {
    let Loaded {
        dir,
        bundle,
        mut staged,
        mut warnings,
    } = loaded;
    let name = bundle.manifest.name.clone();
    let assets = assets_dir(&name);
    let at = |message: Message| refused(&dir, message);
    let identity = bundle.identity();
    crate::layouts::change_or(
        |store| {
            if store.is_safe() {
                return Err(at(layout::StoreError::Safe.message()));
            }
            let mut state = services::state::get();
            let releasing = releasable(&bundle, &state);
            let forgotten = release_gone(&mut state.bundles, &releasing, store);
            let clashing = clashes(&bundle, (&identity, &forgotten), store, &state);
            if !clashing.errors.is_empty() {
                return Err(clashing);
            }
            let installed = bundle.clone().installed_at(&assets);
            let mut written: BTreeMap<String, String> = BTreeMap::new();
            for (id, komponent) in &installed.komponents {
                let print = bundle::fingerprint(komponent)
                    .map_err(|why| at(Message::verbatim(why.to_string())))?;
                written.insert(layout::komponent_path(id), print);
            }
            for layout in installed.layouts.values() {
                let print = bundle::fingerprint(layout)
                    .map_err(|why| at(Message::verbatim(why.to_string())))?;
                written.insert(layout::layout_path(&layout.id), print);
            }
            let set_aside = put_in_place(&staged.dir, &assets)
                .map_err(|why| at(Message::verbatim(format!("{}: {why}", assets.display()))))?;
            staged.kept = true;
            let mut forgotten = BTreeSet::new();
            let mut recorded_before: BTreeMap<String, Option<BundleRecord>> = BTreeMap::new();
            services::state::update(|state| {
                let before = state.bundles.clone();
                let releasing = releasable(&bundle, state);
                forgotten = release_gone(&mut state.bundles, &releasing, store);
                let record = state.bundles.entry(name.clone()).or_default();
                record.identity = identity.clone();
                record.files.extend(written.clone());
                recorded_before = touched(&before, &state.bundles);
            });
            refresh(store);
            let put = store.put_bundle(
                installed.layouts.into_values().collect(),
                installed.komponents.into_iter().collect(),
            );
            let wrote = match put {
                Ok(wrote) if !wrote.files.is_empty() => wrote,
                outcome => {
                    services::state::update(|state| put_back(&mut state.bundles, recorded_before));
                    refresh(store);
                    set_aside.put_back(&assets);
                    return Err(match outcome {
                        Ok(wrote) => wrote.failed,
                        Err(refused) => at(refused.message()),
                    });
                }
            };
            set_aside.forget();
            for other in forgotten.iter().filter(|other| **other != name) {
                remove_assets(other);
            }
            let unwritten: Vec<&String> = written
                .keys()
                .filter(|file| !wrote.files.contains(*file))
                .collect();
            if !unwritten.is_empty() {
                let before = recorded_before.get(&name).and_then(Option::as_ref);
                services::state::update(|state| {
                    let Some(record) = state.bundles.get_mut(&name) else {
                        return;
                    };
                    for file in &unwritten {
                        match before.and_then(|it| it.files.get(*file)) {
                            Some(print) => {
                                record.files.insert((*file).clone(), print.clone());
                            }
                            None => {
                                record.files.remove(*file);
                                record.decisions.retain(|decision| &decision.file != *file);
                            }
                        }
                    }
                });
                refresh(store);
            }
            let state = services::state::get();
            let items = bundled(store.all(), &state)
                .into_iter()
                .find(|held| held.name == name)
                .map(|held| held.items)
                .unwrap_or_default();
            let current: Vec<Item> = items.iter().map(|(item, _)| item.clone()).collect();
            services::state::update(|state| {
                if let Some(record) = state.bundles.get_mut(&name) {
                    forget_stale(record, &current);
                }
            });
            refresh(store);
            if !wrote.failed.errors.is_empty() {
                return Err(wrote.failed);
            }
            Ok(Imported {
                manifest: bundle.manifest.clone(),
                files: wrote.files.into_iter().collect(),
                assets: bundle.assets.len(),
                items,
                warnings: std::mem::take(&mut warnings),
            })
        },
        at,
    )
}

/// Each record of `after` that is not as it is in `before`, with what it was there: `None` for one `before` did not hold.
fn touched(
    before: &BTreeMap<String, BundleRecord>,
    after: &BTreeMap<String, BundleRecord>,
) -> BTreeMap<String, Option<BundleRecord>> {
    before
        .keys()
        .chain(after.keys())
        .filter(|name| before.get(*name) != after.get(*name))
        .map(|name| (name.clone(), before.get(name).cloned()))
        .collect()
}

/// Puts each record `touched` names back as it was, and nothing else: what an install that wrote nothing recorded is taken back without touching a record it did not change.
fn put_back(
    bundles: &mut BTreeMap<String, BundleRecord>,
    touched: BTreeMap<String, Option<BundleRecord>>,
) {
    for (name, was) in touched {
        match was {
            Some(record) => bundles.insert(name, record),
            None => bundles.remove(&name),
        };
    }
}

/// The picture directory a reimport replaced, kept aside until the install holds.
struct SetAside(Option<PathBuf>);

impl SetAside {
    /// Puts the set-aside pictures back as `assets`, in place of what the failed install moved there.
    fn put_back(self, assets: &Path) {
        let _ = std::fs::remove_dir_all(assets);
        if let Some(aside) = &self.0
            && let Err(why) = std::fs::rename(aside, assets)
        {
            tracing::warn!(
                "putting the pictures at {} back as {}: {why}",
                aside.display(),
                assets.display()
            );
        }
    }

    /// Deletes the set-aside pictures: the install holds.
    fn forget(self) {
        if let Some(aside) = &self.0 {
            let _ = std::fs::remove_dir_all(aside);
        }
    }
}

/// Makes the staged pictures in `staged` the bundle's picture directory `assets`, setting aside what a reimport of it kept there before.
fn put_in_place(staged: &Path, assets: &Path) -> std::io::Result<SetAside> {
    std::fs::create_dir_all(staged)?;
    let aside = match std::fs::symlink_metadata(assets) {
        Ok(_) => {
            let aside = bundles_dir().join(format!("{OUTGOING}{}", unique()));
            std::fs::rename(assets, &aside)?;
            Some(aside)
        }
        Err(_) => None,
    };
    if let Err(why) = std::fs::rename(staged, assets) {
        SetAside(aside).put_back(assets);
        return Err(why);
    }
    Ok(SetAside(aside))
}

/// Where an import started with [`import`] has got to.
pub enum Importing {
    /// The bundle is being read and checked on the import worker; the driver thread installs it once it is, and says how that went — to whoever [`Reading::answer_with`] names, or in a notice.
    Reading(Reading),
    /// There is no event loop to hand a result back to — a test, a process that is not the shell — so it was read, checked and installed here.
    Done(Box<Result<Imported, Report>>),
}

/// The import in flight, as whoever started it holds it.
pub struct Reading(());

impl Reading {
    /// Hands how the import went to `reply` rather than to a notice, once it is installed: `reply` answers whether that reached whoever asked, and where it did not — they stopped waiting — the notice says it after all.
    pub fn answer_with(self, reply: impl FnOnce(&Result<Imported, Report>) -> bool + 'static) {
        IMPORTING.with(|importing| {
            if let Some(installing) = importing.borrow_mut().as_mut() {
                installing.reply = Some(Box::new(reply));
            }
        });
    }
}

/// Who is told how an import went, answering whether it reached them.
type ImportReply = Box<dyn FnOnce(&Result<Imported, Report>) -> bool>;

/// The import in flight, and who it answers.
struct Installing {
    dir: PathBuf,
    reply: Option<ImportReply>,
}

/// Imports the bundle in `dir` into the running shell: read and checked against `catalogue` and `config` on the import worker ([`load`]), then installed on the driver thread ([`install`]) and said to whoever waits for it ([`Reading::answer_with`]) or else in a notice, with the trust dialog opened when the bundle brought something new to answer. One import at a time: a second while one is reading is refused.
pub fn import(
    dir: &Path,
    catalogue: &Descriptors,
    config: &config::Config,
) -> Result<Importing, Message> {
    if IMPORTING.with(|importing| importing.borrow().is_some()) {
        return Err(util::message!("finding.bundle_importing"));
    }
    let checking = Checking::new(catalogue, config);
    IMPORTING.with(|importing| {
        *importing.borrow_mut() = Some(Installing {
            dir: dir.to_path_buf(),
            reply: None,
        })
    });
    if ask_worker(dir, &checking) {
        return Ok(Importing::Reading(Reading(())));
    }
    IMPORTING.with(|importing| importing.borrow_mut().take());
    let done = load(dir, &checking).and_then(install);
    Ok(Importing::Done(Box::new(done)))
}

/// Hands `dir` to the import worker, starting it again once if the one there stopped. `false` where there is no event loop to run one on.
fn ask_worker(dir: &Path, checking: &Checking) -> bool {
    for _ in 0..2 {
        let Some(worker) = worker() else {
            return false;
        };
        if worker.send((dir.to_path_buf(), checking.clone())).is_ok() {
            return true;
        }
        WORKER.with(|held| held.borrow_mut().take());
    }
    false
}

/// The import worker, started on the first import: one thread reading and checking bundles in the order asked, its results delivered on the driver thread. `None` where there is no event loop to deliver them on.
fn worker() -> Option<mpsc::Sender<(PathBuf, Checking)>> {
    WORKER.with(|held| {
        if let Some(worker) = held.borrow().as_ref() {
            return Some(worker.clone());
        }
        let (requests, incoming) = mpsc::channel::<(PathBuf, Checking)>();
        platform_wayland::app_watch(
            move |sender| {
                for (dir, checking) in incoming {
                    if !sender.send(load_guarded(&dir, &checking)) {
                        return;
                    }
                }
            },
            finish,
        )?;
        *held.borrow_mut() = Some(requests.clone());
        Some(requests)
    })
}

/// [`load`], with a panic in it caught at the worker's edge and answered as the import's failure: the worker lives on, and the driver thread hears how the import ended, so it never waits on one forever.
pub fn load_guarded(dir: &Path, checking: &Checking) -> Result<Loaded, Report> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| load(dir, checking))).unwrap_or_else(
        |panic| {
            let why = panic
                .downcast_ref::<&str>()
                .map(|it| it.to_string())
                .or_else(|| panic.downcast_ref::<String>().cloned())
                .unwrap_or_default();
            Err(refused(
                dir,
                util::message!("finding.bundle_import_panicked", why = why),
            ))
        },
    )
}

/// Installs what the worker read, on the driver thread, and says how it went: to whoever waits for it, else in a notice.
fn finish(loaded: Result<Loaded, Report>) {
    let Some(installing) = IMPORTING.with(|importing| importing.borrow_mut().take()) else {
        return;
    };
    let waiting_before = waiting_ids();
    let outcome = loaded.and_then(install);
    let answered = installing.reply.is_some_and(|reply| reply(&outcome));
    if !answered {
        announce(&installing.dir, &outcome, &waiting_before);
    }
}

fn waiting_ids() -> BTreeSet<String> {
    bundles()
        .unwrap_or_default()
        .iter()
        .flat_map(|bundle| bundle.pending().map(Item::id).collect::<Vec<_>>())
        .collect()
}

/// How long after the trust dialog was opened for an import another import may open it again: what keeps a run of imports from holding the keyboard.
const DIALOG_AGAIN: Duration = Duration::from_secs(10);

/// Says how an import that ran on the worker went: a notice either way, and the trust dialog where it brought something to answer that was not waiting before.
fn announce(dir: &Path, outcome: &Result<Imported, Report>, waiting_before: &BTreeSet<String>) {
    use services::notifications::{Urgency, notify_shell};
    match outcome {
        Ok(imported) => {
            let waiting = imported
                .items
                .iter()
                .filter(|(_, verdict)| *verdict == Verdict::Pending)
                .count();
            notify_shell(
                "hogar-shell",
                &telar::t!(
                    "notice.bundle_imported",
                    bundle = imported.manifest.name.clone()
                ),
                &telar::t!("notice.bundle_imported_body", count = waiting),
                "document-import",
                Urgency::Normal,
            );
            let brought = imported.items.iter().any(|(item, verdict)| {
                *verdict == Verdict::Pending && !waiting_before.contains(&item.id())
            });
            if brought {
                open_dialog_once();
            }
        }
        Err(report) => {
            tracing::warn!("importing {} failed:\n{}", dir.display(), report.render());
            notify_shell(
                "hogar-shell",
                &telar::t!(
                    "notice.bundle_import_failed",
                    path = util::text::shown(&dir.display().to_string())
                ),
                &failure_body(report),
                "dialog-error",
                Urgency::Normal,
            );
        }
    }
}

/// What the notice of a failed import says: the first few findings, each in the user's language with whatever it quotes written out ([`util::report::Message::render`]); [`services::notifications::notify_shell`] keeps it from being read as markup.
pub(crate) fn failure_body(report: &Report) -> String {
    report
        .errors
        .iter()
        .take(3)
        .map(|finding| finding.message.render())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Opens the trust dialog for an import, unless an import opened it within [`DIALOG_AGAIN`].
pub fn open_dialog_once() -> Option<Result<usize, Message>> {
    let now = Instant::now();
    let recently = DIALOG_OPENED
        .with(Cell::get)
        .is_some_and(|at| now.duration_since(at) < DIALOG_AGAIN);
    if recently {
        return None;
    }
    let opened = crate::trust_dialog::open();
    if opened.is_ok() {
        DIALOG_OPENED.with(|at| at.set(Some(now)));
    }
    Some(opened)
}

/// Each name `bundle` would take that this installation already uses for something other than what the same bundle wrote unchanged: a record of that name a different bundle made, a file another bundle's record claims, a file that is there and is not the unchanged copy this bundle wrote (one that could not be read included, since it is the user's all the same), and a picture directory of that name no record accounts for — one a record forgotten by this import kept is that record's, and goes with it.
fn clashes(
    bundle: &Bundle,
    (identity, forgotten): (&str, &BTreeSet<String>),
    store: &LayoutStore,
    state: &ShellState,
) -> Report {
    let mut report = Report::default();
    let name = &bundle.manifest.name;
    let record = state.bundles.get(name);
    if let Some(record) = record
        && record.identity != identity
    {
        let files: Vec<&str> = record.files.keys().map(String::as_str).collect();
        let writes: Vec<String> = bundle.files().collect();
        report.error(Finding::new(
            bundle::MANIFEST,
            "name",
            util::message!(
                "finding.bundle_impostor",
                bundle = name,
                files = files.join(", "),
                writes = writes.join(", ")
            ),
        ));
        return report;
    }
    let unaccounted = record.is_none()
        && !forgotten.contains(name)
        && std::fs::read_dir(assets_dir(name)).is_ok_and(|mut it| it.next().is_some());
    if unaccounted {
        report.error(Finding::new(
            assets_dir(name),
            "",
            util::message!(
                "finding.bundle_assets_clash",
                path = assets_dir(name).display(),
                bundle = name
            ),
        ));
    }
    for file in bundle.files() {
        if let Some((other, _)) = state
            .bundles
            .iter()
            .find(|(other, held)| *other != name && held.files.contains_key(&file))
        {
            report.error(Finding::new(
                &file,
                "",
                util::message!(
                    "finding.bundle_claimed",
                    file = &file,
                    bundle = name,
                    other = other
                ),
            ));
            continue;
        }
        let (held, path) = match Named::of(&file) {
            Some(Named::Layout(id)) => {
                (store.get(&id).map(bundle::fingerprint), store.path_of(&id))
            }
            Some(Named::Komponent(id)) => (
                store.komponent(&id).map(bundle::fingerprint),
                store.komponent_path(&id),
            ),
            None => continue,
        };
        if held.is_none() && !path.exists() {
            continue;
        }
        let unchanged = record
            .and_then(|record| record.files.get(&file))
            .is_some_and(|wrote| {
                held.as_ref()
                    .is_some_and(|it| it.as_ref().ok() == Some(wrote))
            });
        if !unchanged {
            report.error(Finding::new(
                &file,
                "",
                util::message!("finding.bundle_clash", file = &file, bundle = name),
            ));
        }
    }
    report
}

/// Everything wrong with what `bundle`'s files say, checked against the built-in layout and the bundle alone, each finding naming the file in `dir` it is about.
fn checked(
    bundle: &Bundle,
    dir: &Path,
    catalogue: &Descriptors,
    config: &config::Config,
) -> Report {
    let mut library = Library::of_layouts(
        std::iter::once(layout::built_in()).chain(bundle.layouts.values().cloned()),
    );
    library.komponents = bundle.komponents.clone();
    let text = |file: &str| bundle.texts.get(file).cloned();
    let theme = config.resolve_theme();
    let mut report = Report::default();
    let mut drawn = BTreeSet::new();
    for layout in bundle.layouts.values() {
        let (found, resolved) =
            catalogue
                .clone()
                .check(layout, &library, &text, &config.automation);
        report.merge(found);
        report.merge(layout::validate_resolved(
            &resolved,
            &layout::layout_path(&layout.id),
            &theme,
        ));
        drawn.extend(layout::komponents_of(layout, &library));
    }
    for (id, komponent) in bundle
        .komponents
        .iter()
        .filter(|(id, _)| !drawn.contains(*id))
    {
        let mut found = layout::validate_komponent(id, komponent, catalogue);
        if let Some(written) = text(&layout::komponent_path(id)) {
            layout::locate_komponent_expressions(&written, &mut found);
        }
        report.merge(found);
    }
    let mut seen = Vec::new();
    for findings in [&mut report.errors, &mut report.warnings] {
        findings.retain(|finding| {
            let new = !seen.contains(finding);
            if new {
                seen.push(finding.clone());
            }
            new
        });
    }
    for finding in report.findings_mut() {
        if finding.file.is_relative() {
            finding.file = dir.join(&finding.file);
        }
    }
    report
}
