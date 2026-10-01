//! The add-widget palette (TA-5): every module that draws as a widget, grouped by what it is about and narrowed by what is typed, and — "From bars…" — the chips on this screen's bars that could be widgets instead.
//!
//! **Three ways to place (WCAG 2.5.7).** An entry dragged onto a grid lands where it is let go, the cells it would cover outlined as it goes; an entry pressed is picked, and the next press on a grid puts it on the cells there; Enter puts the entry the arrows point at on the free cells nearest the selection. Typing narrows the list whether or not the search field has the focus.
//!
//! **On the lock screen** only representations that read and never answer the pointer are offered (TA-8): the palette asks [`offered`] for every module, and that is what filters it.

use std::cell::RefCell;
use std::rc::Rc;

use telar::{
    DismissRegistration, Key, LayoutItem, LayoutStyle, NamedKey, ReactiveList, RectStyle, RwSignal,
    SizeDimension, StyledContainer, Text, box_item, detached, effect, signal, use_theme,
};

use config::theme::{FontRole, NordTheme};
use layout::{LayerKind, Representation, ResolvedAreaKind};
use platform_wayland::KeyboardMode;
use surfaces::reconcile::{self, Desktop};
use surfaces::rects::Node;
use surfaces::transient::{self, Place, Spec};
use ui::chrome::Chrome;
use ui::descriptor::{Built, Category, ModuleDescriptor};

use crate::host::{passthrough, whole};
use crate::mode::said;
use crate::popover::rows::{self, label};
use crate::session::{Edit, EditError};

use super::desktop::{self, Adding};
use super::gesture;
use super::widgets::{self, Aim};

/// The transient the palette is.
pub const ID: &str = "editor:palette";

/// How wide the palette's card is.
const WIDTH: f32 = 320.0;

/// Which of its sections the palette opens with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Offer {
    /// Every module that draws as a widget, then the chips on the bars.
    Every,
    /// The chips on the bars alone.
    FromBars,
}

/// What an entry of the palette puts on a grid.
#[derive(Clone, Debug, PartialEq)]
pub enum Pick {
    /// A new instance of this module.
    Module(String),
    /// The chip instance this node names, moved off its bar.
    FromBar(Node),
}

impl Pick {
    /// The module it puts on a grid: a chip's as its screen shows it now.
    pub(crate) fn module(&self) -> Option<String> {
        match self {
            Pick::Module(module) => Some(module.clone()),
            Pick::FromBar(node) => {
                let desktop = reconcile::desktop_now(node.output.as_deref())?;
                desktop::shown_instance(&desktop, node).map(|chip| chip.module)
            }
        }
    }
}

/// One line of the palette.
#[derive(Clone, Debug, PartialEq)]
pub enum Line {
    Heading(String),
    Entry {
        pick: Pick,
        name: String,
        icon: &'static str,
    },
}

thread_local! {
    static PICKED: RwSignal<Option<Pick>> = detached(|| signal(None));
    static PLACING: RefCell<Option<DismissRegistration>> = const { RefCell::new(None) };
}

/// What the palette picked, while it waits for a press on a grid to put it there.
pub(crate) fn picked() -> RwSignal<Option<Pick>> {
    PICKED.with(|picked| *picked)
}

/// Picks `chosen` to be put on the next grid cell pressed; Esc drops it.
fn pick(chosen: Pick) {
    picked().set(Some(chosen));
    let dropping = DismissRegistration::new(Rc::new(unpick));
    PLACING.with(|placing| *placing.borrow_mut() = Some(dropping));
}

/// Drops what the palette picked, and the way out of placing it with it.
pub(crate) fn unpick() {
    if picked().peek().is_some() {
        picked().set(None);
    }
    widgets::aim().set(None);
    drop(PLACING.with(|placing| placing.borrow_mut().take()));
}

/// Drops what the palette picked whenever the mode changes, so a pick never outlives the mode it was made in. Installed once, on the driver thread.
pub(crate) fn install() {
    detached(|| {
        effect(|| {
            crate::mode::active().with(|_| ());
            unpick();
        });
    });
}

/// The size `descriptor` is offered at on `layer`: of the sizes it draws there ([`desktop::sizes_drawn`]), the medium one first. `None` for a module the palette does not offer there.
pub fn offered(descriptor: &ModuleDescriptor, layer: LayerKind) -> Option<Representation> {
    let drawn = desktop::sizes_drawn(descriptor, layer);
    [
        Representation::WidgetM,
        Representation::WidgetS,
        Representation::WidgetL,
    ]
    .into_iter()
    .find(|size| drawn.contains(size))
}

fn category_name(category: Category) -> String {
    match category {
        Category::Time => telar::t!("editor.palette.category.time"),
        Category::System => telar::t!("editor.palette.category.system"),
        Category::Network => telar::t!("editor.palette.category.network"),
        Category::Media => telar::t!("editor.palette.category.media"),
        Category::Windows => telar::t!("editor.palette.category.windows"),
        Category::Info => telar::t!("editor.palette.category.info"),
        Category::Shell => telar::t!("editor.palette.category.shell"),
    }
}

fn matches(query: &str, words: &[&str]) -> bool {
    let query = query.trim().to_lowercase();
    query.is_empty()
        || words
            .iter()
            .any(|word| word.to_lowercase().contains(&query))
}

/// What the palette lists for `layer` of `desktop`'s screen when `query` is typed: each category's modules under its heading, then the chips on the screen's bars that could be widgets — the first or the second alone, as `offer` asks. The lock screen has no bars to take chips from.
pub fn lines(desktop: &Desktop, layer: LayerKind, query: &str, offer: Offer) -> Vec<Line> {
    let mut lines = Vec::new();
    if offer == Offer::Every {
        for category in Category::ALL {
            let mut modules: Vec<&ModuleDescriptor> = ui::descriptor::installed()
                .iter()
                .filter(|module| module.category == category)
                .filter(|module| offered(module, layer).is_some())
                .filter(|module| matches(query, &[module.name, module.id]))
                .collect();
            if modules.is_empty() {
                continue;
            }
            modules.sort_by_key(|module| module.name);
            lines.push(Line::Heading(category_name(category)));
            lines.extend(modules.into_iter().map(|module| Line::Entry {
                pick: Pick::Module(module.id.to_string()),
                name: module.name.to_string(),
                icon: module.icon,
            }));
        }
    }
    if layer == LayerKind::Lock {
        return lines;
    }
    let chips = chips(desktop, layer, query);
    if !chips.is_empty() {
        lines.push(Line::Heading(telar::t!("editor.desktop.from_bars")));
        lines.extend(chips);
    }
    lines
}

/// Every chip on a bar of `desktop`'s screen whose module draws as a widget on `layer`.
fn chips(desktop: &Desktop, layer: LayerKind, query: &str) -> Vec<Line> {
    let mut found = Vec::new();
    for on in LayerKind::SESSION {
        let Some(held) = desktop.resolved.layer(on) else {
            continue;
        };
        for area in &held.areas {
            if !matches!(area.kind, ResolvedAreaKind::Bar { .. }) {
                continue;
            }
            for group in &area.groups {
                for child in &group.children {
                    let Some(module) = ui::descriptor::find(&child.module) else {
                        continue;
                    };
                    let wanted = child.representation == Representation::Chip
                        && offered(module, layer).is_some()
                        && matches(query, &[module.name, module.id, area.id.as_str()]);
                    if !wanted {
                        continue;
                    }
                    let node = Node::area(desktop.output.as_deref(), on, &area.id)
                        .instance(&group.id, &child.id);
                    found.push(Line::Entry {
                        pick: Pick::FromBar(node),
                        name: telar::t!(
                            "editor.palette.on_bar",
                            name = module.name,
                            bar = area.id.to_string()
                        ),
                        icon: module.icon,
                    });
                }
            }
        }
    }
    found
}

/// Opens the palette on the screen being edited, closing (and so keeping what it changed) whichever popover or menu was open.
pub(crate) fn open(offer: Offer) -> Result<(), EditError> {
    let mode = crate::mode::required()?;
    crate::host::close_transients();
    let (output, layer) = (mode.output.clone(), mode.layer);
    transient::open(
        Spec::new(
            ID,
            Place::Whole,
            Rc::new(move |_: &Chrome| tree(&output, layer, offer)),
        )
        .output(Some(mode.output.clone()))
        .keyboard(KeyboardMode::Exclusive)
        .dismiss_on_outside(),
    );
    Ok(())
}

/// The palette's card: a title, the search field and the lines it lists, at the left of what the reserving areas leave.
pub(crate) fn tree(output: &str, layer: LayerKind, offer: Offer) -> Built {
    let theme = use_theme::<NordTheme>();
    let query = signal(String::new());
    let pointed = signal(0usize);
    let listing = output.to_string();
    let listed = move || {
        reconcile::desktop(Some(&listing))
            .map(|desktop| lines(&desktop, layer, &query.get(), offer))
            .unwrap_or_default()
    };
    let entries = {
        let listed = listed.clone();
        move || -> Vec<Pick> {
            listed()
                .into_iter()
                .filter_map(|line| match line {
                    Line::Entry { pick, .. } => Some(pick),
                    Line::Heading(_) => None,
                })
                .collect()
        }
    };
    let building = output.to_string();
    let list = ReactiveList::with_style(
        LayoutStyle::new()
            .flex_column()
            .gap(ui::scale::space::xs())
            .width(SizeDimension::Percent(1.0)),
        move || vec![(query.get(), pointed.get())],
        |shown: &(String, usize)| shown.clone(),
        {
            let listed = listed.clone();
            move |(_, at): (String, usize)| lines_of(&building, layer, listed(), at)
        },
    )?;
    let title = Text::new(
        || telar::t!("editor.palette.title"),
        LayoutStyle::new(),
        move || {
            theme
                .text_style(FontRole::Body, theme.text)
                .with_font_weight(700)
        },
    )?;
    let search = rows::text(label!("editor.palette.search"), None, query)?;
    let scroll = telar::LayoutScrollArea::new(
        LayoutStyle::new()
            .width(WIDTH - 2.0 * ui::scale::space::lg())
            .flex_grow(1.0)
            .flex_shrink(1.0),
        box_item(list),
    )?;
    let placed = output.to_string();
    let card = StyledContainer::new(
        LayoutStyle::new()
            .absolute()
            .width(WIDTH)
            .flex_column()
            .gap(ui::scale::space::md())
            .padding_all(ui::scale::space::lg()),
        move |_| RectStyle::filled(theme.surface, ui::scale::corner::xl()),
        vec![box_item(title), search, Box::new(scroll)],
    )?
    .styled_by(move || {
        let usable = crate::host::usable(Some(&placed));
        let margin = ui::scale::space::lg();
        LayoutStyle::new()
            .absolute()
            .inset_start(usable.x + margin)
            .inset_top(usable.y + 4.0 * margin)
            .width(WIDTH)
            .max_height((usable.height - 8.0 * margin).max(120.0))
            .flex_column()
            .gap(ui::scale::space::md())
            .padding_all(ui::scale::space::lg())
    })
    .input_opaque()
    .on_key(move |key: &Key| answer(key, query, pointed, &entries, layer));
    Ok(Box::new(passthrough(whole(), vec![Box::new(card)])?))
}

/// What a key does on the open palette: the arrows move the entry pointed at, Enter puts it on the free cells nearest the selection, and typing narrows the list.
fn answer(
    key: &Key,
    query: RwSignal<String>,
    pointed: RwSignal<usize>,
    entries: &dyn Fn() -> Vec<Pick>,
    layer: LayerKind,
) -> bool {
    let modifiers = telar::modifiers();
    if modifiers.is_ctrl || modifiers.is_alt || modifiers.is_meta {
        return false;
    }
    match key {
        Key::Named(NamedKey::ArrowDown) => {
            let count = entries().len();
            pointed.update(|at| *at = (*at + 1).min(count.saturating_sub(1)));
        }
        Key::Named(NamedKey::ArrowUp) => pointed.update(|at| *at = at.saturating_sub(1)),
        Key::Named(NamedKey::Enter) => {
            let Some(chosen) = entries().into_iter().nth(pointed.peek()) else {
                return true;
            };
            transient::close(ID);
            said(desktop::put(&chosen, None, layer));
        }
        Key::Named(NamedKey::Backspace) => {
            query.update(|text| {
                text.pop();
            });
            pointed.set(0);
        }
        Key::Char(ch) if !ch.is_control() => {
            query.update(|text| text.push(*ch));
            pointed.set(0);
        }
        _ => return false,
    }
    true
}

/// The palette's lines as rows, the entry `at` counts to highlighted.
fn lines_of(output: &str, layer: LayerKind, lines: Vec<Line>, at: usize) -> Built {
    let mut rows: Vec<Box<dyn LayoutItem>> = Vec::with_capacity(lines.len());
    let mut entry = 0;
    for line in lines {
        match line {
            Line::Heading(said) => rows.push(rows::heading(move || said.clone())?),
            Line::Entry { pick, name, icon } => {
                rows.push(entry_row(output, layer, pick, name, icon, entry == at)?);
                entry += 1;
            }
        }
    }
    if rows.is_empty() {
        rows.push(rows::note(|| telar::t!("editor.palette.nothing"))?);
    }
    Ok(box_item(telar::Container::new(
        LayoutStyle::new()
            .flex_column()
            .gap(ui::scale::space::xs())
            .width(SizeDimension::Percent(1.0)),
        rows,
    )?))
}

/// One entry: pressed it is picked for the next grid cell pressed, dragged it is carried onto a grid and put where it is let go.
fn entry_row(
    output: &str,
    layer: LayerKind,
    chosen: Pick,
    name: String,
    icon: &'static str,
    pointed: bool,
) -> Built {
    let theme = use_theme::<NordTheme>();
    let glyph = ui::icon::icon_view(move || icon.to_string(), move || theme.text, 18.0)?;
    let label = Text::new(
        move || name.clone(),
        LayoutStyle::new().flex_grow(1.0),
        move || theme.text_style(FontRole::Body, theme.text),
    )?;
    let fill = match pointed {
        true => theme.overlay,
        false => theme.surface,
    };
    let edit = Edit::new(telar::t!("editor.palette.placed"));
    let (pressing, dragging) = (chosen.clone(), chosen);
    let previewing = edit.clone();
    let output = output.to_string();
    Ok(Box::new(gesture::drag(
        StyledContainer::new(
            LayoutStyle::new()
                .flex_row()
                .gap(ui::scale::space::sm())
                .align_items(telar::AlignItems::CENTER)
                .padding_all(ui::scale::space::xs())
                .width(SizeDimension::Percent(1.0)),
            move |_| RectStyle::filled(fill, ui::scale::corner::xs()),
            vec![glyph, box_item(label)],
        )?
        .hover_style(move |_| RectStyle::filled(theme.overlay, ui::scale::corner::xs()))
        .control(telar::Role::Button)
        .cursor(telar::Cursor::Grab)
        .on_press(move || {
            transient::close(ID);
            pick(pressing.clone());
        }),
        edit.transaction(),
        move |_| Some(widgets::grids(&output, layer)),
        move |grids, _| {
            if let Some(point) = surfaces::menu::pointer() {
                carry(&previewing, grids, &dragging, point, layer);
            }
        },
        |_, let_go| {
            widgets::aim().set(None);
            if let_go && transient::is_open(ID) {
                transient::close(ID);
            }
        },
    )))
}

/// Previews `carried` put where the pointer at `point` is over one of `grids`, and outlines it; over none, the layout stays as it was.
fn carry(
    edit: &Edit,
    grids: &[(widgets::Geometry, layout::ResolvedArea)],
    carried: &Pick,
    point: (f32, f32),
    layer: LayerKind,
) {
    let footprint = widgets::picked_footprint(carried, layer);
    let middle = grids
        .first()
        .map(|(geometry, _)| {
            let rect = geometry.rect_of(footprint.at(0, 0));
            (rect.width / 2.0, rect.height / 2.0)
        })
        .unwrap_or_default();
    let planned = widgets::landing_at(grids, point, middle, footprint, None).and_then(
        |(onto, landing, aimed)| {
            let desktop::Landing::Cell { col, row } = landing else {
                return None;
            };
            let aimed = match aimed {
                Aim::Onto(_) => return None,
                cells => cells,
            };
            let before = edit.transaction().before()?;
            let desktop = reconcile::desktop_now(Some(&crate::mode::current()?.output))?;
            let module = carried.module()?;
            let representation = desktop::first_size(&module, layer)?;
            let at = Some((col, row));
            let ops = match carried {
                Pick::Module(_) => {
                    let adding = Adding {
                        module: &module,
                        representation,
                        at,
                        near: (0, 0),
                    };
                    desktop::added(&before, &desktop, layer, &onto, &adding)
                        .ok()?
                        .0
                }
                Pick::FromBar(node) => {
                    desktop::moved_onto(&before, &desktop, node, (layer, &onto), representation, at)
                        .ok()?
                }
            };
            Some((ops, aimed))
        },
    );
    gesture::aimed(edit, planned, widgets::aim());
}
