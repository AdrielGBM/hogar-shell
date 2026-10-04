//! The add-widget palette (TA-5): every module that draws as a widget, grouped by what it is about and narrowed by what is typed, the komponents saved in `components/` with the number of parameters each takes.
//!
//! **Three ways to place (WCAG 2.5.7).** An entry dragged onto a grid lands where it is let go, the cells it would cover outlined as it goes; an entry pressed is picked, and the next press on a grid puts it on the cells there; Enter puts the entry the arrows point at on the free cells nearest the selection. Typing narrows the list whether or not the search field has the focus.
//!
//! **On the lock screen** only representations that read and never answer the pointer are offered (TA-8): the palette asks [`offered`] for every module and [`crate::komponent::offered`] for every komponent, and that is what filters them. A komponent put on a grid is a group of its own that draws it, its parameters at their defaults, placed like a widget group.

use std::cell::RefCell;
use std::rc::Rc;

use telar::{
    DismissRegistration, Key, LayoutError, LayoutItem, LayoutStyle, NamedKey, NodeId, ReactiveList,
    RectStyle, RwSignal, SizeDimension, StyledContainer, Text, box_item, detached, effect, signal,
    use_theme,
};

use config::theme::{FontRole, NordTheme};
use layout::{KomponentId, LayerKind, Representation};
use platform_wayland::KeyboardMode;
use surfaces::reconcile::{self};
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

/// What an entry of the palette puts on a grid.
#[derive(Clone, Debug, PartialEq)]
pub enum Pick {
    /// A new instance of this module.
    Module(String),
    /// A new group that draws this komponent.
    Komponent(KomponentId),
}

impl Pick {
    pub(crate) fn module(&self) -> Option<String> {
        match self {
            Pick::Module(module) => Some(module.clone()),
            Pick::Komponent(_) => None,
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

/// What the palette lists for `layer` when `query` is typed: each category's modules under its heading, then the komponents.
pub fn lines(layer: LayerKind, query: &str) -> Vec<Line> {
    let mut lines = Vec::new();
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
    lines.extend(komponents(layer, query));
    lines
}

/// The komponents of the library the palette puts on a grid of `layer` that `query` matches, under their heading.
fn komponents(layer: LayerKind, query: &str) -> Vec<Line> {
    let library = crate::written::known();
    let offered = crate::komponent::offered(
        &library,
        &surfaces::catalogue::Descriptors::installed(),
        layer,
        true,
    );
    let mut lines: Vec<Line> = offered
        .into_iter()
        .filter(|found| matches(query, &[found.id.as_str()]))
        .map(|found| Line::Entry {
            name: telar::t!(
                "editor.palette.komponent",
                name = found.id.to_string(),
                count = found.parameters
            ),
            pick: Pick::Komponent(found.id),
            icon: "puzzle",
        })
        .collect();
    if !lines.is_empty() {
        lines.insert(0, Line::Heading(telar::t!("editor.palette.komponents")));
    }
    lines
}

/// Opens the palette on the screen being edited, closing (and so keeping what it changed) whichever popover or menu was open.
pub(crate) fn open() -> Result<(), EditError> {
    let mode = crate::mode::required()?;
    crate::host::close_transients();
    let (output, layer) = (mode.output.clone(), mode.layer);
    transient::open(
        Spec::new(
            ID,
            Place::Whole,
            Rc::new(move |_: &Chrome| tree(&output, layer)),
        )
        .output(Some(mode.output.clone()))
        .keyboard(KeyboardMode::Exclusive)
        .dismiss_on_outside(),
    );
    Ok(())
}

/// The palette's card: a title, the search field and the lines it lists, at the left of what the reserving areas leave. The card is capped to the screen, and the lines fill what the title and the search field leave of it, scrolling past it ([`crate::popover::capped_rows`]); the entry the arrows point at is kept in view.
pub(crate) fn tree(output: &str, layer: LayerKind) -> Built {
    let theme = use_theme::<NordTheme>();
    let pad = ui::scale::space::lg();
    let gap = ui::scale::space::md();
    let query = signal(String::new());
    let pointed = signal(0usize);
    let pointed_row: RwSignal<Option<NodeId>> = signal(None);
    let listed = move || lines(layer, &query.get());
    let entries = {
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
        move |(_, at): (String, usize)| lines_of(&building, layer, listed(), at, pointed_row),
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
    let tracked = |item: &dyn LayoutItem, what: &str| {
        telar::track_layout(item.layout_node())
            .ok_or_else(|| LayoutError::Engine(format!("the palette's {what} has no layout node")))
    };
    let (title_height, search_height) = (tracked(&title, "title")?, tracked(&*search, "search")?);
    let capped = output.to_string();
    let room = move || {
        let usable = crate::host::usable(Some(&capped));
        let card = (usable.height - 8.0 * pad).max(120.0);
        (card - title_height.get().height - search_height.get().height - 2.0 * gap - 2.0 * pad)
            .max(0.0)
    };
    let (lines, viewport) = crate::popover::capped_rows(box_item(list), WIDTH - 2.0 * pad, room)?;
    effect(move || {
        if let Some(row) = pointed_row.get() {
            viewport.reveal(row, gap);
        }
    });
    let placed = output.to_string();
    let style = move || {
        let usable = crate::host::usable(Some(&placed));
        LayoutStyle::new()
            .absolute()
            .inset_start(usable.x + pad)
            .inset_top(usable.y + 4.0 * pad)
            .width(WIDTH)
            .flex_column()
            .gap(gap)
            .padding_all(pad)
    };
    let card = StyledContainer::new(
        style(),
        move |_| RectStyle::filled(theme.surface, ui::scale::corner::xl()),
        vec![box_item(title), search, lines],
    )?
    .styled_by(style)
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

/// The palette's lines as rows, the entry `at` counts to highlighted and its row's node told to `pointed_row`, which keeps it in view.
fn lines_of(
    output: &str,
    layer: LayerKind,
    lines: Vec<Line>,
    at: usize,
    pointed_row: RwSignal<Option<NodeId>>,
) -> Built {
    let mut rows: Vec<Box<dyn LayoutItem>> = Vec::with_capacity(lines.len());
    let mut entry = 0;
    let mut pointed = None;
    for line in lines {
        match line {
            Line::Heading(said) => rows.push(rows::heading(move || said.clone())?),
            Line::Entry { pick, name, icon } => {
                let row = entry_row(output, layer, pick, name, icon, entry == at)?;
                if entry == at {
                    pointed = Some(row.layout_node());
                }
                rows.push(row);
                entry += 1;
            }
        }
    }
    if rows.is_empty() {
        rows.push(rows::note(|| telar::t!("editor.palette.nothing"))?);
    }
    if pointed_row.peek() != pointed {
        pointed_row.set(pointed);
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
            let at = Some((col, row));
            let ops = match carried {
                Pick::Module(module) => {
                    let representation = desktop::first_size(module, layer)?;
                    let adding = Adding {
                        module,
                        representation,
                        at,
                        near: (0, 0),
                    };
                    desktop::added(&before, &desktop, layer, &onto, &adding)
                        .ok()?
                        .0
                }
                Pick::Komponent(id) => {
                    desktop::planned_use(&before, &desktop, layer, &onto, id, (at, (0, 0)))
                        .ok()?
                        .0
                }
            };
            Some((ops, aimed))
        },
    );
    gesture::aimed(edit, planned, widgets::aim());
}
