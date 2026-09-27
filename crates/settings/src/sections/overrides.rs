//! Every module's own presentation, and the battery warnings: the forms whose rows are a list the machine decides the length of. Where a chip is placed is the layout's.

use std::rc::Rc;
use ui::descriptor::ModuleDescriptor;

use telar::{Container, LayoutError, LayoutItem, LayoutStyle, RwSignal, signal};

use crate::form::*;
use crate::table::*;
use config::theme::NordTheme;
use config::{BatteryConfig, BatteryWarning, ModuleOverride, OpenMode, Variant};

/// Every module in the installed table a bar can place, which is every one with a chip, sorted by id.
fn bar_modules() -> Vec<&'static ModuleDescriptor> {
    let mut modules: Vec<&'static ModuleDescriptor> = ui::descriptor::installed()
        .iter()
        .filter(|module| module.representations.chip.is_some())
        .collect();
    modules.sort_unstable_by_key(|module| module.id);
    modules
}

/// `[modules.<id>]`: the per-module presentation overrides.
///
/// Keyed on the registry rather than on what the bars currently use, so a module can be styled before it is placed — the alternative would be a user having to add a chip, save, reopen the page and only then be able to give it an accent.
pub(crate) fn module_overrides_section() -> Result<Box<dyn LayoutItem>, LayoutError> {
    let (config, path) = crate::form::source();
    let theme = telar::use_theme::<NordTheme>();
    let mut ids: Vec<String> = bar_modules()
        .iter()
        .map(|module| module.id.to_string())
        .collect();
    for configured in config.modules.keys() {
        if !ids.contains(configured) {
            ids.push(configured.clone());
        }
    }
    ids.sort_unstable();

    struct Fields {
        id: String,
        variant: RwSignal<String>,
        accent: RwSignal<String>,
        open: RwSignal<String>,
        width: RwSignal<String>,
        height: RwSignal<String>,
    }

    let mut fields: Vec<Fields> = Vec::with_capacity(ids.len());
    let mut rows: Vec<Box<dyn LayoutItem>> = Vec::with_capacity(ids.len() * 6);
    for id in ids {
        let existing = config.modules.get(&id).cloned().unwrap_or_default();
        let entry = Fields {
            variant: signal(variant_str(existing.variant).to_string()),
            accent: signal(existing.accent.clone().unwrap_or_default()),
            open: signal(open_mode_str(existing.open).to_string()),
            width: signal(opt_num(existing.width)),
            height: signal(opt_num(existing.height)),
            id: id.clone(),
        };
        rows.push(subheader(move || id.clone(), theme)?);
        rows.push(enum_field(
            || telar::t!("settings.field.variant_style"),
            entry.variant,
            VARIANT_STYLES,
            theme,
        )?);
        rows.push(text_field(
            || telar::t!("settings.field.accent"),
            entry.accent,
            "(theme)",
            theme,
        )?);
        rows.push(enum_field(
            || telar::t!("settings.field.open"),
            entry.open,
            OPEN_MODES,
            theme,
        )?);
        rows.push(text_field(
            || telar::t!("settings.field.width"),
            entry.width,
            "(panels)",
            theme,
        )?);
        rows.push(text_field(
            || telar::t!("settings.field.height"),
            entry.height,
            "(panels)",
            theme,
        )?);
        fields.push(entry);
    }

    let path = path.to_path_buf();
    let save = save_button(
        SaveButtonProps::props()
            .label(telar::Reactive::of(|| telar::t!("settings.save.modules")))
            .on_press(std::rc::Rc::new(move || {
                let overrides: std::collections::HashMap<String, ModuleOverride> = fields
                    .iter()
                    .filter_map(|entry| {
                        let value = ModuleOverride {
                            variant: parse_variant(&entry.variant.peek()),
                            accent: opt_string(&entry.accent.peek()),
                            open: parse_open_mode(&entry.open.peek()),
                            width: opt_u32(&entry.width.peek()),
                            height: opt_u32(&entry.height.peek()),
                        };
                        // A module left entirely at its defaults gets no table at all, so the file keeps only the overrides a user actually made rather than thirty empty sections.
                        if is_default_override(&value) {
                            None
                        } else {
                            Some((entry.id.clone(), value))
                        }
                    })
                    .collect();
                persist(&path, "modules", &overrides);
            }))
            .build(),
        telar::Children::default(),
    )?;
    section(|| telar::t!("settings.section.modules"), rows, save, theme)
}

fn is_default_override(value: &ModuleOverride) -> bool {
    value.variant == Variant::Default
        && value.accent.is_none()
        && value.open == OpenMode::default()
        && value.width.is_none()
        && value.height.is_none()
}

/// The `[[battery.warn_levels]]` editor: one card per warning, with Add and Remove.
pub(crate) fn battery_warnings_section() -> Result<Box<dyn LayoutItem>, LayoutError> {
    let (config, path) = crate::form::source();
    let theme = telar::use_theme::<NordTheme>();
    let list = Rc::new(TableList::new(config.battery.warn_levels.clone()));

    let rows = {
        let list = Rc::clone(&list);
        let handle = Rc::clone(&list);
        handle.view(move |id| {
            let Some(warning) = list.get(id) else {
                return Ok(Box::new(Container::new(LayoutStyle::new(), vec![])?));
            };
            let level = bound_field(
                || telar::t!("settings.field.level"),
                &list,
                id,
                warning.level.to_string(),
                "20",
                theme,
                |entry: &mut BatteryWarning, text| entry.level = parse_i32(text, entry.level),
            )?;
            let title = bound_field(
                || telar::t!("settings.field.title"),
                &list,
                id,
                warning.title.clone(),
                "(default)",
                theme,
                |entry: &mut BatteryWarning, text| entry.title = text.to_string(),
            )?;
            let message = bound_field(
                || telar::t!("settings.field.message"),
                &list,
                id,
                warning.message.clone(),
                "(default)",
                theme,
                |entry: &mut BatteryWarning, text| entry.message = text.to_string(),
            )?;
            let icon = bound_field(
                || telar::t!("settings.field.icon"),
                &list,
                id,
                warning.icon.clone(),
                "battery-low",
                theme,
                |entry: &mut BatteryWarning, text| entry.icon = text.to_string(),
            )?;
            let critical = bound_toggle(
                || telar::t!("settings.field.critical_urgency"),
                &list,
                id,
                warning.critical,
                theme,
                |entry: &mut BatteryWarning, on| entry.critical = on,
            )?;
            entry_card(
                vec![level, title, message, icon, critical],
                &list,
                id,
                theme,
            )
        })?
    };

    let add = {
        let list = Rc::clone(&list);
        save_button(
            SaveButtonProps::props()
                .label(telar::Reactive::of(|| telar::t!("settings.list.add")))
                .on_press(std::rc::Rc::new(move || {
                    list.add(BatteryWarning::default())
                }))
                .build(),
            telar::Children::default(),
        )?
    };

    let path = path.to_path_buf();
    let saved = Rc::clone(&list);
    let save = save_button(
        SaveButtonProps::props()
            .label(telar::Reactive::of(|| {
                telar::t!("settings.save.battery_warnings")
            }))
            .on_press(std::rc::Rc::new(move || {
                persist_with(&path, "battery", |current| BatteryConfig {
                    warn_levels: saved.collect(),
                    ..current.battery.clone()
                });
            }))
            .build(),
        telar::Children::default(),
    )?;

    section(
        || telar::t!("settings.section.battery_warnings"),
        vec![rows, add],
        save,
        theme,
    )
}
