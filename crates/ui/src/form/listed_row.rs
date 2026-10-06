use std::rc::Rc;

use telar::{Children, Reactive, RwSignal, Slots, use_theme};

use config::theme::NordTheme;

use crate::descriptor::Built;
use crate::form::labelled::labelled;
use crate::form::recorder::record_field;

/// A labelled dropdown over `options` decided as the form is built, each the value the file writes and what the list calls it: for choices that are not a model enum — the icon themes installed, a picture of the wallpaper library. A value none of them is shows as the first.
pub fn listed_row(
    label: Reactive<String>,
    value: RwSignal<String>,
    options: Rc<[(String, String)]>,
) -> Built {
    record_field(&value);
    let index_of = {
        let options = Rc::clone(&options);
        move |now: &str| {
            options
                .iter()
                .position(|(held, _)| held == now)
                .unwrap_or(0) as u32
        }
    };
    let picked = telar::signal(value.peek_with(|now| index_of(now)));
    telar::effect(move || {
        let at = value.with(|now| index_of(now));
        if picked.peek() != at {
            picked.set(at);
        }
    });
    let items = options
        .iter()
        .map(|(_, shown)| {
            telar::item(
                telar::ItemProps::props().label(shown.clone()).build(),
                Children::default(),
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut slots = Slots::new();
    slots.extend_default(items);
    let theme = use_theme::<NordTheme>();
    let choosing = Rc::clone(&options);
    let select = telar::select(
        telar::SelectProps::props()
            .selected(picked)
            .color(Reactive::of(move || theme.accent))
            .stretch(true)
            .on_select(Rc::new(move |at: u32| {
                if let Some((chosen, _)) = choosing.get(at as usize)
                    && value.peek_with(|now| now != chosen)
                {
                    value.set(chosen.clone());
                }
            }))
            .build(),
        Children::from(slots),
    )?;
    labelled(label, select)
}

/// `options` with `current` at the end when it is none of them, so a value the file says that the list does not offer is shown as itself rather than as the first option.
pub fn keeping_current(
    mut options: Vec<(String, String)>,
    current: &str,
) -> Rc<[(String, String)]> {
    if !options.iter().any(|(held, _)| held == current) {
        options.push((current.to_string(), current.to_string()));
    }
    Rc::from(options)
}
