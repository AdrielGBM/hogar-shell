//! Every bar, and every chip a bar can carry.
//!
//! What is left here is the forms this area cannot say in `.rsx`: the ones whose rows are a list the machine decides the length of. The static-shape forms are `.rsx` components beside this file.

use std::rc::Rc;
use ui::scale::space;

use telar::{
    AlignItems, Container, LayoutError, LayoutItem, LayoutStyle, ReactiveList, Rect, RectStyle,
    RwSignal, SizeDimension, StyledContainer, Text, box_item, signal,
};

use crate::form::*;
use crate::table::*;
use config::theme::{FontRole, NordTheme};
use config::{
    BarConfig, BarsConfig, BatteryConfig, BatteryWarning, ModuleEntry, ModuleOverride, OpenMode,
    Variant,
};

/// Every pill's laid-out box, keyed by `(zone, index)` — and by `(zone, ZONE_ROW)` for a zone's own row.
type PillRects =
    Rc<std::cell::RefCell<std::collections::HashMap<(usize, usize), telar::ReadSignal<Rect>>>>;

#[derive(Clone)]
struct BarSignals {
    size: RwSignal<String>,
    persistent: RwSignal<bool>,
    show_on_hover: RwSignal<bool>,
    peek: RwSignal<String>,
    zones: ZoneEditor,
}

fn bar_signals(bar: &BarConfig) -> BarSignals {
    BarSignals {
        size: signal(bar.size.to_string()),
        persistent: signal(bar.persistent),
        show_on_hover: signal(bar.show_on_hover),
        peek: signal(bar.peek.to_string()),
        zones: ZoneEditor::new(bar),
    }
}

/// K3: one bar's three zones, edited as draggable module pills.
///
/// What this replaces is three comma-separated text fields of desktop ids — a control that required knowing every module's spelling, gave no way to see what was available, and turned "put the clock on the other end" into two careful edits. A pill can be dragged anywhere in any of the three zones, dropped to reorder, and dismissed with its own ✕; the palette underneath is every module the shell registers.
///
/// The entries are carried whole rather than by id, which is what keeps `{ id = "clock", accent = "red" }` intact across a reorder — the thing the CSV field had to reconstruct by claiming entries by name.
#[derive(Clone)]
struct ZoneEditor {
    zones: [RwSignal<Vec<ModuleEntry>>; 3],
    /// Where each pill and each zone row was laid out. A drop is resolved against the pointer's actual position, so dragging a pill onto another zone's *empty* space works as well as onto a pill in it.
    rects: PillRects,
    /// Which zone the palette adds to, so pressing a module is one press rather than a press and a drag.
    target: RwSignal<usize>,
}

/// The key a zone row registers its own rect under — past any pill index it could ever hold.
const ZONE_ROW: usize = usize::MAX;

impl ZoneEditor {
    fn new(bar: &BarConfig) -> Self {
        Self {
            zones: [
                signal(bar.start.clone()),
                signal(bar.center.clone()),
                signal(bar.end.clone()),
            ],
            rects: Rc::new(std::cell::RefCell::new(std::collections::HashMap::new())),
            target: signal(0usize),
        }
    }

    fn entries(&self, zone: usize) -> Vec<ModuleEntry> {
        self.zones[zone].peek()
    }

    fn append(&self, zone: usize, entry: ModuleEntry) {
        let mut entries = self.zones[zone].peek();
        entries.push(entry);
        self.zones[zone].set(entries);
    }

    fn remove(&self, zone: usize, index: usize) {
        let mut entries = self.zones[zone].peek();
        if index < entries.len() {
            entries.remove(index);
            self.zones[zone].set(entries);
        }
    }

    /// Moves the pill at `(from_zone, from_index)` to `to_index` of `to_zone`.
    fn move_entry(&self, from: (usize, usize), to: (usize, usize)) {
        let mut source = self.zones[from.0].peek();
        if from.1 >= source.len() {
            return;
        }
        let entry = source.remove(from.1);
        if from.0 == to.0 {
            let index = to.1.min(source.len());
            source.insert(index, entry);
            self.zones[from.0].set(source);
            return;
        }
        let mut target = self.zones[to.0].peek();
        let index = to.1.min(target.len());
        target.insert(index, entry);
        self.zones[from.0].set(source);
        self.zones[to.0].set(target);
    }

    /// Where a drop at `point` (surface coordinates) lands: the pill under it, else the zone row it is over.
    fn drop_target(&self, point: (f32, f32)) -> Option<(usize, usize)> {
        // Read the three lengths once, and use them to ignore the rects of pills that are no longer there. A zone that went from three pills to two leaves `(zone, 2)` in the map pointing at a destroyed widget's rect, and nothing about that entry says so — it would go on winning drops over the area it used to occupy, ahead of whichever live pill the map happened to be walked to second.
        let lengths = [
            self.zones[0].peek().len(),
            self.zones[1].peek().len(),
            self.zones[2].peek().len(),
        ];
        let rects = self.rects.borrow();
        let mut row: Option<(usize, usize)> = None;
        for ((zone, index), rect) in rects.iter() {
            if *index != ZONE_ROW && *index >= lengths[*zone] {
                continue;
            }
            let rect = rect.get();
            if !rect.contains(point.0, point.1) {
                continue;
            }
            if *index == ZONE_ROW {
                // Held rather than returned: a pill's own rect is inside its row's, and the pill is the more precise answer whichever order the map happens to be walked in.
                row = Some((*zone, lengths[*zone]));
                continue;
            }
            // The half of the pill the pointer is on decides which side of it the dragged one lands.
            let after = point.0 > rect.x + rect.width / 2.0;
            return Some((*zone, index + usize::from(after)));
        }
        row
    }

    fn track(&self, zone: usize, index: usize, rect: telar::ReadSignal<Rect>) {
        self.rects.borrow_mut().insert((zone, index), rect);
    }
}

fn bar_rows(
    label: impl Fn() -> String + 'static,
    s: &BarSignals,
    theme: NordTheme,
) -> Result<Vec<Box<dyn LayoutItem>>, LayoutError> {
    let mut rows = vec![
        subheader(label, theme)?,
        text_field(|| telar::t!("settings.field.size"), s.size, "34", theme)?,
        toggle_field(
            || telar::t!("settings.field.persistent"),
            s.persistent,
            theme,
        )?,
        toggle_field(
            || telar::t!("settings.field.show_on_hover"),
            s.show_on_hover,
            theme,
        )?,
        text_field(|| telar::t!("settings.field.peek"), s.peek, "2", theme)?,
    ];
    for (zone, label) in ZONE_LABELS.iter().enumerate() {
        rows.push(zone_row(label, zone, &s.zones, theme)?);
    }
    rows.push(module_palette(&s.zones, theme)?);
    Ok(rows)
}

const ZONE_LABELS: [&str; 3] = ["start", "center", "end"];
const PILL_RADIUS: f32 = 8.0;

/// One zone: its name, and the pills in it.
fn zone_row(
    label: &'static str,
    zone: usize,
    editor: &ZoneEditor,
    theme: NordTheme,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let source = editor.zones[zone].read_only();
    let list_editor = editor.clone();
    let pills = ReactiveList::with_style(
        LayoutStyle::new()
            .flex_row()
            .flex_wrap()
            .gap(space::md())
            .flex_grow(1.0)
            .min_width(0.0),
        move || source.get().into_iter().enumerate().collect(),
        // Keyed on the position *and* the id: a reorder has to redraw both pills that swapped, and a list keyed on the id alone would leave them where they were.
        |(index, entry): &(usize, ModuleEntry)| format!("{index}|{}", entry.id),
        move |(index, entry): (usize, ModuleEntry)| {
            module_pill(zone, index, entry, list_editor.clone(), theme)
        },
    )?;

    let selected = editor.target.read_only();
    let choose = editor.target;
    let row = StyledContainer::new(
        LayoutStyle::new()
            .flex_row()
            .align_items(AlignItems::CENTER)
            .gap(space::md())
            .padding_all(space::md())
            .min_height(theme.font(FontRole::Body) * 2.4)
            .width(SizeDimension::Percent(1.0)),
        move |_r| {
            let fill = if selected.get() == zone {
                theme.overlay
            } else {
                theme.base
            };
            RectStyle::filled(fill, PILL_RADIUS)
        },
        vec![
            box_item(Text::new(
                move || crate::pages::label("settings.field", label),
                LayoutStyle::new().width(90.0).flex_shrink(0.0),
                move || theme.text_style(FontRole::Caption, theme.subtle),
            )?),
            Box::new(pills),
        ],
    )?
    .on_press(move || choose.set(zone));
    let rect = telar::track_layout(row.layout_node())
        .expect("a container registers its rect")
        .read_only();
    editor.track(zone, ZONE_ROW, rect);
    Ok(Box::new(row))
}

/// One module on a bar: its id, a ✕ that takes it off, and the drag that moves it.
fn module_pill(
    zone: usize,
    index: usize,
    entry: ModuleEntry,
    editor: ZoneEditor,
    theme: NordTheme,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let id = entry.id.clone();
    let label = Text::new(
        move || id.clone(),
        LayoutStyle::new(),
        move || theme.text_style(FontRole::Caption, theme.text),
    )?;
    let remove = {
        let editor = editor.clone();
        toggle_pill("x", false, theme.red, theme, move || {
            editor.remove(zone, index)
        })?
    };

    let pill = StyledContainer::new(
        LayoutStyle::new()
            .flex_row()
            .align_items(AlignItems::CENTER)
            .gap(space::sm())
            .padding_horizontal(space::md())
            .padding_vertical(space::sm())
            .flex_shrink(0.0),
        move |_r| RectStyle::filled(theme.surface, PILL_RADIUS),
        vec![box_item(label), remove],
    )?;
    let rect = telar::track_layout(pill.layout_node())
        .expect("a container registers its rect")
        .read_only();
    editor.track(zone, index, rect);

    let dropped = editor.clone();
    let pill = pill.on_drag_end(move |x, y| {
        // The gesture reports where the pointer is *inside the pill*; the drop is about where that is on the surface, so the pill's own origin has to be added back before anything can be hit-tested.
        let origin = rect.peek();
        let point = (origin.x + x, origin.y + y);
        if let Some(target) = dropped.drop_target(point) {
            dropped.move_entry((zone, index), target);
        }
    });
    Ok(Box::new(pill))
}

/// Every module the shell registers, as something to press. The add half of K3: the CSV field it replaces required knowing a module existed before it could be typed.
fn module_palette(
    editor: &ZoneEditor,
    theme: NordTheme,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let mut chips: Vec<Box<dyn LayoutItem>> = Vec::new();
    for id in ui::module::with_registry(|registry| registry.ids()) {
        let editor = editor.clone();
        let label = id.clone();
        let text = Text::new(
            move || label.clone(),
            LayoutStyle::new(),
            move || theme.text_style(FontRole::Caption, theme.subtle),
        )?;
        chips.push(Box::new(
            StyledContainer::new(
                LayoutStyle::new()
                    .padding_horizontal(space::md())
                    .padding_vertical(space::sm())
                    .flex_shrink(0.0),
                move |_r| RectStyle::filled(theme.base, PILL_RADIUS),
                vec![box_item(text)],
            )?
            .hover_style(move |_r| RectStyle::filled(theme.overlay, PILL_RADIUS))
            .on_press(move || {
                let zone = editor.target.peek();
                editor.append(zone, ModuleEntry::bare(id.clone()));
            }),
        ));
    }
    let grid = Container::new(
        LayoutStyle::new()
            .flex_row()
            .flex_wrap()
            .gap(space::md())
            .flex_grow(1.0)
            .min_width(0.0),
        chips,
    )?;
    labelled(
        || telar::t!("settings.field.add_module"),
        Box::new(grid),
        theme,
    )
}

fn bar_from(s: &BarSignals, base: &BarConfig) -> BarConfig {
    BarConfig {
        size: parse_u32(&s.size.peek(), base.size),
        start: s.zones.entries(0),
        center: s.zones.entries(1),
        end: s.zones.entries(2),
        shape: base.shape,
        persistent: s.persistent.peek(),
        show_on_hover: s.show_on_hover.peek(),
        peek: parse_u32(&s.peek.peek(), base.peek),
    }
}

pub(crate) fn bars_section() -> Result<Box<dyn LayoutItem>, LayoutError> {
    let (config, path) = crate::form::source();
    let theme = telar::use_theme::<NordTheme>();
    let bars = &config.bars;
    let top = bar_signals(&bars.top);
    let bottom = bar_signals(&bars.bottom);
    let left = bar_signals(&bars.left);
    let right = bar_signals(&bars.right);

    let mut rows = Vec::new();
    rows.extend(bar_rows(
        || telar::t!("settings.subheader.top"),
        &top,
        theme,
    )?);
    rows.extend(bar_rows(
        || telar::t!("settings.subheader.bottom"),
        &bottom,
        theme,
    )?);
    rows.extend(bar_rows(
        || telar::t!("settings.subheader.left"),
        &left,
        theme,
    )?);
    rows.extend(bar_rows(
        || telar::t!("settings.subheader.right"),
        &right,
        theme,
    )?);

    let base = bars.clone();
    let path = path.to_path_buf();
    let save = save_button(
        SaveButtonProps::props()
            .label(telar::Reactive::of(|| telar::t!("settings.save.bars")))
            .on_press(std::rc::Rc::new(move || {
            let value = BarsConfig {
                // Carried through unchanged: the panel edits the four zones, and rewriting the section must not drop a screen exclusion it has no field for.
                excluded_screens: base.excluded_screens.clone(),
                top: bar_from(&top, &base.top),
                bottom: bar_from(&bottom, &base.bottom),
                left: bar_from(&left, &base.left),
                right: bar_from(&right, &base.right),
            };
            persist(&path, "bars", &value);
        }))
            .build(),
        telar::Children::default(),
    )?;
    section(|| telar::t!("settings.section.bars"), rows, save, theme)
}

/// `[modules.<id>]`: the per-module presentation overrides.
///
/// Keyed on the registry rather than on what the bars currently use, so a module can be styled before it is placed — the alternative would be a user having to add a chip, save, reopen the page and only then be able to give it an accent.
pub(crate) fn module_overrides_section() -> Result<Box<dyn LayoutItem>, LayoutError> {
    let (config, path) = crate::form::source();
    let theme = telar::use_theme::<NordTheme>();
    let mut ids: Vec<String> = ui::module::with_registry(|registry| registry.ids());
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
                .on_press(std::rc::Rc::new(move || list.add(BatteryWarning::default())))
                .build(),
            telar::Children::default(),
        )?
    };

    let path = path.to_path_buf();
    let saved = Rc::clone(&list);
    let save = save_button(
        SaveButtonProps::props()
            .label(telar::Reactive::of(|| telar::t!("settings.save.battery_warnings")))
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dragging_a_module_carries_its_own_settings_with_it() {
        telar::reset_runtime();
        let accented = ModuleEntry {
            id: "clock".to_string(),
            accent: Some("red".to_string()),
            variant: None,
        };
        let filled = ModuleEntry {
            id: "clock".to_string(),
            variant: Some(config::Variant::Filled),
            accent: None,
        };
        let bar = BarConfig {
            start: vec![ModuleEntry::bare("workspaces"), accented.clone()],
            center: vec![filled.clone()],
            end: Vec::new(),
            ..BarConfig::default()
        };
        let editor = ZoneEditor::new(&bar);

        editor.move_entry((0, 1), (0, 0));
        assert_eq!(
            editor.entries(0),
            vec![accented.clone(), ModuleEntry::bare("workspaces")]
        );

        editor.move_entry((0, 0), (1, 1));
        assert_eq!(editor.entries(0), vec![ModuleEntry::bare("workspaces")]);
        assert_eq!(editor.entries(1), vec![filled, accented]);

        editor.move_entry((1, 0), (2, 9));
        assert_eq!(editor.entries(2).len(), 1);
        editor.remove(2, 0);
        assert!(editor.entries(2).is_empty());
        editor.remove(2, 0);
        assert!(editor.entries(2).is_empty(), "removing nothing is a no-op");

        editor.append(2, ModuleEntry::bare("notes"));
        assert_eq!(editor.entries(2), vec![ModuleEntry::bare("notes")]);
    }

    /// A removed pill leaves its rect behind, and nothing about the entry says the widget is gone. Without the length check, that ghost goes on winning drops over the area it used to occupy — ahead of whichever live pill the map happened to be walked to second, which makes it look intermittent.
    #[test]
    fn a_removed_pill_does_not_keep_catching_drops() {
        telar::reset_runtime();
        let bar = BarConfig {
            start: vec![
                ModuleEntry::bare("workspaces"),
                ModuleEntry::bare("clock"),
                ModuleEntry::bare("notes"),
            ],
            ..BarConfig::default()
        };
        let editor = ZoneEditor::new(&bar);
        let at = |x: f32| Rect {
            x,
            y: 0.0,
            width: 40.0,
            height: 20.0,
        };
        for (index, x) in [0.0f32, 50.0, 100.0].into_iter().enumerate() {
            editor.track(0, index, signal(at(x)).read_only());
        }
        // The zone row underneath them all, which is what a drop on empty space lands on.
        editor.track(
            0,
            ZONE_ROW,
            signal(Rect {
                x: 0.0,
                y: 0.0,
                width: 400.0,
                height: 20.0,
            })
            .read_only(),
        );

        assert_eq!(editor.drop_target((105.0, 10.0)), Some((0, 2)));
        editor.remove(0, 2);
        assert_eq!(
            editor.drop_target((105.0, 10.0)),
            Some((0, 2)),
            "the ghost's area now falls through to the zone row, which appends"
        );
        // And the pills that are still there keep answering for themselves.
        assert_eq!(editor.drop_target((55.0, 10.0)), Some((0, 1)));
        assert_eq!(
            editor.drop_target((75.0, 10.0)),
            Some((0, 2)),
            "past the middle of a pill lands after it"
        );
    }
}
