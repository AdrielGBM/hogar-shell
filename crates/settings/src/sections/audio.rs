//! Volume, the mixer, the visualiser and what is playing.
//!
//! What is left here is the forms this area cannot say in `.rsx`: the ones whose rows are a list the machine decides the length of. The static-shape forms are `.rsx` components beside this file.

use telar::{LayoutError, LayoutItem, LayoutStyle, RwSignal, Text, box_item, signal};

use crate::form::*;
use config::MediaConfig;
use config::theme::{FontRole, NordTheme};

/// Every media player a `[media.aliases]` row should exist for: the ones seen on the bus this session, plus any the config already renames. Both halves matter, for the reason `monitor_keys` documents.
fn player_keys(configured: &std::collections::HashMap<String, String>) -> Vec<String> {
    let mut keys: Vec<String> = configured.keys().cloned().collect();
    if let Some(player) = services::mpris::current()
        && !player.identity.trim().is_empty()
        && !keys.contains(&player.identity)
    {
        keys.push(player.identity.clone());
    }
    keys.sort_unstable();
    keys
}

pub(crate) fn media_aliases_section() -> Result<Box<dyn LayoutItem>, LayoutError> {
    let (config, path) = crate::form::source();
    let theme = telar::use_theme::<NordTheme>();
    let keys = player_keys(&config.media.aliases);
    let fields: Vec<(String, RwSignal<String>)> = keys
        .iter()
        .map(|key| {
            (
                key.clone(),
                signal(config.media.aliases.get(key).cloned().unwrap_or_default()),
            )
        })
        .collect();

    let mut rows: Vec<Box<dyn LayoutItem>> = Vec::with_capacity(fields.len().max(1));
    if fields.is_empty() {
        rows.push(box_item(Text::auto(
            || telar::t!("settings.media.no_players"),
            LayoutStyle::new(),
            move || theme.text_style(FontRole::Caption, theme.muted),
        )?));
    }
    for (key, value) in &fields {
        let label = key.clone();
        rows.push(text_field(
            move || label.clone(),
            value.clone(),
            key,
            theme,
        )?);
    }

    let path = path.to_path_buf();
    let save = save_button(
        || telar::t!("settings.save.media_aliases"),
        move || {
            let aliases: std::collections::HashMap<String, String> = fields
                .iter()
                .filter_map(|(key, value)| {
                    opt_string(&value.peek()).map(|alias| (key.clone(), alias))
                })
                .collect();
            persist_with(&path, "media", |current| MediaConfig {
                aliases,
                ..current.media.clone()
            });
        },
    )?;
    section(
        || telar::t!("settings.section.media_aliases"),
        rows,
        save,
        theme,
    )
}
