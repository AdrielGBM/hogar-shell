//! The undo history as a list to jump through (T-4.12), the same on the strip and in the context menu: "At the start", then every entry an undo would take back, the oldest first, then every entry a redo would put back, dimmed. The one the layout is at now is marked, and each says how many steps away it is; picking one walks there through [`session::travel`], one ordinary undo or redo a step.

use telar::MenuEntry;

use layout::{History, LayoutStore};
use surfaces::layouts;

use crate::session;

/// What marks the entry the layout is at now.
const NOW: &str = "● ";

/// One line of the history list.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Line {
    pub label: String,
    /// "now", "n back" or "n forward".
    pub hint: String,
    pub now: bool,
    /// An entry a redo would put back, which is drawn dimmed.
    pub ahead: bool,
    /// How far picking it walks: back while negative, forward while positive.
    pub steps: isize,
}

impl Line {
    /// The label with the mark of the entry the layout is at now in front of it.
    pub fn marked(&self) -> String {
        crate::mode::marked(self.now, NOW, &self.label)
    }

    /// Picks the line: walks the history to it, saying where it landed.
    pub fn pick(&self) {
        session::travel_saying(self.steps);
    }
}

/// The history of the running shell's store as the list shows it; empty outside the shell.
pub fn lines() -> Vec<Line> {
    lines_of(&current())
}

/// The history of the running shell's store; empty outside the shell.
pub fn current() -> History {
    layouts::read(LayoutStore::history).unwrap_or_default()
}

/// `history` as the list shows it.
pub fn lines_of(history: &History) -> Vec<Line> {
    let back = history.undo.len() as isize;
    let at = |steps: isize, label: String, ahead: bool| Line {
        label,
        hint: hint(steps),
        now: steps == 0,
        ahead,
        steps,
    };
    let mut lines = vec![at(-back, telar::t!("editor.history.start"), false)];
    lines.extend(
        history
            .undo
            .iter()
            .enumerate()
            .map(|(index, label)| at(index as isize + 1 - back, label.clone(), false)),
    );
    lines.extend(
        history
            .redo
            .iter()
            .enumerate()
            .map(|(index, label)| at(index as isize + 1, label.clone(), true)),
    );
    lines
}

fn hint(steps: isize) -> String {
    match steps {
        0 => telar::t!("editor.history.now"),
        back if back < 0 => telar::t!("editor.history.back", steps = back.unsigned_abs()),
        ahead => telar::t!("editor.history.forward", steps = ahead),
    }
}

/// The list as context menu rows, for a "History" submenu; `None` while there is nothing to walk through.
pub(crate) fn menu(history: &History) -> Option<MenuEntry> {
    if history.is_empty() {
        return None;
    }
    Some(MenuEntry::Sub {
        label: telar::t!("editor.history.title"),
        entries: lines_of(history).into_iter().map(entry).collect(),
    })
}

fn entry(line: Line) -> MenuEntry {
    let label = line.marked();
    let hint = line.hint.clone();
    match line.ahead {
        false => MenuEntry::row(label, hint, move || line.pick()),
        true => crate::context::dimmed_row(label, hint, move || line.pick()),
    }
}
