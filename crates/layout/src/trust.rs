//! What an imported bundle may run, and what it may not yet (DEC-30).
//!
//! A layout or komponent file that came with a bundle can make the shell do things the user did not write (F-10.54): a `poll` or `listen` source's command, an `http` source's address, and every line of an action chain except the few that only move what the shell shows ([`Trust::runs_unasked`], the command table's own allow-list). Each is an [`Item`]: where it is written — the file and the key, as a finding names them — and its exact text. Trust is decided per item, per bundle, and bound to that text, so an edit or a reimport that changes it asks again for that item alone, and nothing else in the file is affected.
//!
//! [`Trust`] is what resolution reads it from, carried by the [`Library`]: a layout resolved against it draws the action lines it has not accepted as never written ([`Trust::gate_layout`], [`Trust::gate_komponent`]), and [`crate::sources`] leaves the sources it has not accepted out. A file no bundle brought is the user's own and is never held, whatever it runs — which is why no edit may copy a held text into one ([`carried`], which the layout store checks every write against).

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};

use sha2::{Digest, Sha256};
use util::report::{Finding, Message, Report};

use crate::library::{Library, komponent_path};
use crate::model::*;
use crate::ops::{sites, sites_mut};
use crate::resolve::{chain_of, layout_path};

/// What an item does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ItemKind {
    /// A `poll` source's `cmd`.
    Poll,
    /// A `listen` source's `cmd`.
    Listen,
    /// An `http` source's `url`.
    Http,
    /// A line of an action chain that does more than move what the shell shows.
    Action,
}

impl ItemKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ItemKind::Poll => "poll",
            ItemKind::Listen => "listen",
            ItemKind::Http => "http",
            ItemKind::Action => "action",
        }
    }

    /// What an item of this kind does, in no language yet.
    pub fn message(self) -> Message {
        match self {
            ItemKind::Poll => util::message!("finding.trust_kind.poll"),
            ItemKind::Listen => util::message!("finding.trust_kind.listen"),
            ItemKind::Http => util::message!("finding.trust_kind.http"),
            ItemKind::Action => util::message!("finding.trust_kind.action"),
        }
    }
}

/// One thing a file can make the shell do, where it is written and exactly as it is written.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Item {
    /// The file, as findings name it: `layouts/<name>.toml` or `components/<name>.toml`.
    pub file: String,
    /// The key it is written at: `sources.<name>.cmd`, `sources.<name>.url`, or the `actions.<trigger>` of an area or an instance.
    pub key: String,
    pub kind: ItemKind,
    /// The command, the address or the line, as written.
    pub text: String,
    /// Whether the file says the lock screen may show what the source reads. Shown beside the command, and part of what is accepted: a file that starts saying it asks again.
    pub lock_safe: bool,
}

impl Item {
    /// A short name for the item, the same in every process for the same file, key, kind, text and promise, which is what `layout trust <bundle> <item>` takes: [`ID_BYTES`] of a SHA-256 of all of them, so a name the user copied answers for exactly the item they were shown and for nothing a later edit made.
    pub fn id(&self) -> String {
        digest(
            &[
                self.file.as_bytes(),
                self.key.as_bytes(),
                self.kind.as_str().as_bytes(),
                self.text.as_bytes(),
                if self.lock_safe { b"1" } else { b"0" },
            ],
            ID_BYTES,
        )
    }

    /// The text as it can be shown: every character that would hide, reorder or rewrite what is around it written out instead ([`util::text::shown`]).
    pub fn shown(&self) -> String {
        util::text::shown(&self.text)
    }

    /// What it runs, as a finding names it.
    pub fn what(&self) -> Message {
        let text = self.shown();
        match self.kind {
            ItemKind::Poll | ItemKind::Listen => {
                util::message!("finding.trust_what.command", text = &text)
            }
            ItemKind::Http => util::message!("finding.trust_what.address", text = &text),
            ItemKind::Action => util::message!("finding.trust_what.action", text = &text),
        }
    }
}

/// A name for exactly the items `items` are, whatever order they come in: what `layout trust <bundle> --all <set>` takes, so accepting everything accepts what was listed and nothing that came to be listed since.
pub fn set_of(items: &[Item]) -> String {
    let ids: BTreeSet<String> = items.iter().map(Item::id).collect();
    let joined: Vec<&[u8]> = ids.iter().map(|id| id.as_bytes()).collect();
    digest(&joined, ID_BYTES)
}

/// How much of a SHA-256 an item's id and a set's id keep: 128 bits, 32 hex digits, so finding a second text that answers to an id the user was shown is out of reach as it is for the whole hash.
pub const ID_BYTES: usize = 16;

/// SHA-256 over `parts`, each length-prefixed so no two lists of parts read alike, as the hex of its first `bytes` bytes.
pub(crate) fn digest(parts: &[&[u8]], bytes: usize) -> String {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update((part.len() as u64).to_le_bytes());
        hasher.update(part);
    }
    hasher.finalize()[..bytes]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Where an item stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Verdict {
    /// In a file no bundle brought: the user's own, which always runs.
    Own,
    /// Brought by a bundle and not decided at this text.
    Pending,
    Accepted,
    Declined,
}

impl Verdict {
    pub fn runs(self) -> bool {
        matches!(self, Verdict::Own | Verdict::Accepted)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Verdict::Own => "own",
            Verdict::Pending => "pending",
            Verdict::Accepted => "accepted",
            Verdict::Declined => "declined",
        }
    }

    /// What it is, in no language yet.
    pub fn message(self) -> Message {
        match self {
            Verdict::Own => util::message!("finding.trust_verdict.own"),
            Verdict::Pending => util::message!("finding.trust_verdict.pending"),
            Verdict::Accepted => util::message!("finding.trust_verdict.accepted"),
            Verdict::Declined => util::message!("finding.trust_verdict.declined"),
        }
    }
}

/// An item as one bundle's answer was given for it: the text and the lock promise that were shown.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Decided {
    bundle: String,
    file: String,
    key: String,
    text: String,
    lock_safe: bool,
}

/// Who brought a file.
#[derive(Clone, Debug, PartialEq)]
enum Claim {
    Bundle(String),
    /// Two records say they wrote it, which machine state should never hold: nothing it runs is trusted until that is untangled.
    Contested(BTreeSet<String>),
}

/// The lines an action chain may run without anybody's trust: the command table's answer, which this crate cannot see.
pub type Rule = fn(&str) -> bool;

fn nothing_runs_unasked(_: &str) -> bool {
    false
}

/// Which files came with which bundle, and what the user decided about what they run.
#[derive(Clone, Debug)]
pub struct Trust {
    claims: BTreeMap<String, Claim>,
    decided: BTreeMap<Decided, bool>,
    runs_unasked: Rule,
}

/// A trust with no bundle in it, under which every action line of a bundle's file would be an item.
impl Default for Trust {
    fn default() -> Self {
        Self::with_rule(nothing_runs_unasked)
    }
}

impl PartialEq for Trust {
    fn eq(&self, other: &Self) -> bool {
        self.claims == other.claims
            && self.decided == other.decided
            && std::ptr::fn_addr_eq(self.runs_unasked, other.runs_unasked)
    }
}

impl Trust {
    /// No bundle yet, and `runs_unasked` saying which action lines a bundle's file runs without anybody's trust.
    pub fn with_rule(runs_unasked: Rule) -> Self {
        Self {
            claims: BTreeMap::new(),
            decided: BTreeMap::new(),
            runs_unasked,
        }
    }

    /// The rule this trust was made with, for a trust made again from newer machine state.
    pub fn rule(&self) -> Rule {
        self.runs_unasked
    }

    /// Records that `file` came with the bundle `bundle`. A file two bundles claim belongs to neither, and nothing in it runs.
    pub fn import(&mut self, file: impl Into<String>, bundle: impl Into<String>) {
        let (file, bundle) = (file.into(), bundle.into());
        let claim = match self.claims.remove(&file) {
            None => Claim::Bundle(bundle),
            Some(Claim::Bundle(held)) if held == bundle => Claim::Bundle(held),
            Some(Claim::Bundle(held)) => Claim::Contested(BTreeSet::from([held, bundle])),
            Some(Claim::Contested(mut all)) => {
                all.insert(bundle);
                Claim::Contested(all)
            }
        };
        self.claims.insert(file, claim);
    }

    /// Records the user's answer for `item` at the text it has now, for the bundle its file came with. Answers whether there was one such bundle to record it for: a file of the user's own needs no answer, and a contested one takes none.
    pub fn decide(&mut self, item: &Item, accepted: bool) -> bool {
        let Some(bundle) = self.bundle_of(&item.file).map(str::to_string) else {
            return false;
        };
        self.decided.insert(
            Decided {
                bundle,
                file: item.file.clone(),
                key: item.key.clone(),
                text: item.text.clone(),
                lock_safe: item.lock_safe,
            },
            accepted,
        );
        true
    }

    /// Records an answer given before, as machine state kept it: the bundle `bundle`'s, for the item at `key` of `file` while it read `text` and promised `lock_safe`.
    pub fn recall(
        &mut self,
        bundle: &str,
        file: &str,
        key: &str,
        text: &str,
        lock_safe: bool,
        accepted: bool,
    ) {
        let decided = Decided {
            bundle: bundle.to_string(),
            file: file.to_string(),
            key: key.to_string(),
            text: text.to_string(),
            lock_safe,
        };
        self.decided.insert(decided, accepted);
    }

    /// The one bundle `file` came with, or `None` for one of the user's own or one two bundles claim.
    pub fn bundle_of(&self, file: &str) -> Option<&str> {
        match self.claims.get(file)? {
            Claim::Bundle(bundle) => Some(bundle),
            Claim::Contested(_) => None,
        }
    }

    /// Whether any bundle claims `file`: what makes it held item by item rather than the user's own.
    pub fn is_bundled(&self, file: &str) -> bool {
        self.claims.contains_key(file)
    }

    /// The bundles that claim `file` where more than one does.
    pub fn contesting(&self, file: &str) -> Option<&BTreeSet<String>> {
        match self.claims.get(file)? {
            Claim::Contested(all) => Some(all),
            Claim::Bundle(_) => None,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.claims.is_empty()
    }

    pub fn verdict(&self, item: &Item) -> Verdict {
        if !self.is_bundled(&item.file) {
            return Verdict::Own;
        }
        let Some(bundle) = self.bundle_of(&item.file) else {
            return Verdict::Pending;
        };
        let decided = Decided {
            bundle: bundle.to_string(),
            file: item.file.clone(),
            key: item.key.clone(),
            text: item.text.clone(),
            lock_safe: item.lock_safe,
        };
        match self.decided.get(&decided) {
            Some(true) => Verdict::Accepted,
            Some(false) => Verdict::Declined,
            None => Verdict::Pending,
        }
    }

    /// Whether an action line of a bundle's file runs without anybody's trust: one the command table counts as only moving what the shell shows, and never one that grants trust.
    pub fn runs_unasked(&self, line: &str) -> bool {
        !grants_trust(line) && (self.runs_unasked)(line)
    }

    /// Whether an action line of a bundle's file is an item: everything but what runs unasked and what never runs at all.
    pub fn is_item(&self, line: &str) -> bool {
        !grants_trust(line) && !(self.runs_unasked)(line)
    }

    /// `layout` as it may run: every action line that is an item not accepted at the text it has taken out of its chain. The chain keeps its trigger, so a held line leaves the gesture doing nothing rather than handing it to something else.
    pub fn gate_layout<'a>(&self, layout: &'a Layout) -> Cow<'a, Layout> {
        let file = layout_path(&layout.id);
        if !self.is_bundled(&file) {
            return Cow::Borrowed(layout);
        }
        let mut gated = layout.clone();
        for (key, action) in layout_chains_mut(&mut gated) {
            self.keep_runnable(&file, &key, action);
        }
        Cow::Owned(gated)
    }

    /// The same for the komponent `id`.
    pub fn gate_komponent<'a>(
        &self,
        id: &KomponentId,
        komponent: &'a Komponent,
    ) -> Cow<'a, Komponent> {
        let file = komponent_path(id);
        if !self.is_bundled(&file) {
            return Cow::Borrowed(komponent);
        }
        let mut gated = komponent.clone();
        for child in &mut gated.children {
            let at = format!("children.{}", child.id);
            for (trigger, action) in &mut child.actions {
                self.keep_runnable(&file, &format!("{at}.actions.{}", trigger.as_str()), action);
            }
        }
        Cow::Owned(gated)
    }

    fn keep_runnable(&self, file: &str, key: &str, action: &mut Action) {
        action.0.retain(|line| {
            self.runs_unasked(line)
                || (self.is_item(line) && self.verdict(&action_item(file, key, line)).runs())
        });
    }

    fn chain_items(&self, file: &str, chains: Vec<(String, &Action)>) -> Vec<Item> {
        chains
            .into_iter()
            .flat_map(|(key, action)| {
                action
                    .0
                    .iter()
                    .filter(|line| self.is_item(line))
                    .map(move |line| action_item(file, &key, line))
                    .collect::<Vec<_>>()
            })
            .collect()
    }
}

/// Whether `line` is `layout trust …`, which no action may run: trust is the user's to give, and a file that could press it for them would trust itself. Read the way the command table reads a line: its first two whitespace-separated words.
pub fn grants_trust(line: &str) -> bool {
    let mut words = line.split_whitespace();
    words.next() == Some("layout") && words.next() == Some("trust")
}

fn action_item(file: &str, key: &str, line: &str) -> Item {
    Item {
        file: file.to_string(),
        key: key.to_string(),
        kind: ItemKind::Action,
        text: line.to_string(),
        lock_safe: false,
    }
}

/// What `layout`'s own file can make the shell do, in file order: the command or address of each source it writes one for, then each action line of its chains that is an item under `trust`.
pub fn layout_items(layout: &Layout, trust: &Trust) -> Vec<Item> {
    let file = layout_path(&layout.id);
    let mut items: Vec<Item> = layout
        .sources
        .iter()
        .filter_map(|(name, source)| source_item(&file, name, source))
        .collect();
    items.extend(trust.chain_items(&file, layout_chains(layout)));
    items
}

/// What the komponent `id` can make the shell do: each action line of its children that is an item under `trust`.
pub fn komponent_items(id: &KomponentId, komponent: &Komponent, trust: &Trust) -> Vec<Item> {
    trust.chain_items(&komponent_path(id), komponent_chains(komponent))
}

fn source_item(file: &str, name: &str, source: &Source) -> Option<Item> {
    let (kind, field, text) = match source {
        Source::Poll { cmd: Some(cmd), .. } => (ItemKind::Poll, "cmd", cmd),
        Source::Listen { cmd: Some(cmd), .. } => (ItemKind::Listen, "cmd", cmd),
        Source::Http { url: Some(url), .. } => (ItemKind::Http, "url", url),
        _ => return None,
    };
    Some(Item {
        file: file.to_string(),
        key: format!("sources.{name}.{field}"),
        kind,
        text: text.clone(),
        lock_safe: source.lock_safe(),
    })
}

/// For each source `chain` declares, the item of the level that wrote what it runs — the last level to write its `cmd` or `url`, or to make it another kind of source — with the `lock_safe` the whole chain leaves it with. A source no level gives a command or address has none.
///
/// What is accepted is a text and the promise it runs under, so a later level that starts promising the lock screen what an earlier one runs asks again, whichever file that level is.
pub(crate) fn source_items(chain: &[&Layout]) -> BTreeMap<String, Item> {
    let mut merged: BTreeMap<String, Source> = BTreeMap::new();
    let mut writers: BTreeMap<&str, &Layout> = BTreeMap::new();
    for level in chain {
        for (name, source) in &level.sources {
            let writes = match merged.get(name) {
                Some(held) if held.is_same_kind(source) => {
                    source.cmd().is_some() || source.url().is_some()
                }
                _ => true,
            };
            if writes {
                writers.insert(name, level);
            }
        }
        crate::merge::merge_sources(&mut merged, &level.sources);
    }
    writers
        .into_iter()
        .filter_map(|(name, level)| {
            source_item(&layout_path(&level.id), name, &merged[name])
                .map(|item| (name.to_string(), item))
        })
        .collect()
}

/// Everything the files of `library` make the shell do, as resolution weighs each: every action item of every layout and komponent, and every source as the `extends` chain of each layout that declares it leaves it — so one file's source is two items where two chains give it two promises.
pub fn library_items(library: &Library) -> BTreeSet<Item> {
    let trust = &library.trust;
    let mut items = BTreeSet::new();
    for layout in library.layouts.values() {
        let chain = chain_of(layout, library, &mut Report::default());
        items.extend(source_items(&chain).into_values());
        items.extend(
            layout_items(layout, trust)
                .into_iter()
                .filter(|item| item.kind == ItemKind::Action),
        );
    }
    for (id, komponent) in &library.komponents {
        items.extend(komponent_items(id, komponent, trust));
    }
    items
}

/// A text a bundle brought that an edit would copy into a file of the user's own, where nothing would hold it, with the bundle's item it is held as.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Carried {
    /// The file the edit writes.
    pub into: String,
    pub bundle: String,
    pub item: Item,
}

impl Carried {
    /// Why the edit is refused, in no language yet.
    pub fn message(&self) -> Message {
        util::message!(
            "finding.trust_carried",
            what = self.item.what(),
            bundle = &self.bundle,
            file = &self.into,
            id = self.item.id()
        )
    }
}

/// What an edit that turns the file `into` from running `before` into running `after` would take from a bundle without the user's trust: the first text `after` runs and `before` did not that a bundle's file runs held — pending or declined there, and accepted nowhere.
///
/// Trust follows the text, not the file. A bundle's file is held item by item wherever a text moves inside it, so an edit there needs no check; a file of the user's own runs whatever it holds, so an edit copying a held line, command or address into one — an instance moved out of an imported area, a group saved as a komponent, a komponent detached — would make the bundle's text the user's without the user ever trusting it. A text that file already ran is the user's, and stays so.
///
/// A text is judged with the promise it runs under: a command the user's file ran off the lock screen is not one it ran on it, so a copy that starts saying `lock_safe` is a new text as much as a changed command is.
pub fn carried(library: &Library, into: &str, before: &[Item], after: &[Item]) -> Option<Carried> {
    let trust = &library.trust;
    if trust.is_empty() || trust.is_bundled(into) {
        return None;
    }
    let had: BTreeSet<(&str, bool)> = before.iter().map(promised).collect();
    let new: BTreeSet<(&str, bool)> = after
        .iter()
        .map(promised)
        .filter(|pair| !had.contains(pair))
        .collect();
    if new.is_empty() {
        return None;
    }
    let bundled: Vec<(Item, Verdict, String)> = library_items(library)
        .into_iter()
        .filter(|item| new.contains(&promised(item)))
        .filter_map(|item| {
            let bundle = match trust.contesting(&item.file) {
                Some(all) => all.iter().cloned().collect::<Vec<_>>().join(", "),
                None => trust.bundle_of(&item.file)?.to_string(),
            };
            let verdict = trust.verdict(&item);
            Some((item, verdict, bundle))
        })
        .collect();
    let accepted: BTreeSet<(String, bool)> = bundled
        .iter()
        .filter(|(_, verdict, _)| *verdict == Verdict::Accepted)
        .map(|(item, _, _)| (item.text.clone(), item.lock_safe))
        .collect();
    bundled
        .into_iter()
        .find(|(item, verdict, _)| {
            !verdict.runs() && !accepted.contains(&(item.text.clone(), item.lock_safe))
        })
        .map(|(item, _, bundle)| Carried {
            into: into.to_string(),
            bundle,
            item,
        })
}

/// What an item runs and the promise it runs it under: what [`carried`] weighs.
fn promised(item: &Item) -> (&str, bool) {
    (item.text.as_str(), item.lock_safe)
}

/// Everything `layout` would run that is held, each where it is written: a source of its chain, an action item of a level of its chain, or one of a komponent it draws. A warning each, saying how to trust it; an error for each file two bundles claim.
pub fn held(layout: &Layout, library: &Library) -> Report {
    let mut report = Report::default();
    let trust = &library.trust;
    if trust.is_empty() {
        return report;
    }
    let chain = chain_of(layout, library, &mut Report::default());
    let mut items: Vec<Item> = source_items(&chain).into_values().collect();
    for level in &chain {
        items.extend(
            layout_items(level, trust)
                .into_iter()
                .filter(|item| item.kind == ItemKind::Action),
        );
    }
    let mut files: BTreeSet<String> = chain.iter().map(|level| layout_path(&level.id)).collect();
    for id in crate::components::komponents_of(layout, library) {
        files.insert(komponent_path(&id));
        if let Some(komponent) = library.komponent(&id) {
            items.extend(komponent_items(&id, komponent, trust));
        }
    }
    for file in files {
        if let Some(all) = trust.contesting(&file) {
            let bundles: Vec<&str> = all.iter().map(String::as_str).collect();
            report.error(Finding::new(
                file.clone(),
                "",
                util::message!(
                    "finding.trust_contested",
                    file = &file,
                    bundles = bundles.join(", ")
                ),
            ));
        }
    }
    for item in items {
        let Some(bundle) = trust.bundle_of(&item.file) else {
            continue;
        };
        let message = match trust.verdict(&item) {
            Verdict::Pending => util::message!(
                "finding.trust_pending",
                what = item.what(),
                bundle = bundle,
                id = item.id()
            ),
            Verdict::Declined => util::message!(
                "finding.trust_declined",
                what = item.what(),
                bundle = bundle,
                id = item.id()
            ),
            Verdict::Own | Verdict::Accepted => continue,
        };
        report.warn(Finding::new(item.file.clone(), item.key.clone(), message));
    }
    report
}

/// Every line of `chains` that grants trust, with the key it is written at: what no write may add, whoever's file it is.
pub(crate) fn granting(chains: Vec<(String, &Action)>) -> BTreeSet<(String, String)> {
    chains
        .into_iter()
        .flat_map(|(key, action)| {
            action
                .0
                .iter()
                .filter(|line| grants_trust(line))
                .map(move |line| (key.clone(), line.clone()))
                .collect::<Vec<_>>()
        })
        .collect()
}

/// Every action chain `layout` writes, with the key it is written at.
pub(crate) fn layout_chains(layout: &Layout) -> Vec<(String, &Action)> {
    let mut chains = Vec::new();
    for (site, layer) in sites(layout) {
        for area in &layer.areas {
            let path = format!("{site}.areas.{}", area.id);
            for (trigger, action) in &area.actions {
                chains.push((format!("{path}.actions.{}", trigger.as_str()), action));
            }
            for group in &area.groups {
                for child in &group.children {
                    let at = format!("{path}.groups.{}.children.{}", group.id, child.id);
                    for (trigger, action) in &child.actions {
                        chains.push((format!("{at}.actions.{}", trigger.as_str()), action));
                    }
                }
            }
        }
    }
    chains
}

/// [`layout_chains`], to change.
fn layout_chains_mut(layout: &mut Layout) -> Vec<(String, &mut Action)> {
    let mut chains = Vec::new();
    for (site, layer) in sites_mut(layout) {
        for area in &mut layer.areas {
            let path = format!("{site}.areas.{}", area.id);
            for (trigger, action) in &mut area.actions {
                chains.push((format!("{path}.actions.{}", trigger.as_str()), action));
            }
            for group in &mut area.groups {
                for child in &mut group.children {
                    let at = format!("{path}.groups.{}.children.{}", group.id, child.id);
                    for (trigger, action) in &mut child.actions {
                        chains.push((format!("{at}.actions.{}", trigger.as_str()), action));
                    }
                }
            }
        }
    }
    chains
}

/// Every action chain the komponent `komponent`'s children write, with the key it is written at.
pub(crate) fn komponent_chains(komponent: &Komponent) -> Vec<(String, &Action)> {
    komponent
        .children
        .iter()
        .flat_map(|child| {
            child.actions.iter().map(move |(trigger, action)| {
                (
                    format!("children.{}.actions.{}", child.id, trigger.as_str()),
                    action,
                )
            })
        })
        .collect()
}
