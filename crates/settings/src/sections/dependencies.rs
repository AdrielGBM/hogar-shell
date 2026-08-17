//! What the shell reaches for outside itself, and whether this machine has it.
//!
//! Readings, not fields — nothing here is a setting, so the form has no Save. It is the one page that answers
//! "why is that module empty" without leaving the shell, which is why every row says what its absence costs
//! rather than only whether it is there.
//!
//! Built in Rust rather than `.rsx` for the reason the applications list is: its rows are a list whose length
//! the registry decides, and the value each shows arrives from a worker thread rather than from the config.

use telar::{LayoutError, LayoutItem, ReactiveList, box_item, signal};

use util::deps::{self, Need, Presence, Status};

/// The dependency report.
///
/// Probing is a process start and a bus round trip per row, so it cannot happen while this is being built:
/// [`deps::report`] does it on a thread of its own and the page fills in when the answers arrive. Empty until
/// then, which is honest — a row that has not been probed has nothing to say.
pub(crate) fn dependencies_section() -> Result<Box<dyn LayoutItem>, LayoutError> {
    let statuses = signal(Vec::<Status>::new());
    let read = statuses.read_only();
    platform_wayland::watch(deps::report, move |report: Vec<Status>| {
        statuses.set(report)
    });

    // Keyed on the dependency, never on the position: the list is [`deps::ALL`] in order, and keying on an
    // index would reuse one row's widgets for another the moment the report arrives and the list stops being
    // empty.
    let rows = ReactiveList::new(
        move || read.get(),
        |status: &Status| deps::entry(status.dep).id,
        row,
        0.0,
    )?;

    crate::form_section(
        crate::FormSectionProps {
            title: Box::new(|| telar::t!("settings.section.dependencies")),
        },
        {
            let mut slots = telar::Slots::new();
            slots.push(None, box_item(rows));
            slots
        },
    )
}

/// One dependency: what it is for, and — when it is missing — what that costs.
fn row(status: Status) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let entry = deps::entry(status.dep);
    let id = entry.id.to_string();
    let detail = match status.presence {
        Presence::Present => entry.what.to_string(),
        Presence::Absent => entry.without.to_string(),
        Presence::Unknown => telar::t!("settings.deps.unknown"),
    };
    let mark = mark_for(status).to_string();
    crate::reading_row(crate::ReadingRowProps {
        label: Box::new(move || format!("{mark}  {id}")),
        value: Box::new(move || detail.clone()),
    })
}

/// The glyphless mark in front of the name. Deliberately words rather than colour alone: this page is read by
/// someone trying to find out why something does not work, and a colour is not an answer.
fn mark_for(status: Status) -> &'static str {
    match (status.presence, deps::entry(status.dep).need) {
        (Presence::Present, _) => "ok",
        (Presence::Absent, Need::Required) => "MISSING",
        (Presence::Absent, Need::Optional) => "absent",
        (Presence::Unknown, _) => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use config::theme::NordTheme;
    use telar::{reset_layout_runtime, set_theme};

    /// Every row reads as a sentence about *this* machine, whichever way the probe went — including the third
    /// answer, which is the one a naive `bool` would have turned into a false accusation.
    ///
    /// The section as a whole is built by `every_section_on_every_page_builds`, which walks the page table;
    /// what that cannot reach is a row for a presence this machine does not happen to produce.
    #[test]
    fn every_presence_has_something_to_say() {
        for presence in [Presence::Present, Presence::Absent, Presence::Unknown] {
            for entry in deps::ALL {
                reset_layout_runtime();
                set_theme(NordTheme::new());
                let status = Status {
                    dep: entry.dep,
                    presence,
                };
                assert!(!mark_for(status).is_empty());
                assert!(row(status).is_ok(), "{} at {presence:?}", entry.id);
            }
        }
    }
}
