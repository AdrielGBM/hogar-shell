mod dock;
mod lens;
mod order;
mod pins;
mod targets;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use telar::{
    AlignItems, Color, DragAxis, JustifyContent, LayoutError, LayoutItem, LayoutStyle, Memo,
    ReactiveList, Rect, RectStyle, RwSignal, SizeDimension, StyledContainer, Text, box_item,
    effect, memo, on_cleanup, signal, track_layout, use_theme,
};

use config::WindowsConfig;
use config::theme::{FontRole, NordTheme};
use platform_wayland::{ManagedToplevel, ManagedToplevelId, SurfaceRef, ToplevelArea};
use ui::host::Host;
use ui::icon::{app_icon_view, icon_view};
use ui::scale::space;

pub use dock::dot_fills;
pub use order::{Arrangement, arrangement};

use targets::Holding;

const ROW: f32 = 30.0;
const ROW_ICON: f32 = 18.0;
const TITLE_MAX: f32 = 220.0;
const DRAG_SLOP: f32 = 4.0;
const DRAGGED_OPACITY: f32 = 0.45;
const REST_ALPHA: f32 = 0.07;
const HOVER_ALPHA: f32 = 0.14;
const ACTIVE_ALPHA: f32 = 0.26;
const GLYPH: &str = "app-window";

pub fn fills(theme: NordTheme) -> (Color, Color) {
    (
        theme.text.with_alpha(REST_ALPHA),
        theme.accent.with_alpha(ACTIVE_ALPHA),
    )
}

pub fn hover_fill(theme: NordTheme) -> Color {
    theme.text.with_alpha(HOVER_ALPHA)
}

pub fn strip(host: &Host) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let windows = signal(services::windows::current().unwrap_or_default());
    platform_wayland::watch(services::windows::subscribe, move |open| windows.set(open));
    if !host.is_docked() {
        return strip_over(host, windows);
    }
    let apps = signal(services::apps::all());
    platform_wayland::watch(services::apps::subscribe, move |all| apps.set(all));
    dock::dock(host, windows, apps, dock::Seams::live())
}

fn strip_over(
    host: &Host,
    windows: RwSignal<Vec<ManagedToplevel>>,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let titles = host.options::<WindowsConfig>().titles;
    let output = host.output.clone();
    let key = output.clone().unwrap_or_default();

    let remembered = remembered(&key);
    let unsupported = signal(false);
    platform_wayland::watch(services::windows::subscribe_unsupported, move |missing| {
        unsupported.set(missing)
    });
    let reordered = signal(0u64);
    let shown = {
        let key = key.clone();
        memo(move || {
            reordered.get();
            order::arrange(&key, &windows.get(), &remembered.get())
        })
    };

    let spacing = Spacing::of(host);
    let shape = host.follow(move |host| Shape::of(host, titles, spacing));
    let looks = memo(move || Look {
        shape: shape.get(),
        empty: shown.with(Vec::is_empty),
        unsupported: unsupported.get(),
    });

    let strip = Rc::new(Strip {
        theme: use_theme::<NordTheme>(),
        radius: host.corner_radius(),
        spacing,
        thickness: host.axis.map(|_| host.thickness()),
        key,
        output,
        windows,
        shown,
        reordered,
        surface: platform_wayland::current_surface(),
        spans: Rc::default(),
        dragging: signal(None),
        landed: Cell::new(None),
        moved: Cell::new(false),
    });
    let outer = match host.axis {
        Some(_) => LayoutStyle::new()
            .flex_column()
            .min_width(0.0)
            .min_height(0.0)
            .flex_shrink(1.0),
        None => LayoutStyle::new()
            .flex_column()
            .width(SizeDimension::Percent(1.0))
            .height(SizeDimension::Percent(1.0)),
    };
    let list = ReactiveList::with_style(
        outer,
        move || vec![looks.get()],
        Look::key,
        move |look: Look| match look.empty {
            true => empty(&strip, look),
            false => entries(&strip, look),
        },
    )?;
    Ok(Box::new(list))
}

fn remembered(output: &str) -> RwSignal<Vec<String>> {
    let remembered = signal(remembered_on(&services::state::get(), output));
    let output = output.to_string();
    platform_wayland::watch(
        services::state::subscribe,
        move |state: services::state::ShellState| {
            let now = remembered_on(&state, &output);
            if remembered.peek() != now {
                remembered.set(now);
            }
        },
    );
    remembered
}

fn remembered_on(state: &services::state::ShellState, output: &str) -> Vec<String> {
    state.window_order.get(output).cloned().unwrap_or_default()
}

fn on_output(output: Option<&str>, window: &ManagedToplevel) -> bool {
    output.is_none_or(|output| {
        window.outputs.is_empty() || window.outputs.iter().any(|on| on == output)
    })
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Spacing {
    gap: f32,
    pad: f32,
    inset: f32,
    text: f32,
}

impl Spacing {
    fn of(host: &Host) -> Self {
        Self {
            gap: space::xs(),
            pad: space::md(),
            inset: match host.axis {
                Some(_) => host.inset(),
                None => space::sm(),
            },
            text: space::md(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Shape {
    arrangement: Arrangement,
    icon: f32,
    /// How many entries a list has room for; `None` where the strip may run as long as its windows do.
    capacity: Option<usize>,
}

impl Shape {
    fn of(host: &Host, titles: bool, spacing: Spacing) -> Self {
        let extent = host.extent();
        let arrangement = arrangement(extent, host.axis, titles);
        let (icon, capacity) = match host.axis {
            Some(_) if arrangement.column && arrangement.titles => (ROW_ICON, None),
            Some(_) => ((host.icon_size() * 0.8).round(), None),
            None if arrangement.column => {
                let room = extent.height - 2.0 * spacing.pad + spacing.gap;
                (
                    ROW_ICON,
                    Some((room / (ROW + spacing.gap)).floor().max(1.0) as usize),
                )
            }
            None => (ROW_ICON, None),
        };
        Self {
            arrangement,
            icon,
            capacity,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Look {
    shape: Shape,
    empty: bool,
    unsupported: bool,
}

impl Look {
    fn key(&self) -> (Arrangement, u32, Option<usize>, bool, bool) {
        (
            self.shape.arrangement,
            self.shape.icon.to_bits(),
            self.shape.capacity,
            self.empty,
            self.unsupported,
        )
    }
}

struct Strip {
    theme: NordTheme,
    radius: f32,
    spacing: Spacing,
    /// The bar's thickness for a chip; `None` for a widget.
    thickness: Option<f32>,
    key: String,
    output: Option<String>,
    windows: RwSignal<Vec<ManagedToplevel>>,
    shown: Memo<Vec<ManagedToplevel>>,
    reordered: RwSignal<u64>,
    surface: Option<SurfaceRef>,
    spans: Rc<RefCell<HashMap<ManagedToplevelId, Rect>>>,
    dragging: RwSignal<Option<ManagedToplevelId>>,
    /// The window the drag last moved onto, so a move read against the rects of the previous layout does not move it straight back.
    landed: Cell<Option<ManagedToplevelId>>,
    moved: Cell<bool>,
}

impl Strip {
    fn axis(&self, column: bool) -> LayoutStyle {
        let style = match column {
            true => LayoutStyle::new().flex_column(),
            false => LayoutStyle::new().flex_row(),
        };
        let padding = match self.thickness {
            Some(_) => self.spacing.inset,
            None if column => self.spacing.pad,
            None => self.spacing.inset,
        };
        let style = style.gap(self.spacing.gap).padding_all(padding);
        match self.thickness {
            Some(_) => style.align_items(AlignItems::CENTER).min_width(0.0),
            None => style
                .align_items(AlignItems::STRETCH)
                .width(SizeDimension::Percent(1.0))
                .height(SizeDimension::Percent(1.0)),
        }
    }

    fn across(&self) -> Option<f32> {
        self.thickness
            .map(|thickness| (thickness - 2.0 * self.spacing.inset).max(1.0))
    }

    fn drag(&self, id: ManagedToplevelId, column: bool, x: f32, y: f32) {
        let spans = self.spans.borrow();
        let Some(own) = spans.get(&id) else {
            return;
        };
        let point = match column {
            true => own.y + y,
            false => own.x + x,
        };
        let mut ordered: Vec<(ManagedToplevelId, f32, f32)> = spans
            .iter()
            .map(|(window, rect)| match column {
                true => (*window, rect.y, rect.y + rect.height),
                false => (*window, rect.x, rect.x + rect.width),
            })
            .collect();
        drop(spans);
        ordered.sort_by(|a, b| a.1.total_cmp(&b.1));
        self.dragging.set(Some(id));
        match order::landing(&ordered, point) {
            Some(onto) if onto == id => self.landed.set(None),
            Some(onto) if self.landed.get() != Some(onto) => {
                order::hold_moved(&self.key, id, onto);
                self.landed.set(Some(onto));
                self.moved.set(true);
                self.reordered
                    .update(|count| *count = count.wrapping_add(1));
            }
            _ => {}
        }
    }

    fn drop_dragged(&self) {
        self.dragging.set(None);
        self.landed.set(None);
        if !self.moved.replace(false) {
            return;
        }
        let shown = order::held(&self.key, &self.windows.peek());
        let key = self.key.clone();
        services::state::update(move |state| {
            let before = state.window_order.get(&key).cloned().unwrap_or_default();
            state
                .window_order
                .insert(key, order::remembered_after(&shown, &before));
        });
    }

    fn window(&self, id: ManagedToplevelId) -> Option<ManagedToplevel> {
        self.windows
            .peek_with(|windows| windows.iter().find(|window| window.id == id).cloned())
    }

    fn here(&self, window: &ManagedToplevel) -> bool {
        on_output(self.output.as_deref(), window)
    }
}

fn entries(strip: &Rc<Strip>, look: Look) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let column = look.shape.arrangement.column;
    let shown = strip.shown;
    let capacity = look.shape.capacity;
    let built = Rc::clone(strip);
    let list = ReactiveList::with_style(
        strip.axis(column),
        move || {
            let mut windows = shown.get();
            if let Some(capacity) = capacity {
                windows.truncate(capacity);
            }
            windows
        },
        |window: &ManagedToplevel| window.id,
        move |window: ManagedToplevel| entry(&built, window.id, look),
    )?;
    Ok(Box::new(list))
}

fn label(window: &ManagedToplevel) -> String {
    match window.title.is_empty() {
        true => window.app_id.clone(),
        false => window.title.clone(),
    }
}

fn entry(
    strip: &Rc<Strip>,
    id: ManagedToplevelId,
    look: Look,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let Look { shape, .. } = look;
    let Arrangement { column, titles } = shape.arrangement;
    let theme = strip.theme;
    let shown = strip.shown;
    let live = memo(move || shown.with(|windows| windows.iter().find(|w| w.id == id).cloned()));
    let app = strip
        .window(id)
        .map(|window| window.app_id)
        .unwrap_or_default();

    let mut content: Vec<Box<dyn LayoutItem>> = Vec::with_capacity(2);
    content.push(match app_icon_view(&app, shape.icon)? {
        Some(icon) => icon,
        None => icon_view(|| GLYPH.to_string(), move || theme.text, shape.icon)?,
    });
    if titles {
        let title = Text::declaring(
            move || live.get().as_ref().map(label).unwrap_or_default(),
            LayoutStyle::new().min_width(0.0).flex_shrink(1.0),
            move |inherited| {
                theme
                    .text_over(inherited, FontRole::Caption, theme.text)
                    .with_clamp(1, true)
            },
        )?;
        content.push(box_item(title));
    }

    let style = entry_style(strip, column, titles, shape.icon);
    let (rest, active) = fills(theme);
    let hover = hover_fill(theme);
    let radius = strip.radius;
    let focused = move || live.with(|window| window.as_ref().is_some_and(|w| w.activated));
    let dragging = strip.dragging;
    let on_press = Rc::clone(strip);
    let on_drag = Rc::clone(strip);
    let on_drop = Rc::clone(strip);
    let item = StyledContainer::new(
        style,
        move |_| RectStyle::filled(if focused() { active } else { rest }, radius),
        content,
    )?
    .hover_style(move |_| RectStyle::filled(if focused() { active } else { hover }, radius))
    .with_opacity(move || match dragging.get() == Some(id) {
        true => DRAGGED_OPACITY,
        false => 1.0,
    })
    .on_press(move || {
        if let Some(window) = on_press.window(id) {
            services::windows::activate(&window);
        }
    })
    .drag_threshold(DRAG_SLOP)
    .drag_axis(match column {
        true => DragAxis::Vertical,
        false => DragAxis::Horizontal,
    })
    .on_drag(move |x, y| on_drag.drag(id, column, x, y))
    .on_drag_end(move |_, _| on_drop.drop_dragged());

    track(strip, id, item.layout_node(), live);
    Ok(Box::new(item))
}

fn entry_style(strip: &Strip, column: bool, titles: bool, icon: f32) -> LayoutStyle {
    let style = LayoutStyle::new()
        .flex_row()
        .align_items(AlignItems::CENTER)
        .gap(strip.spacing.text);
    let style = match titles {
        true => style
            .justify_content(JustifyContent::START)
            .padding_horizontal(strip.spacing.text),
        false => style.justify_content(JustifyContent::CENTER),
    };
    match (strip.across(), column) {
        (Some(across), false) => {
            let style = style.height(across).min_width(across).flex_shrink(1.0);
            match titles {
                true => style.max_width(TITLE_MAX),
                false => style.width(across).flex_shrink(0.0),
            }
        }
        (Some(across), true) => style
            .width(across)
            .height(if titles { ROW } else { across })
            .flex_shrink(0.0),
        (None, true) => style.height(ROW).flex_shrink(0.0),
        (None, false) => style
            .flex_grow(1.0)
            .flex_shrink(1.0)
            .flex_basis(0.0)
            .min_width(icon + 2.0 * strip.spacing.inset),
    }
}

fn track(
    strip: &Rc<Strip>,
    id: ManagedToplevelId,
    node: telar::NodeId,
    live: Memo<Option<ManagedToplevel>>,
) {
    let Some(rect) = track_layout(node) else {
        return;
    };
    let spans = Rc::clone(&strip.spans);
    effect(move || {
        spans.borrow_mut().insert(id, rect.get());
    });
    let spans = Rc::clone(&strip.spans);
    on_cleanup(move || {
        spans.borrow_mut().remove(&id);
    });

    let Some(surface) = strip.surface.clone() else {
        return;
    };
    let holding = Rc::new(Holding::claim(surface, id));
    let here = Rc::clone(strip);
    let sending = Rc::clone(&holding);
    effect(move || {
        let laid = rect.get();
        let area = live
            .get()
            .filter(|window| here.here(window))
            .and_then(|_| ToplevelArea::covering(laid));
        sending.send(area);
    });
    on_cleanup(move || holding.release());
}

fn empty(strip: &Rc<Strip>, look: Look) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let theme = strip.theme;
    let muted = theme.muted;
    match strip.across() {
        Some(across) => {
            let glyph = icon_view(|| GLYPH.to_string(), move || muted, look.shape.icon)?;
            Ok(Box::new(StyledContainer::new(
                LayoutStyle::new()
                    .flex_row()
                    .align_items(AlignItems::CENTER)
                    .justify_content(JustifyContent::CENTER)
                    .width(across + 2.0 * strip.spacing.inset)
                    .height(across + 2.0 * strip.spacing.inset)
                    .flex_shrink(0.0),
                |_| RectStyle::default(),
                vec![glyph],
            )?))
        }
        None => {
            let glyph = icon_view(|| GLYPH.to_string(), move || muted, ROW_ICON)?;
            let unsupported = look.unsupported;
            let reason = Text::declaring(
                move || match unsupported {
                    true => telar::t!("windows.unsupported"),
                    false => telar::t!("windows.none"),
                },
                LayoutStyle::new(),
                move |inherited| {
                    theme
                        .text_over(inherited, FontRole::Caption, muted)
                        .with_clamp(2, true)
                },
            )?;
            Ok(Box::new(StyledContainer::new(
                LayoutStyle::new()
                    .flex_column()
                    .align_items(AlignItems::CENTER)
                    .justify_content(JustifyContent::CENTER)
                    .gap(strip.spacing.text)
                    .padding_all(strip.spacing.pad)
                    .width(SizeDimension::Percent(1.0))
                    .height(SizeDimension::Percent(1.0)),
                |_| RectStyle::default(),
                vec![glyph, box_item(reason)],
            )?))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use telar::{
        AvailableSpace, Event, PointerButton, PointerSource, compute_layout, new_container,
        reset_layout_runtime, set_theme,
    };

    use config::{Config, Edge};
    use ui::host::{Instance, Representation, Size, WidgetSize};

    use super::*;

    fn open() -> Vec<ManagedToplevel> {
        [(1, "kitty"), (2, "firefox"), (3, "code")]
            .into_iter()
            .map(|(id, app)| ManagedToplevel {
                id: ManagedToplevelId::from_raw(id),
                app_id: app.to_string(),
                title: app.to_string(),
                ..ManagedToplevel::default()
            })
            .collect()
    }

    fn press(item: &mut dyn LayoutItem, at: (f32, f32), to: (f32, f32)) {
        item.on_event(&Event::PointerPressed {
            x: f64::from(at.0),
            y: f64::from(at.1),
            button: PointerButton::Primary,
            source: PointerSource::Mouse,
        });
        item.on_event(&Event::PointerMoved {
            x: f64::from(to.0),
            y: f64::from(to.1),
            source: PointerSource::Mouse,
        });
        item.on_event(&Event::PointerReleased {
            x: f64::from(to.0),
            y: f64::from(to.1),
            button: PointerButton::Primary,
            source: PointerSource::Mouse,
        });
    }

    fn dragged(host: &Host, size: (f32, f32), at: (f32, f32), to: (f32, f32)) -> Vec<u32> {
        reset_layout_runtime();
        set_theme(NordTheme::new());
        let scope = telar::owner_scope();
        let owner = scope.id();
        let windows = signal(open());
        let mut item = telar::batch(|| strip_over(host, windows)).expect("the strip builds");
        let root = new_container(
            LayoutStyle::new()
                .flex_column()
                .width(size.0)
                .height(size.1),
            &[item.layout_node()],
        )
        .expect("a root");
        let lay_out = || {
            compute_layout(
                root,
                AvailableSpace::Definite(size.0),
                AvailableSpace::Definite(size.1),
            )
            .expect("the strip lays out")
        };
        lay_out();
        telar::batch(|| press(item.as_mut(), at, to));
        lay_out();
        let key = host.output.clone().unwrap_or_default();
        let held = order::arrange(&key, &windows.peek(), &[]);
        drop(scope);
        telar::dispose_owner(owner);
        held.iter().map(|window| window.id.raw()).collect()
    }

    fn widget(output: &str) -> Host {
        let config = Arc::new(Config::starter());
        let theme = config.resolve_theme();
        let shape = config.shape_from(None, None, None, None);
        Host::placed(
            Instance::of_module("windows"),
            config,
            Representation::Widget(WidgetSize::M),
            WidgetSize::M.extent(),
            None,
            shape,
            theme.accent,
            theme.text,
            Some(output.to_string()),
        )
    }

    fn bar(edge: Edge) -> Host {
        let config = Arc::new(Config::starter());
        let theme = config.resolve_theme();
        Host::chip(
            Instance::of_module("windows"),
            config,
            edge,
            34.0,
            theme.accent,
            theme.text,
            Some(format!("bar-{edge:?}")),
        )
    }

    fn row(index: f32) -> f32 {
        space::md() + index * (ROW + space::xs()) + ROW / 2.0
    }

    fn listed() -> (f32, f32) {
        let Size { width, height } = WidgetSize::M.extent();
        (width, height)
    }

    #[test]
    fn dragging_an_entry_down_a_list_puts_it_where_it_was_dropped() {
        assert_eq!(
            dragged(
                &widget("list"),
                listed(),
                (40.0, row(0.0)),
                (40.0, row(2.0))
            ),
            vec![2, 3, 1]
        );
    }

    #[test]
    fn dragging_an_entry_onto_its_neighbour_swaps_the_two() {
        assert_eq!(
            dragged(
                &widget("neighbour"),
                listed(),
                (40.0, row(1.0)),
                (40.0, row(0.0))
            ),
            vec![2, 1, 3]
        );
    }

    #[test]
    fn a_press_that_does_not_travel_moves_nothing() {
        assert_eq!(
            dragged(
                &widget("press"),
                listed(),
                (40.0, row(0.0)),
                (41.0, row(0.0) + 1.0)
            ),
            vec![1, 2, 3]
        );
        assert_eq!(
            services::state::get().window_order.get("press"),
            None,
            "a press is not a drag, so it leaves nothing to remember"
        );
    }

    #[test]
    fn dragging_along_a_bar_reorders_along_its_own_axis_on_every_edge() {
        for edge in Edge::ALL {
            let across = 34.0 / 2.0;
            let (size, at, to) = match edge.is_vertical() {
                true => ((34.0, 600.0), (across, 10.0), (across, 590.0)),
                false => ((600.0, 34.0), (10.0, across), (590.0, across)),
            };
            assert_eq!(dragged(&bar(edge), size, at, to), vec![2, 3, 1], "{edge:?}");
        }
    }

    #[test]
    fn the_strip_remembers_a_drag_by_application() {
        dragged(
            &widget("remember"),
            listed(),
            (40.0, row(0.0)),
            (40.0, row(2.0)),
        );
        assert_eq!(
            services::state::get().window_order.get("remember").cloned(),
            Some(vec![
                "firefox".to_string(),
                "code".to_string(),
                "kitty".to_string()
            ])
        );
    }

    fn named(count: u32) -> Vec<ManagedToplevel> {
        (1..=count)
            .map(|id| ManagedToplevel {
                id: ManagedToplevelId::from_raw(id),
                app_id: format!("app{id}"),
                title: format!("Window {id}"),
                ..ManagedToplevel::default()
            })
            .collect()
    }

    fn written(host: &Host, size: (f32, f32), windows: Vec<ManagedToplevel>) -> Vec<String> {
        reset_layout_runtime();
        set_theme(NordTheme::new());
        let scope = telar::owner_scope();
        let owner = scope.id();
        let item = telar::batch(|| strip_over(host, signal(windows))).expect("the strip builds");
        let page = telar::Container::new(
            LayoutStyle::new()
                .flex_column()
                .width(size.0)
                .height(size.1),
            vec![item],
        )
        .expect("a page");
        let root = page.layout_node();
        let tree = telar::ComponentList::new(page);
        compute_layout(
            root,
            AvailableSpace::Definite(size.0),
            AvailableSpace::Definite(size.1),
        )
        .expect("the strip lays out");
        let texts = tree
            .commands()
            .iter()
            .filter_map(|command| match command {
                telar::DrawCommand::Text { text, .. } => Some(text.to_string()),
                _ => None,
            })
            .collect();
        drop(tree);
        drop(scope);
        telar::dispose_owner(owner);
        texts
    }

    fn without_titles(host: impl FnOnce(Arc<Config>) -> Host) -> Host {
        let mut config = Config::starter();
        config.windows.titles = false;
        host(Arc::new(config))
    }

    #[test]
    fn an_entry_is_named_by_its_title_else_by_its_application() {
        let window = |app: &str, title: &str| ManagedToplevel {
            app_id: app.to_string(),
            title: title.to_string(),
            ..ManagedToplevel::default()
        };
        assert_eq!(label(&window("kitty", "nvim")), "nvim");
        assert_eq!(label(&window("kitty", "")), "kitty");
    }

    #[test]
    fn a_list_writes_each_title_and_a_bar_along_the_top_does_too() {
        assert_eq!(
            written(&widget("titles-list"), listed(), named(3)),
            ["Window 1", "Window 2", "Window 3"]
        );
        assert_eq!(
            written(&bar(Edge::Top), (600.0, 34.0), named(2)),
            ["Window 1", "Window 2"]
        );
    }

    #[test]
    fn titles_off_leaves_the_icons_alone_and_a_vertical_bar_never_writes_them() {
        let quiet = without_titles(|config| {
            let theme = config.resolve_theme();
            let shape = config.shape_from(None, None, None, None);
            Host::placed(
                Instance::of_module("windows"),
                config,
                Representation::Widget(WidgetSize::M),
                WidgetSize::M.extent(),
                None,
                shape,
                theme.accent,
                theme.text,
                Some("quiet".to_string()),
            )
        });
        assert!(written(&quiet, listed(), named(3)).is_empty());
        assert!(
            written(&bar(Edge::Left), (34.0, 600.0), named(3)).is_empty(),
            "a bar of icon width has no room for a title"
        );
    }

    #[test]
    fn a_list_holds_only_the_entries_it_has_room_for_and_a_bar_holds_them_all() {
        let shown = written(&widget("capacity"), listed(), named(12));
        assert!(!shown.is_empty() && shown.len() < 12, "{shown:?}");
        let expected: Vec<String> = (1..=shown.len()).map(|n| format!("Window {n}")).collect();
        assert_eq!(shown, expected, "the first ones, in order");
        assert_eq!(
            written(&bar(Edge::Top), (2000.0, 34.0), named(12)).len(),
            12,
            "a bar runs as long as its windows do"
        );
    }

    #[test]
    fn with_no_window_a_list_says_so_and_a_bar_keeps_one_glyph() {
        let said = written(&widget("nobody"), listed(), Vec::new());
        assert_eq!(said.len(), 1, "{said:?}");
        assert!(!said[0].is_empty());
        assert!(written(&bar(Edge::Top), (600.0, 34.0), Vec::new()).is_empty());
    }
}
