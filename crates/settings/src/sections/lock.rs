//! The session lock and the idle stages that lead to it.
//!
//! What is left here is the forms this area cannot say in `.rsx`: the ones whose rows are a list the machine
//! decides the length of. The static-shape forms are `.rsx` components beside this file.

use std::rc::Rc;

use telar::{Container, LayoutError, LayoutItem, LayoutStyle};

use crate::form::*;
use crate::table::*;
use config::theme::NordTheme;
use config::{IdleConfig, IdleStage};

/// The `[[idle.stages]]` editor. `hogar-shell --list` is what the action fields accept; the placeholders name
/// the three a user reaches for.
pub(crate) fn idle_stages_section() -> Result<Box<dyn LayoutItem>, LayoutError> {
    let (config, path) = crate::form::source();
    let theme = telar::use_theme::<NordTheme>();
    let list = Rc::new(TableList::new(config.idle.stages.clone()));

    let rows = {
        let list = Rc::clone(&list);
        let handle = Rc::clone(&list);
        handle.view(move |id| {
            let Some(stage) = list.get(id) else {
                return Ok(Box::new(Container::new(LayoutStyle::new(), vec![])?));
            };
            let (timeout, a) = bound_field(
                || telar::t!("settings.field.timeout_seconds"),
                &list,
                id,
                stage.timeout.to_string(),
                "300",
                theme,
                |entry: &mut IdleStage, text| entry.timeout = parse_u64(text, entry.timeout),
            )?;
            let (action, b) = bound_field(
                || telar::t!("settings.field.action"),
                &list,
                id,
                stage.action.clone(),
                "lock on",
                theme,
                |entry: &mut IdleStage, text| entry.action = text.to_string(),
            )?;
            let (return_action, c) = bound_field(
                || telar::t!("settings.field.return_action"),
                &list,
                id,
                stage.return_action.clone(),
                "shell dpms on",
                theme,
                |entry: &mut IdleStage, text| entry.return_action = text.to_string(),
            )?;
            entry_card(
                vec![timeout, action, return_action],
                &list,
                id,
                theme,
                vec![a, b, c],
            )
        })?
    };

    let add = {
        let list = Rc::clone(&list);
        save_button(
            || telar::t!("settings.list.add"),
            move || list.add(IdleStage::default()),
        )?
    };

    let path = path.to_path_buf();
    let saved = Rc::clone(&list);
    let save = save_button(
        || telar::t!("settings.save.idle_stages"),
        move || {
            persist_with(&path, "idle", |current| IdleConfig {
                stages: saved.collect(),
                ..current.idle.clone()
            });
        },
    )?;

    section(
        || telar::t!("settings.section.idle_stages"),
        vec![rows, add],
        save,
        theme,
    )
}
