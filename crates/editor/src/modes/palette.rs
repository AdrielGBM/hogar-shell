//! The add palette: every module the edited layer draws — as a widget on a grid or an open panel, as a chip on a bar in the top mode — grouped by what it is about and narrowed by what is typed, after a new container (and, in the top mode, a new group on a plate first; on the overlay, a new stack of cards), and the komponents saved in `components/` with the number of parameters each takes.
//!
//! **Three ways to place (WCAG 2.5.7).** An entry dragged lands where it is let go: on the cells of a grid or an open panel, outlined as it goes; onto the middle of a loose widget, the two one Smart Stack; into a container, at the slot under the pointer; or on a bar, at the insertion line between two chips. A tag beside the pointer names it and says where, over a copy of the cells it would cover. An entry pressed is picked, and the next press on a grid, a panel or a bar puts it there; Enter puts the entry the arrows point at near the selection. Typing narrows the list whether or not the search field has the focus.
//!
//! **Sizes.** Each module entry shows the sizes its module draws as a widget on the layer, S, M and L, the medium one chosen until another is pressed (or ←/→ step the entry pointed at); the chosen size is what a drag, a press and Enter put. A chip on a bar has one size.
//!
//! **On the lock screen** only representations that read and never answer the pointer are offered: the palette asks [`offered`] for every module and [`crate::komponent::offered`] for every komponent, and that is what filters them. A komponent put on a grid is a group of its own that draws it, its parameters at their defaults, placed like a widget group.

use std::cell::RefCell;
use std::rc::Rc;

use telar::{
    Border, DismissRegistration, Key, LayoutError, LayoutItem, LayoutStyle, NamedKey, NodeId,
    ReactiveList, Rect, RectStyle, RwSignal, SizeDimension, StyledContainer, Text, box_item,
    detached, effect, signal, use_theme,
};

use config::theme::{FontRole, NordTheme};
use layout::{
    AreaId, GroupId, InstanceId, KomponentId, LayerKind, Layout, LayoutOp, Representation,
    ResolvedAreaKind,
};
use platform_wayland::KeyboardMode;
use surfaces::reconcile::{self, Desktop};
use surfaces::rects::{self, Node};
use surfaces::transient::{self, Place, Spec};
use ui::chrome::Chrome;
use ui::descriptor::{Built, Category, ModuleDescriptor};

use crate::host::{passthrough, whole};
use crate::keys::{self, Chord, KeyOp, Run};
use crate::mode::said;
use crate::popover::rows::{self, label};
use crate::session::{self, Edit, EditError, Selection};
use crate::written::Work;

use super::bars::{self, Seen};
use super::container;
use super::desktop::{self, Adding, Landing};
use super::gesture::{self, Hint};
use super::top::{self, ChipLanding};
use super::widgets::{self, Aim, Grids};

/// The transient the palette is.
pub const ID: &str = "editor:palette";

/// How wide the palette's card is.
const WIDTH: f32 = 340.0;

/// What an entry of the palette puts on the layer.
#[derive(Clone, Debug, PartialEq)]
pub enum Pick {
    /// A new instance of this module: drawn as a widget at this size on cells, where it draws one on the layer, and as a chip on a bar.
    Module(String, Option<Representation>),
    /// A new group that draws this komponent.
    Komponent(KomponentId),
    /// A new, empty container.
    Container,
    /// A new, empty group drawn on a plate in a bar's zone, which lays its chips out itself.
    Plate,
    Stack,
}

impl Pick {
    /// Refused, saying so, for a module that draws no widget on the layer.
    pub(crate) fn widget(&self) -> Result<(&str, Representation), EditError> {
        match self {
            Pick::Module(module, Some(size)) => Ok((module, *size)),
            Pick::Module(module, None) => Err(EditError::refused(telar::t!(
                "editor.desktop.no_widget",
                module = module.clone()
            ))),
            Pick::Komponent(_) | Pick::Container | Pick::Plate | Pick::Stack => {
                Err(EditError::nothing())
            }
        }
    }

    fn sized(self, chosen: &[(String, Representation)]) -> Self {
        match self {
            Pick::Module(module, size) => {
                let size = chosen
                    .iter()
                    .find(|(held, _)| *held == module)
                    .map(|(_, size)| *size)
                    .or(size);
                Pick::Module(module, size)
            }
            other => other,
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
        sizes: Vec<Representation>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Spot {
    /// On a grid or an open panel.
    Cells(AreaId, Landing),
    Bar(ChipLanding),
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Made {
    Instance(InstanceId),
    Group(AreaId, GroupId),
}

thread_local! {
    static PICKED: RwSignal<Option<Pick>> = detached(|| signal(None));
    static PLACING: RefCell<Option<DismissRegistration>> = const { RefCell::new(None) };
    static UNSELECTED: RwSignal<Option<(String, InstanceId)>> = detached(|| signal(None));
}

/// What the palette picked, while it waits for a press on a grid, a panel or a bar to put it there.
pub(crate) fn picked() -> RwSignal<Option<Pick>> {
    PICKED.with(|picked| *picked)
}

/// Picks `chosen` to be put where the next press lands; Esc drops it.
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
    bars::mark(None);
    drop(PLACING.with(|placing| placing.borrow_mut().take()));
}

/// A pick, and an instance waiting to be drawn to be selected, never outlive the mode they were made in. Installed once, on the driver thread.
pub(crate) fn install() {
    detached(|| {
        effect(|| {
            crate::mode::active().with(|_| ());
            unpick();
            UNSELECTED.with(|waiting| {
                if waiting.peek().is_some() {
                    waiting.set(None);
                }
            });
        });
        effect(select_once_drawn);
    });
    crate::host::set_add(LayerKind::Top, open);
    for layer in [LayerKind::Top, LayerKind::Overlay] {
        keys::add_mode_key_op(layer, add_key());
    }
    crate::host::add_tool(LayerKind::Overlay, widgets::placing_tool);
}

pub(crate) fn add_key() -> KeyOp {
    KeyOp {
        name: "widget-add",
        keys: vec![Chord::char('a')],
        label: || telar::t!("editor.keys.op.widget-add"),
        run: Run::Act(|_| open()),
    }
}

/// The size `descriptor` is offered at on `layer`: of the sizes it draws there ([`desktop::sizes_drawn`]), the medium one first. `None` for a module that draws no widget there.
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

/// Whether the palette lists `descriptor` on `layer`: in the top mode a module that draws a chip, which is what a bar takes; elsewhere one that draws a widget there ([`offered`]).
pub fn listed(descriptor: &ModuleDescriptor, layer: LayerKind) -> bool {
    match layer {
        LayerKind::Top => descriptor
            .input(surfaces::area::representation(Representation::Chip))
            .is_some(),
        _ => offered(descriptor, layer).is_some(),
    }
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

/// What the palette lists for `layer` when `query` is typed: in the top mode an empty group on a plate, then an empty container (and, on the overlay, an empty stack), each category's modules under its heading, then the komponents.
pub fn lines(layer: LayerKind, query: &str) -> Vec<Line> {
    let mut lines = Vec::new();
    let plate = telar::t!("editor.palette.plate");
    if layer == LayerKind::Top && matches(query, &[&plate, "plate"]) {
        lines.push(Line::Entry {
            pick: Pick::Plate,
            name: plate,
            icon: "rectangle-horizontal",
            sizes: Vec::new(),
        });
    }
    let container = telar::t!("editor.palette.container");
    if matches(query, &[&container, "container"]) {
        lines.push(Line::Entry {
            pick: Pick::Container,
            name: container,
            icon: "layout-grid",
            sizes: Vec::new(),
        });
    }
    let stack = telar::t!("editor.palette.stack");
    if layer == LayerKind::Overlay && matches(query, &[&stack, "stack"]) {
        lines.push(Line::Entry {
            pick: Pick::Stack,
            name: stack,
            icon: "layers",
            sizes: Vec::new(),
        });
    }
    for category in Category::ALL {
        let mut modules: Vec<&ModuleDescriptor> = ui::descriptor::installed()
            .iter()
            .filter(|module| module.category == category)
            .filter(|module| listed(module, layer))
            .filter(|module| matches(query, &[module.name, module.id]))
            .collect();
        if modules.is_empty() {
            continue;
        }
        modules.sort_by_key(|module| module.name);
        lines.push(Line::Heading(category_name(category)));
        lines.extend(modules.into_iter().map(|module| Line::Entry {
            pick: Pick::Module(module.id.to_string(), offered(module, layer)),
            name: module.name.to_string(),
            icon: module.icon,
            sizes: desktop::sizes_drawn(module, layer),
        }));
    }
    lines.extend(komponents(layer, query));
    lines
}

/// The komponents of the library the palette puts on `layer` that `query` matches, under their heading: in a grid cell, or in a bar's zone in the top mode.
fn komponents(layer: LayerKind, query: &str) -> Vec<Line> {
    let library = crate::written::known();
    let offered = crate::komponent::offered(
        &library,
        &surfaces::catalogue::Descriptors::installed(),
        layer,
        layer != LayerKind::Top,
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
            sizes: Vec::new(),
        })
        .collect();
    if !lines.is_empty() {
        lines.insert(0, Line::Heading(telar::t!("editor.palette.komponents")));
    }
    lines
}

fn has_grid(output: &str, layer: LayerKind) -> bool {
    reconcile::desktop_now(Some(output))
        .and_then(|desktop| {
            Some(
                desktop
                    .resolved
                    .layer(layer)?
                    .areas
                    .iter()
                    .any(|area| matches!(area.kind, ResolvedAreaKind::Grid { .. })),
            )
        })
        .unwrap_or(false)
}

/// Opens the palette on the screen being edited, closing (and so keeping what it changed) whichever popover or menu was open. The overlay mode has it once the overlay has a grid.
pub(crate) fn open() -> Result<(), EditError> {
    let mode = crate::mode::required()?;
    if mode.layer == LayerKind::Overlay && !has_grid(&mode.output, mode.layer) {
        return Err(EditError::refused(telar::t!("editor.palette.no_grid")));
    }
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

type Chosen = RwSignal<Vec<(String, Representation)>>;

/// The palette's card: a title, the search field and the lines it lists, at the left of what the reserving areas leave. The card is capped to the screen, and the lines fill what the title and the search field leave of it, scrolling past it ([`crate::popover::capped_rows`]); the entry the arrows point at is kept in view.
pub(crate) fn tree(output: &str, layer: LayerKind) -> Built {
    let theme = use_theme::<NordTheme>();
    let pad = ui::scale::space::lg();
    let gap = ui::scale::space::md();
    let query = signal(String::new());
    let pointed = signal(0usize);
    let chosen: Chosen = signal(Vec::new());
    let pointed_row: RwSignal<Option<NodeId>> = signal(None);
    let listed = move || {
        let chosen = chosen.get();
        lines(layer, &query.get())
            .into_iter()
            .map(|line| match line {
                Line::Entry {
                    pick,
                    name,
                    icon,
                    sizes,
                } => Line::Entry {
                    pick: pick.sized(&chosen),
                    name,
                    icon,
                    sizes,
                },
                heading => heading,
            })
            .collect::<Vec<Line>>()
    };
    let entries = move || -> Vec<(Pick, Vec<Representation>)> {
        listed()
            .into_iter()
            .filter_map(|line| match line {
                Line::Entry { pick, sizes, .. } => Some((pick, sizes)),
                Line::Heading(_) => None,
            })
            .collect()
    };
    let building = output.to_string();
    let list = ReactiveList::with_style(
        LayoutStyle::new()
            .flex_column()
            .gap(ui::scale::space::xs())
            .width(SizeDimension::Percent(1.0)),
        move || vec![(query.get(), pointed.get(), format!("{:?}", chosen.get()))],
        |shown: &(String, usize, String)| shown.clone(),
        move |(_, at, _): (String, usize, String)| {
            lines_of(&building, layer, listed(), (at, pointed_row), chosen)
        },
    )?;
    let title = Text::new(
        move || match layer {
            LayerKind::Top => telar::t!("editor.palette.title_top"),
            _ => telar::t!("editor.palette.title"),
        },
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
    .on_key(move |key: &Key| {
        answer(
            key,
            Asked {
                query,
                pointed,
                chosen,
            },
            &entries,
            layer,
        )
    });
    Ok(Box::new(passthrough(whole(), vec![Box::new(card)])?))
}

#[derive(Clone, Copy)]
struct Asked {
    query: RwSignal<String>,
    pointed: RwSignal<usize>,
    chosen: Chosen,
}

/// What a key does on the open palette: ↑/↓ move the entry pointed at, ←/→ step its size, Enter puts it near the selection, and typing narrows the list.
fn answer(
    key: &Key,
    asked: Asked,
    entries: &dyn Fn() -> Vec<(Pick, Vec<Representation>)>,
    layer: LayerKind,
) -> bool {
    let Asked {
        query,
        pointed,
        chosen,
    } = asked;
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
        Key::Named(step @ (NamedKey::ArrowLeft | NamedKey::ArrowRight)) => {
            if let Some((Pick::Module(module, Some(now)), sizes)) =
                entries().into_iter().nth(pointed.peek())
                && let Some(at) = sizes.iter().position(|size| *size == now)
            {
                let to = match step {
                    NamedKey::ArrowLeft => at.saturating_sub(1),
                    _ => (at + 1).min(sizes.len() - 1),
                };
                choose(chosen, &module, sizes[to]);
            }
        }
        Key::Named(NamedKey::Enter) => {
            let Some((chosen, _)) = entries().into_iter().nth(pointed.peek()) else {
                return true;
            };
            transient::close(ID);
            said(put_near(&chosen, layer));
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

fn choose(chosen: Chosen, module: &str, size: Representation) {
    chosen.update(|held| {
        held.retain(|(of, _)| of != module);
        held.push((module.to_string(), size));
    });
}

/// The palette's lines as rows, the entry `at` counts to highlighted and its row's node told to `pointed_row`, which keeps it in view.
fn lines_of(
    output: &str,
    layer: LayerKind,
    lines: Vec<Line>,
    (at, pointed_row): (usize, RwSignal<Option<NodeId>>),
    chosen: Chosen,
) -> Built {
    let mut rows: Vec<Box<dyn LayoutItem>> = Vec::with_capacity(lines.len());
    let mut entry = 0;
    let mut pointed = None;
    for line in lines {
        match line {
            Line::Heading(said) => rows.push(rows::heading(move || said.clone())?),
            Line::Entry {
                pick,
                name,
                icon,
                sizes,
            } => {
                let shown = Shown {
                    name,
                    icon,
                    sizes,
                    pointed: entry == at,
                };
                let row = entry_row(output, layer, pick, shown, chosen)?;
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

struct Shown {
    name: String,
    icon: &'static str,
    sizes: Vec<Representation>,
    pointed: bool,
}

/// One entry: pressed it is picked for the next press on a grid, a panel or a bar — a new stack is made at once — and dragged it is carried there and put where it is let go. Its sizes are pressed to choose the one it puts.
fn entry_row(output: &str, layer: LayerKind, chosen: Pick, shown: Shown, sizes: Chosen) -> Built {
    let theme = use_theme::<NordTheme>();
    let Shown {
        name,
        icon,
        sizes: drawn,
        pointed,
    } = shown;
    let glyph = ui::icon::icon_view(move || icon.to_string(), move || theme.text, 18.0)?;
    let said_name = name.clone();
    let label = Text::new(
        move || said_name.clone(),
        LayoutStyle::new().flex_grow(1.0),
        move || theme.text_style(FontRole::Body, theme.text),
    )?;
    let fill = match pointed {
        true => theme.overlay,
        false => theme.surface,
    };
    let mut children: Vec<Box<dyn LayoutItem>> = vec![glyph, box_item(label)];
    if let Pick::Module(module, now) = &chosen {
        for size in drawn {
            children.push(size_button(module, size, *now == Some(size), sizes)?);
        }
    }
    let edit = Edit::new(telar::t!("editor.palette.placed"));
    let (pressing, dragging) = (chosen.clone(), chosen);
    let previewing = edit.clone();
    let (taking, selecting) = (output.to_string(), output.to_string());
    Ok(Box::new(gesture::drag(
        StyledContainer::new(
            LayoutStyle::new()
                .flex_row()
                .gap(ui::scale::space::sm())
                .align_items(telar::AlignItems::CENTER)
                .padding_all(ui::scale::space::xs())
                .width(SizeDimension::Percent(1.0)),
            move |_| RectStyle::filled(fill, ui::scale::corner::xs()),
            children,
        )?
        .hover_style(move |_| RectStyle::filled(theme.overlay, ui::scale::corner::xs()))
        .control(telar::Role::Button)
        .cursor(telar::Cursor::Grab)
        .on_press(move || {
            transient::close(ID);
            match pressing {
                Pick::Stack => said(desktop::put(&Pick::Stack, None, layer)),
                ref other => pick(other.clone()),
            }
        }),
        edit.transaction(),
        move |_| (dragging != Pick::Stack).then(|| Carrying::of(&taking, layer, &dragging, &name)),
        move |carrying, _| {
            if let Some(point) = surfaces::menu::pointer() {
                carry(&previewing, carrying, point, layer);
            }
        },
        move |carrying, let_go| {
            widgets::aim().set(None);
            bars::mark(None);
            if !let_go {
                return;
            }
            if transient::is_open(ID) {
                transient::close(ID);
            }
            if let Some(made) = carrying.and_then(|carrying| carrying.made.into_inner()) {
                select(&selecting, layer, &made);
            }
        },
    )))
}

fn size_button(
    module: &str,
    size: Representation,
    on: bool,
    chosen: Chosen,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let theme = use_theme::<NordTheme>();
    let ink = match on {
        true => theme.accent,
        false => theme.subtle,
    };
    let letter = Text::new(
        move || desktop::size_letter(size).to_string(),
        LayoutStyle::new(),
        move || {
            theme
                .text_style(FontRole::Caption, ink)
                .with_font_weight(700)
        },
    )?;
    let module = module.to_string();
    let edge = match on {
        true => theme.accent,
        false => theme.overlay,
    };
    Ok(Box::new(
        StyledContainer::new(
            LayoutStyle::new()
                .flex_row()
                .justify_content(telar::JustifyContent::CENTER)
                .align_items(telar::AlignItems::CENTER)
                .width(22.0)
                .height(22.0),
            move |_| {
                RectStyle::filled(theme.surface, ui::scale::corner::xs())
                    .with_border(Border::uniform(edge, 1.0))
            },
            vec![box_item(letter)],
        )?
        .control(telar::Role::Button)
        .cursor(telar::Cursor::Pointer)
        .on_press(move || choose(chosen, &module, size)),
    ))
}

#[derive(Clone, Debug, PartialEq)]
enum Mark {
    Cells(Aim),
    Line(Rect),
}

/// The places it can land are read as the drag begins, so its own preview cannot move them.
struct Carrying {
    pick: Pick,
    name: String,
    output: String,
    grids: Grids,
    bars: Vec<Seen>,
    made: RefCell<Option<Made>>,
}

impl Carrying {
    fn of(output: &str, layer: LayerKind, pick: &Pick, name: &str) -> Self {
        Self {
            pick: pick.clone(),
            name: name.to_string(),
            output: output.to_string(),
            grids: widgets::drop_grids(output, layer),
            bars: bars::seen_on(output, layer, None),
            made: RefCell::new(None),
        }
    }

    /// Grids and open panels come before bars; only a module stacks onto a widget or joins a container.
    fn aim_at(&self, point: (f32, f32)) -> Option<(Spot, Mark, String)> {
        if let Some(over) = self
            .grids
            .iter()
            .find(|(geometry, _)| geometry.region.contains(point.0, point.1))
        {
            let footprint = widgets::picked_footprint(&self.pick, Some(over));
            let rect = over.0.rect_of(footprint.at(0, 0));
            let grab = (rect.width / 2.0, rect.height / 2.0);
            let (area, landing, aimed) = match self.pick {
                Pick::Module(..) => widgets::landing_at(&self.grids, point, grab, footprint, None)?,
                _ => widgets::cell_landing(over, point, grab, footprint),
            };
            let at = widgets::landing_tag(&landing);
            return Some((Spot::Cells(area, landing), Mark::Cells(aimed), at));
        }
        let bar = bars::over(&self.bars, point)?;
        let (landing, line) = bar.landing(bar.along(point));
        let at = crate::komponent::zone_name(landing.zone);
        Some((Spot::Bar(landing), Mark::Line(bar.line_at(line)), at))
    }
}

/// Over nowhere it can go, or where the layout refuses it, the layout stays as it was and the tag says why.
fn carry(edit: &Edit, carrying: &Carrying, point: (f32, f32), layer: LayerKind) {
    let aimed = carrying.aim_at(point);
    let shown = match &aimed {
        None => Err(EditError::refused(telar::t!("editor.palette.nowhere"))),
        Some((spot, _, _)) => edit
            .transaction()
            .before()
            .ok_or(EditError::NotOpen)
            .and_then(|before| {
                let desktop = reconcile::desktop_now(Some(&carrying.output))
                    .ok_or_else(EditError::no_output)?;
                let (ops, made) = planned(&before, &desktop, layer, &carrying.pick, spot)?;
                edit.preview(ops)?;
                edit.relabel(label_of(&carrying.pick, &made));
                Ok(made)
            }),
    };
    if shown.is_err() {
        let _ = edit.preview(Vec::new());
    }
    let mark = aimed
        .as_ref()
        .filter(|_| shown.is_ok())
        .map(|(_, mark, _)| mark.clone());
    widgets::aim().set(match &mark {
        Some(Mark::Cells(aimed)) => Some(aimed.clone()),
        _ => None,
    });
    bars::mark(match &mark {
        Some(Mark::Line(line)) => Some(*line),
        _ => None,
    });
    let tag = match (&aimed, &shown) {
        (Some((_, _, at)), Ok(_)) => telar::t!(
            "editor.palette.tag",
            name = carrying.name.clone(),
            at = at.clone()
        ),
        (_, Err(why)) => why.to_string(),
        (None, Ok(_)) => String::new(),
    };
    gesture::hint().set(Some(Hint {
        pointer: point,
        tag: Some(tag),
        ghost: match &mark {
            Some(Mark::Cells(Aim::Cells(rect))) => Some(*rect),
            _ => None,
        },
        guides: Vec::new(),
    }));
    *carrying.made.borrow_mut() = shown.ok();
}

pub(crate) fn planned(
    layout: &Layout,
    desktop: &Desktop,
    layer: LayerKind,
    pick: &Pick,
    spot: &Spot,
) -> Result<(Vec<LayoutOp>, Made), EditError> {
    match spot {
        Spot::Cells(area, Landing::Cell { col, row }) => {
            on_cells(layout, desktop, layer, pick, (area, (*col, *row)))
        }
        Spot::Cells(area, Landing::Onto(group)) => {
            joining(layout, desktop, layer, pick, |work, instance| {
                desktop::stack_onto(work, area, group, instance)
            })
        }
        Spot::Cells(area, Landing::Into(group, slot)) => {
            joining(layout, desktop, layer, pick, |work, instance| {
                container::adopt(work, area, group, instance, *slot)
            })
        }
        Spot::Bar(landing) => on_bar(layout, desktop, layer, pick, landing),
    }
}

fn on_cells(
    layout: &Layout,
    desktop: &Desktop,
    layer: LayerKind,
    pick: &Pick,
    (area, at): (&AreaId, (u32, u32)),
) -> Result<(Vec<LayoutOp>, Made), EditError> {
    let made = |group: GroupId| Made::Group(area.clone(), group);
    match pick {
        Pick::Module(..) => {
            let (module, representation) = pick.widget()?;
            let adding = Adding {
                module,
                representation,
                at: Some(at),
                near: (0, 0),
            };
            desktop::added(layout, desktop, layer, area, &adding)
                .map(|(ops, id)| (ops, Made::Instance(id)))
        }
        Pick::Komponent(id) => crate::komponent::planned(
            layout,
            desktop,
            layer,
            area,
            id,
            crate::komponent::Where::Cell {
                at: Some(at),
                near: (0, 0),
            },
        )
        .map(|(ops, group)| (ops, made(group))),
        Pick::Container => container::on_grid(layout, desktop, layer, area, (Some(at), (0, 0)))
            .map(|(ops, group)| (ops, made(group))),
        Pick::Plate => Err(EditError::refused(telar::t!(
            "editor.palette.plate_off_bar"
        ))),
        Pick::Stack => Err(EditError::nothing()),
    }
}

fn joining(
    layout: &Layout,
    desktop: &Desktop,
    layer: LayerKind,
    pick: &Pick,
    put: impl FnOnce(&mut Work, layout::Instance) -> Result<(), EditError>,
) -> Result<(Vec<LayoutOp>, Made), EditError> {
    let (module, representation) = pick.widget()?;
    let mut work = Work::new(layout, desktop, layer);
    let instance = desktop::fresh(&work, module, representation)?;
    let id = instance.id.clone();
    put(&mut work, instance)?;
    Ok((work.done(), Made::Instance(id)))
}

fn on_bar(
    layout: &Layout,
    desktop: &Desktop,
    layer: LayerKind,
    pick: &Pick,
    landing: &ChipLanding,
) -> Result<(Vec<LayoutOp>, Made), EditError> {
    let made = |group: GroupId| Made::Group(landing.area.clone(), group);
    match pick {
        Pick::Module(module, _) => top::chip_added(layout, desktop, layer, module, landing)
            .map(|(ops, id)| (ops, Made::Instance(id))),
        Pick::Komponent(id) => crate::komponent::planned(
            layout,
            desktop,
            layer,
            &landing.area,
            id,
            crate::komponent::Where::Zone(landing.zone),
        )
        .map(|(ops, group)| (ops, made(group))),
        Pick::Plate => container::in_bar(layout, desktop, layer, &landing.area, landing.zone)
            .map(|(ops, group)| (ops, made(group))),
        Pick::Container => Err(EditError::refused(telar::t!(
            "editor.palette.container_on_bar"
        ))),
        Pick::Stack => Err(EditError::nothing()),
    }
}

/// What the history calls putting `pick` down as `made`, however it was put there.
fn label_of(pick: &Pick, made: &Made) -> String {
    match (pick, made) {
        (Pick::Komponent(id), _) => {
            telar::t!("editor.komponent.used", komponent = id.to_string())
        }
        (Pick::Module(module, _), _) => {
            let name = ui::descriptor::find(module).map_or(module.as_str(), |found| found.name);
            telar::t!("editor.desktop.added", name = name)
        }
        (Pick::Plate, Made::Group(_, group)) => top::plated(group),
        (_, Made::Group(_, group)) => {
            telar::t!("editor.container.made", name = group.to_string())
        }
        (_, Made::Instance(_)) => telar::t!("editor.palette.placed"),
    }
}

/// Selects what was made: a group at once, an instance once it is drawn, since it is found by where it is drawn ([`select_once_drawn`]).
fn select(output: &str, layer: LayerKind, made: &Made) {
    match made {
        Made::Instance(id) => {
            UNSELECTED.with(|waiting| waiting.set(Some((output.to_string(), id.clone()))));
            select_once_drawn();
        }
        Made::Group(area, group) => {
            session::select(Selection::Group(
                Node::area(Some(output), layer, area).group(group),
            ));
        }
    }
}

/// Selects the instance the palette made last as soon as it is drawn.
fn select_once_drawn() {
    let Some((output, id)) = UNSELECTED.with(|waiting| waiting.get()) else {
        return;
    };
    if let Some((node, _)) = rects::instance(Some(&output), &id) {
        UNSELECTED.with(|waiting| waiting.set(None));
        session::select(Selection::Instance(node));
    }
}

pub(crate) fn place(pick: &Pick, spot: &Spot) -> Result<(), EditError> {
    let mode = crate::mode::required()?;
    let desktop = reconcile::desktop_now(Some(&mode.output)).ok_or_else(EditError::no_output)?;
    let (ops, made) = planned(&session::draft().peek(), &desktop, mode.layer, pick, spot)?;
    crate::context::commit(label_of(pick, &made), ops)?;
    select(&mode.output, mode.layer, &made);
    Ok(())
}

fn on_a_panel(layer: LayerKind) -> bool {
    let selected = session::selected();
    let Some(node) = selected.node() else {
        return false;
    };
    node.layer == layer
        && reconcile::desktop_now(node.output.as_deref())
            .and_then(|desktop| {
                let area = desktop.resolved.area(layer, &node.area)?;
                Some(matches!(area.kind, ResolvedAreaKind::Panel { .. }))
            })
            .unwrap_or(false)
}

/// Puts `pick` near the selection, as one undo entry, and selects it: in the top mode at the end of the zone of the selected bar (the start of the first bar otherwise), unless a panel is selected, which takes anything but a group on a plate on its free cells as every grid does elsewhere ([`desktop::put`]).
pub(crate) fn put_near(pick: &Pick, layer: LayerKind) -> Result<(), EditError> {
    if layer != LayerKind::Top || (*pick != Pick::Plate && on_a_panel(layer)) {
        return desktop::put(pick, None, layer);
    }
    let mode = crate::mode::required()?;
    let desktop = reconcile::desktop_now(Some(&mode.output)).ok_or_else(EditError::no_output)?;
    let (area, zone) = container::bar_near(&desktop, layer)
        .ok_or_else(|| EditError::refused(telar::t!("editor.container.no_bar")))?;
    place(
        pick,
        &Spot::Bar(ChipLanding {
            area,
            zone,
            index: usize::MAX,
        }),
    )
}
