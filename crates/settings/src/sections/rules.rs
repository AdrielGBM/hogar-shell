//! The `[[rules]]` `config.toml` writes, as the running shell holds them: each one's trigger, whether it is loaded, and when it last fired.
//!
//! Readings, not fields: a rule is a few expressions and command lines, written in the file where `config check` can place a mistake at the character, so the form has no Save. Built in Rust rather than `.rsx` because its rows are a list whose length the config decides.

use automation::rules::{self, Listed, State};
use telar::{LayoutError, LayoutItem};

/// The rules section: one reading per rule, or one line saying where rules are written when there are none.
pub(crate) fn rules_section() -> Result<Box<dyn LayoutItem>, LayoutError> {
    let listed = rules::shell().list();
    let rows: Vec<Box<dyn LayoutItem>> = match listed.is_empty() {
        true => vec![reading(
            telar::t!("settings.rules.none_label"),
            telar::t!("settings.rules.none"),
        )?],
        false => listed
            .into_iter()
            .map(|rule| {
                let detail = detail(&rule);
                reading(rule.id, detail)
            })
            .collect::<Result<_, _>>()?,
    };
    crate::form_section::form_section(
        crate::form_section::FormSectionProps::props()
            .title(telar::Reactive::of(|| telar::t!("settings.section.rules")))
            .build(),
        telar::Children::new({
            let rows = std::cell::RefCell::new(Some(rows));
            move || {
                let mut slots = telar::Slots::new();
                let built = rows
                    .borrow_mut()
                    .take()
                    .ok_or_else(|| LayoutError::Engine("rule rows built twice".into()))?;
                for row in built {
                    slots.push(None, row);
                }
                Ok(slots)
            }
        }),
    )
}

/// `edge $battery.level < 15 · on · last fired 2026-10-01 07:30:00`.
fn detail(rule: &Listed) -> String {
    let state = match rule.state {
        State::On => telar::t!("settings.rules.on"),
        State::Off => telar::t!("settings.rules.off"),
        State::Invalid => telar::t!("settings.rules.invalid"),
        State::Suspended => telar::t!("settings.rules.suspended"),
    };
    let fired = match rule.fired_at() {
        Some(at) => telar::t!("settings.rules.fired", at = at),
        None => telar::t!("settings.rules.never"),
    };
    format!("{} · {state} · {fired}", rule.trigger)
}

fn reading(label: String, value: String) -> Result<Box<dyn LayoutItem>, LayoutError> {
    crate::reading_row::reading_row(
        crate::reading_row::ReadingRowProps::props()
            .label(telar::Reactive::of(move || label.clone()))
            .value(telar::Reactive::of(move || value.clone()))
            .build(),
        telar::Children::default(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    use config::theme::NordTheme;
    use telar::{reset_layout_runtime, set_theme};

    #[test]
    fn every_state_reads_as_a_sentence() {
        for state in [State::On, State::Off, State::Invalid, State::Suspended] {
            reset_layout_runtime();
            set_theme(NordTheme::new());
            let rule = Listed {
                id: "low-battery".to_string(),
                trigger: "edge $battery.level < 15".to_string(),
                state,
                last_fired: None,
            };
            let detail = detail(&rule);
            assert!(
                detail.starts_with("edge $battery.level < 15 · "),
                "{detail}"
            );
            assert!(reading(rule.id.clone(), detail).is_ok());
        }
    }
}
