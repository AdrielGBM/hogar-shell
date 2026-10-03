//! What is failing as the shell runs, and why: a source that cannot answer, a layout expression that cannot be evaluated where it is drawn, a rule that did not finish. One registry, keyed by what failed; [`subscribe`] follows it as findings for the problems notice.

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, PoisonError};

use platform_wayland::EventSender;
use util::broadcast::Store;
use util::report::{Finding, Message, Report};

use crate::sources::{self, SourceSpec};

/// What failed.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Site {
    /// Reported under every name the running layout gives the spec, and not at all while none does.
    Source(SourceSpec),
    Expression(Drawn),
    /// A rule of `file`, by its place among the rules and its id.
    Rule {
        file: PathBuf,
        index: usize,
        id: String,
    },
}

/// A layout expression where one output draws it. Every output drawing the same expression fails and recovers on its own; the finding names the expression once.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Drawn {
    pub file: String,
    /// The key `layout check` names the same expression by.
    pub key: String,
    pub output: Option<String>,
    /// Which copy of a repeated child this is; `key` names the child the copies are made from.
    pub copy: Option<usize>,
}

static FAILING: Mutex<BTreeMap<Site, Message>> = Mutex::new(BTreeMap::new());

static REPORT: Store<Report> = Store::new(Report::default);

fn failing() -> MutexGuard<'static, BTreeMap<Site, Message>> {
    FAILING.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Records why `site` failed, until it recovers.
pub fn fail(site: Site, why: Message) {
    change(|failing| {
        let changed = failing.get(&site) != Some(&why);
        if changed {
            tracing::debug!("{site:?}: {}", why.english());
            failing.insert(site, why);
        }
        changed
    });
}

/// Records that `site` works again, or is gone.
pub fn recover(site: &Site) {
    change(|failing| failing.remove(site).is_some());
}

/// Keeps only the failures `keep` answers `true` for, and names every source again under what the running layout declares now.
pub fn retain(keep: impl Fn(&Site) -> bool) {
    change(|failing| {
        failing.retain(|site, _| keep(site));
        true
    });
}

/// Moves each failure to the site `to` answers for it, or forgets it where that is `None`.
pub fn refile(to: impl Fn(&Site) -> Option<Site>) {
    change(|failing| {
        let before = std::mem::take(failing);
        let changed = before
            .iter()
            .any(|(site, _)| to(site).as_ref() != Some(site));
        failing.extend(
            before
                .into_iter()
                .filter_map(|(site, why)| to(&site).map(|site| (site, why))),
        );
        changed
    });
}

/// Every failure now, as findings.
pub fn report() -> Report {
    REPORT.get()
}

/// Registers `tx` for [`report`] whenever it changes, sending the current one at once.
pub fn subscribe(tx: EventSender<Report>) {
    REPORT.subscribe(tx);
}

fn change(edit: impl FnOnce(&mut BTreeMap<Site, Message>) -> bool) {
    let mut failing = failing();
    if !edit(&mut failing) {
        return;
    }
    // Published under the lock, so two writers on different threads cannot publish their reports in the opposite order to their edits.
    let now = render(&failing);
    if REPORT.get() != now {
        REPORT.update(|report| *report = now);
    }
}

fn render(failing: &BTreeMap<Site, Message>) -> Report {
    let declared = sources::declared_sources();
    let mut report = Report::default();
    let mut said = HashSet::new();
    let mut say = |file: PathBuf, key: String, message: Message| {
        if said.insert((file.clone(), key.clone(), message.clone())) {
            report.error(Finding::new(file, key, message));
        }
    };
    for (site, why) in failing {
        match site {
            Site::Source(spec) => {
                for (name, _) in declared.iter().filter(|(_, source)| source.spec == *spec) {
                    say(
                        declared.file().into(),
                        format!("sources.{name}"),
                        util::message!("finding.source_failed", name = name, why = why),
                    );
                }
            }
            Site::Expression(drawn) => say(
                drawn.file.clone().into(),
                drawn.key.clone(),
                match drawn.copy {
                    Some(copy) => util::message!("finding.copy_failed", copy = copy, why = why),
                    None => why.clone(),
                },
            ),
            Site::Rule { file, index, id } => say(
                file.clone(),
                format!("rules[{index}]"),
                util::message!("finding.rule_failed", id = id, why = why),
            ),
        }
    }
    report
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;

    fn drawn(file: &str, output: &str, copy: Option<usize>) -> Site {
        Site::Expression(Drawn {
            file: file.to_string(),
            key: "outputs.*.layers.top.areas.bar.visible".to_string(),
            output: Some(output.to_string()),
            copy,
        })
    }

    fn division_by_zero() -> Message {
        Message::expression(&telar_expression::ErrorCode::DivisionByZero)
    }

    fn in_file(file: &str) -> Vec<(String, String)> {
        report()
            .findings()
            .filter(|finding| finding.file == std::path::Path::new(file))
            .map(|finding| (finding.key.clone(), finding.message.english()))
            .collect()
    }

    #[test]
    fn two_outputs_fail_and_recover_on_their_own_and_are_said_once() {
        let file = "layouts/failures-outputs.toml";
        let (left, right) = (drawn(file, "DP-1", None), drawn(file, "DP-2", None));
        fail(left.clone(), division_by_zero());
        fail(right.clone(), division_by_zero());
        assert_eq!(
            in_file(file),
            [(
                "outputs.*.layers.top.areas.bar.visible".to_string(),
                "division by zero".to_string()
            )],
            "one line for the expression however many outputs draw it"
        );

        recover(&left);
        assert_eq!(
            in_file(file).len(),
            1,
            "the other output still fails, so the expression still does"
        );
        recover(&right);
        assert!(in_file(file).is_empty());
    }

    #[test]
    fn a_copy_is_reported_at_the_child_it_is_made_from() {
        let file = "layouts/failures-copies.toml";
        fail(drawn(file, "DP-1", Some(2)), division_by_zero());
        fail(drawn(file, "DP-1", Some(3)), division_by_zero());
        assert_eq!(
            in_file(file),
            [
                (
                    "outputs.*.layers.top.areas.bar.visible".to_string(),
                    "copy #2: division by zero".to_string()
                ),
                (
                    "outputs.*.layers.top.areas.bar.visible".to_string(),
                    "copy #3: division by zero".to_string()
                ),
            ]
        );
        recover(&drawn(file, "DP-1", Some(2)));
        assert_eq!(
            in_file(file).len(),
            1,
            "one copy recovering leaves the other"
        );
        recover(&drawn(file, "DP-1", Some(3)));
    }

    #[test]
    fn a_subscriber_hears_each_change_and_nothing_else() {
        let file = "layouts/failures-subscriber.toml";
        let (tx, heard) = platform_wayland::detached::<Report>();
        subscribe(tx);
        let says = |want: bool| {
            let deadline = Instant::now() + Duration::from_secs(2);
            while Instant::now() < deadline {
                if let Some(report) = heard.try_recv()
                    && report
                        .findings()
                        .any(|finding| finding.file == std::path::Path::new(file))
                        == want
                {
                    return true;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            false
        };
        let site = drawn(file, "DP-1", None);
        fail(site.clone(), division_by_zero());
        assert!(says(true), "the failure is published as it happens");
        recover(&site);
        assert!(says(false), "and so is the recovery");
    }

    #[test]
    fn a_rule_moved_in_the_file_keeps_its_failure_at_its_new_place() {
        let file = PathBuf::from("failures-rules.toml");
        let at = |index| Site::Rule {
            file: file.clone(),
            index,
            id: "low".to_string(),
        };
        fail(at(0), division_by_zero());
        refile(|site| match site {
            Site::Rule { file: written, .. } if *written == file => Some(at(3)),
            other => Some(other.clone()),
        });
        let keys: Vec<String> = report()
            .findings()
            .filter(|finding| finding.file == file)
            .map(|finding| finding.key.clone())
            .collect();
        assert_eq!(keys, ["rules[3]"]);
        recover(&at(3));
    }
}
