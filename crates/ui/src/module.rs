use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use telar::{Color, LayoutError, LayoutItem, ReadSignal, Rect, signal};

use config::theme::NordTheme;
use config::{Edge, Variant};

pub use config::{SurfaceEnv, set_surface_env, surface_env};

pub fn bar_edge() -> Edge {
    surface_env().map(|e| e.edge).unwrap_or(Edge::Top)
}

thread_local! {
    static PRESSED_CHIP: Cell<Option<Rect>> = const { Cell::new(None) };
}

/// Runs `act` — a chip's press or drag-open handler — with the chip's own laid-out rect in scope.
///
/// The rect is what a drawer hangs off, on the same terms as the card a hover opens over the same chip, and only the chip knows it. What travelled here before was the *zone* the chip sat in, which could say no more than which end of the bar to align to — and could not always say that: an id placed in more than one zone resolves to whichever the config search reaches first, and a `[corners]` module sits in no zone at all despite being laid out at a very definite end of its bar. A rect answers both without asking the config anything.
///
/// Ambient rather than a parameter because the handler that reads it may be a `ModuleClick::Action` — a bare `fn()` that opens someone else's panel — which no signature change reaches. Scoped strictly to the synchronous dispatch, so nothing can read a stale rect afterwards.
pub fn from_chip<R>(chip: Rect, act: impl FnOnce() -> R) -> R {
    let previous = PRESSED_CHIP.with(|pressed| pressed.replace(Some(chip)));
    let done = act();
    PRESSED_CHIP.with(|pressed| pressed.set(previous));
    done
}

/// The rect of the chip whose press is being dispatched, if a press is what is running. `None` for a panel reached from anywhere else — IPC, a keybind — where there is no chip to hang off.
pub fn pressed_chip() -> Option<Rect> {
    PRESSED_CHIP.with(|pressed| pressed.get())
}

/// How a chip opens its module's panel.
type PanelOpener = Box<dyn Fn(&str)>;

thread_local! {
    // How a chip opens its module's panel. Installed at startup, because *which* surface a module id opens is the shell's routing rather than the chip's: a chip knows it was dragged away from the bar and nothing more.
    static OPEN_PANEL: RefCell<Option<PanelOpener>> = const { RefCell::new(None) };
}

/// Registers how a module id is turned into an open panel. Set once at startup by whoever owns the routing.
pub fn set_panel_opener(open: impl Fn(&str) + 'static) {
    OPEN_PANEL.with(|hook| *hook.borrow_mut() = Some(Box::new(open)));
}

pub(crate) fn open_panel(module: &str) {
    OPEN_PANEL.with(|hook| {
        if let Some(open) = hook.borrow().as_ref() {
            open(module);
        }
    });
}

pub fn bar_is_vertical() -> bool {
    bar_edge().is_vertical()
}

#[derive(Clone, Copy)]
pub struct ModuleCtx {
    pub theme: NordTheme,
    pub accent: Color,
    /// The bar's thickness in px (height for top/bottom, width for left/right).
    pub bar_size: u32,
    pub edge: Edge,
}

thread_local! {
    // The bar sets this per module, just before building that module's content.
    static MODULE_FG: RefCell<Color> = RefCell::new(NordTheme::new().text);
}

pub fn set_module_fg(color: Color) {
    MODULE_FG.with(|c| *c.borrow_mut() = color);
}

/// Snapshot of the current module's foreground for a `.rsx` module to bind as `color:$fg`; must be called once at build time so each module captures its OWN color, not the last-set one.
pub fn module_fg() -> ReadSignal<Color> {
    signal(MODULE_FG.with(|c| *c.borrow())).read_only()
}

/// The bar's thickness in px, or a sane default outside a surface; everything a module sizes derives from this, so a thin bar yields a small, proportional chip instead of an oversized one that squashes.
pub fn bar_thickness() -> f32 {
    surface_env().map(|e| e.bar_size).unwrap_or(34) as f32
}

/// Icon size for the current bar: ~0.75 of its thickness, so the glyph fills most of its square chip and scales with the bar.
pub fn icon_px() -> f32 {
    (bar_thickness() * 0.75).round().clamp(8.0, 64.0)
}

/// The resolved chip corner radius for this bar (per-bar → `[shape]` → theme), so a self-managed module's inner elements (e.g. workspace pills) round like the sibling chips instead of a hardcoded value.
pub fn chip_radius() -> f32 {
    surface_env()
        .map(|e| e.config.shape_for(e.edge).chip_radius())
        .unwrap_or(0.0)
}

/// Chosen so the chip's width (icon ≈ 0.75·thickness + two of these ≈ 0.25·thickness) equals the bar thickness, so a chip stretched to the bar's height comes out square.
pub fn chip_pad() -> f32 {
    (bar_thickness() * 0.125).round().max(1.0)
}

/// The foreground for a container variant: the plain text token when blending into the bar (default), or the higher-contrast of text/base over the accent when filled.
pub fn module_foreground(variant: Variant, accent: Color, theme: NordTheme) -> Color {
    match variant {
        Variant::Default => theme.text,
        Variant::Filled => accent.most_readable(&[theme.text, theme.base]),
    }
}

/// Shared by the chip shell and the bar's wrapper around a self-managed module, so the two never paint a variant differently.
pub fn resting_fill(variant: Variant, rest: Color, accent: Color) -> Color {
    match variant {
        Variant::Default => rest,
        Variant::Filled => accent,
    }
}

/// A chip's drag-to-open gesture: pulling it away from the bar opens the panel it would otherwise toggle.
#[derive(Clone)]
pub struct DragOpen {
    pub module: String,
    /// The bar's edge, which is what says which direction "away from the bar" is.
    pub edge: Edge,
    /// How far the pointer must travel inwards before letting go opens the panel, in px.
    pub threshold: f32,
}

impl DragOpen {
    /// How far a drag has travelled *away from the bar*, from a press at `from` to a pointer now at `to`. Negative is back towards the bar, which is the direction that closes rather than opens.
    pub(crate) fn travel(&self, from: (f32, f32), to: (f32, f32)) -> f32 {
        match self.edge {
            Edge::Top => to.1 - from.1,
            Edge::Bottom => from.1 - to.1,
            Edge::Left => to.0 - from.0,
            Edge::Right => from.0 - to.0,
        }
    }
}

pub type ModuleBuilder = fn(&ModuleCtx) -> Result<Box<dyn LayoutItem>, LayoutError>;

/// What clicking a module does: `Panel` toggles its panel (drawer or float, per the module's `open` config); `Action` runs a custom handler.
#[derive(Clone, Copy)]
pub enum ModuleClick {
    Panel,
    Action(fn()),
}

pub struct ModuleDef {
    pub builder: ModuleBuilder,
    /// If true, the bar places the module bare instead of wrapping it in [`crate::module_shell::module_shell`] (e.g. the workspaces grid).
    pub self_managed: bool,
    /// If true, the container is a square chip that scales with the bar instead of a content-width text pill.
    pub icon: bool,
    /// What clicking the module does; `None` is a display-only chip.
    pub click: Option<ModuleClick>,
    /// What the wheel does over the module, as `(dx, dy)` in pixels; `None` leaves the chip inert to scroll.
    pub scroll: Option<fn(f32, f32)>,
    /// Whether resting the pointer on the chip opens its hover popout. Set from `popout::has_popout` rather than declared twice, so a module can't be wired for a card it has no content for.
    pub popout: bool,
    /// Whether this chip gives up width when its zone runs short, instead of holding its content width like every other one. For the chips whose text has no natural length — a window title, a track name — which are the reason a zone runs short in the first place. Their label elides, so what they lose is the tail of a string rather than anything a reader needs; a chip that gave up width without eliding would just hide its own end.
    pub elastic: bool,
    pub filler: bool,
}

impl ModuleDef {
    pub fn new(builder: ModuleBuilder) -> Self {
        Self {
            builder,
            self_managed: false,
            filler: false,
            icon: false,
            click: None,
            scroll: None,
            popout: false,
            elastic: false,
        }
    }

    pub fn icon(mut self) -> Self {
        self.icon = true;
        self
    }

    pub fn opens(mut self) -> Self {
        self.click = Some(ModuleClick::Panel);
        self
    }

    pub fn on_click(mut self, action: fn()) -> Self {
        self.click = Some(ModuleClick::Action(action));
        self
    }

    /// Wires the wheel over this chip to `action`, receiving the scroll delta in pixels (positive `dy` is a scroll up). Used by the level modules so the chip is a control, not just a readout.
    pub fn on_scroll(mut self, action: fn(f32, f32)) -> Self {
        self.scroll = Some(action);
        self
    }

    pub fn self_managed(mut self) -> Self {
        self.self_managed = true;
        self
    }

    /// Also self-managed: a filler is room rather than a chip, so it is placed bare and rests on no surface even on a chip bar.
    pub fn filler(mut self) -> Self {
        self.self_managed = true;
        self.filler = true;
        self
    }

    /// Lets this chip shrink when its zone is short of room, eliding its label rather than holding a width nothing else can give back.
    pub fn elastic(mut self) -> Self {
        self.elastic = true;
        self
    }
}

#[derive(Default)]
pub struct ModuleRegistry {
    modules: HashMap<String, ModuleDef>,
}

impl ModuleRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, id: &str, def: ModuleDef) {
        self.modules.insert(id.to_string(), def);
    }

    pub fn def(&self, id: &str) -> Option<&ModuleDef> {
        self.modules.get(id)
    }

    /// Every registered id, sorted. What the settings application's per-module overrides enumerate, so a chip can be restyled before it has been put on a bar.
    pub fn ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.modules.keys().cloned().collect();
        ids.sort_unstable();
        ids
    }

    /// Marks every registered module the popout layer has card content for. Driven off that list rather than declared per module, so the two cannot drift into a chip that opens an empty card.
    pub fn wire_popouts(&mut self, has_popout: impl Fn(&str) -> bool) {
        for (id, def) in self.modules.iter_mut() {
            def.popout = has_popout(id);
        }
    }

    /// Every registered module. What lets a test walk the table and check the roles against the routing it is supposed to match.
    pub fn iter(&self) -> impl Iterator<Item = (&String, &ModuleDef)> {
        self.modules.iter()
    }

    pub fn build(
        &self,
        id: &str,
        ctx: &ModuleCtx,
    ) -> Option<Result<Box<dyn LayoutItem>, LayoutError>> {
        self.modules.get(id).map(|d| (d.builder)(ctx))
    }
}

thread_local! {
    static LIVE: RefCell<Option<ModuleRegistry>> = const { RefCell::new(None) };
}

/// Publishes the module vocabulary every bar builds from, and that the settings application enumerates. Set once at startup by whoever owns the module list — the same arrangement as [`crate::panels`], so neither the bar nor a panel that lists modules has to name one.
pub fn install(registry: ModuleRegistry) {
    LIVE.with(|live| *live.borrow_mut() = Some(registry));
}

/// Runs `act` against the installed registry, or against an empty one when nothing has been installed — a bar with no modules rather than a panic, which is what a test that never composed a shell should see.
pub fn with_registry<R>(act: impl FnOnce(&ModuleRegistry) -> R) -> R {
    LIVE.with(|live| match live.borrow().as_ref() {
        Some(registry) => act(registry),
        None => act(&ModuleRegistry::new()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_drag_opens_a_panel_only_when_it_pulls_away_from_the_bar() {
        let gesture = |edge| DragOpen {
            module: "clock".to_string(),
            edge,
            threshold: 48.0,
        };
        // "Away from the bar" is a different direction on each edge, and the sign is what decides.
        assert_eq!(gesture(Edge::Top).travel((10.0, 5.0), (10.0, 65.0)), 60.0);
        assert_eq!(
            gesture(Edge::Bottom).travel((10.0, 65.0), (10.0, 5.0)),
            60.0
        );
        assert_eq!(gesture(Edge::Left).travel((5.0, 10.0), (65.0, 10.0)), 60.0);
        assert_eq!(gesture(Edge::Right).travel((65.0, 10.0), (5.0, 10.0)), 60.0);

        assert!(gesture(Edge::Top).travel((10.0, 65.0), (10.0, 5.0)) < 0.0);
        assert_eq!(gesture(Edge::Top).travel((10.0, 20.0), (300.0, 20.0)), 0.0);
    }

    /// An elastic chip hands back the width a cramped bar needs; every other chip keeps its own.
    ///
    /// This is what makes an eliding label mean anything: a title only ends in `…` when something narrowed the box it is drawn in, and a chip that holds its content width is never narrowed. The other half — that a clamped label ends in an ellipsis rather than being cut mid-glyph — is telar's, and tested there.
    #[test]
    fn an_elastic_chip_yields_width_and_a_plain_one_holds_it() {
        use crate::module_shell::{ModuleShellProps, module_shell};
        use telar::{
            AvailableSpace, Container, LayoutStyle, Slots, compute_layout, reset_layout_runtime,
            set_theme, track_layout,
        };

        const ROOM: f32 = 120.0;
        const LABEL: f32 = 300.0;

        let measure = |elastic: bool| {
            reset_layout_runtime();
            set_theme(NordTheme::new());
            let label = Container::new(LayoutStyle::new().width(LABEL).height(20.0), vec![])
                .expect("the label builds");
            let mut inner = Slots::new();
            inner.push(None, Box::new(label) as Box<dyn LayoutItem>);
            let chip = module_shell(
                ModuleShellProps::props().elastic(elastic).build(),
                telar::Children::new({
                    let inner = std::cell::RefCell::new(Some(inner));
                    move || {
                        inner.borrow_mut().take().ok_or_else(|| {
                            telar::LayoutError::Engine("chip children built twice".into())
                        })
                    }
                }),
            )
            .expect("the chip builds");
            let rect = track_layout(chip.layout_node()).expect("the chip registers its rect");
            let row = Container::new(
                LayoutStyle::new().flex_row().width(ROOM).height(32.0),
                vec![chip],
            )
            .expect("the row builds");
            compute_layout(
                row.layout_node(),
                AvailableSpace::Definite(ROOM),
                AvailableSpace::Definite(32.0),
            )
            .expect("the row lays out");
            rect.get().width
        };

        assert!(
            measure(true) <= ROOM,
            "an elastic chip asked for {LABEL}px in {ROOM}px of bar took {}px — it has to give the room \
             back and let its label elide",
            measure(true)
        );
        assert!(
            measure(false) > ROOM,
            "and a plain chip must still hold its content width, or every readout on the bar would be \
             squeezed by whichever neighbour ran long"
        );
    }

    /// The resting fill once read the theme's `base` token instead of the chip's own, which painted a block behind every chip on a whole bar.
    #[test]
    fn a_resting_chip_paints_its_rest_or_its_accent_and_nothing_else() {
        use crate::module_shell::{ModuleShellProps, module_shell};
        use telar::{
            AvailableSpace, ComponentList, Container, DrawCommand, LayoutStyle, Paint, Slots,
            compute_layout, reset_layout_runtime, set_theme,
        };

        let fills = |variant: Variant, rest: Color, accent: Color| {
            reset_layout_runtime();
            set_theme(NordTheme::new());
            let label = Container::new(LayoutStyle::new().width(20.0).height(20.0), vec![])
                .expect("the label builds");
            let mut inner = Slots::new();
            inner.push(None, Box::new(label) as Box<dyn LayoutItem>);
            let chip = module_shell(
                ModuleShellProps::props()
                    .variant(variant)
                    .rest(rest)
                    .accent(accent)
                    .build(),
                telar::Children::new({
                    let inner = RefCell::new(Some(inner));
                    move || {
                        inner
                            .borrow_mut()
                            .take()
                            .ok_or_else(|| LayoutError::Engine("chip children built twice".into()))
                    }
                }),
            )
            .expect("the chip builds");
            let page = Container::new(
                LayoutStyle::new().flex_row().width(64.0).height(32.0),
                vec![chip],
            )
            .expect("the row builds");
            let root = page.layout_node();
            let tree = ComponentList::new(page);
            compute_layout(
                root,
                AvailableSpace::Definite(64.0),
                AvailableSpace::Definite(32.0),
            )
            .expect("the row lays out");
            tree.commands()
                .iter()
                .filter_map(|command| match command {
                    DrawCommand::Rect { style, .. } => style.fill,
                    _ => None,
                })
                .collect::<Vec<Paint>>()
        };

        let theme = NordTheme::new();
        let blending = fills(Variant::Default, Color::TRANSPARENT, theme.accent);
        assert!(
            blending
                .iter()
                .all(|fill| *fill == Paint::Solid(Color::TRANSPARENT)),
            "a chip blending into its bar painted a background of its own: {blending:?}"
        );
        assert_eq!(
            fills(Variant::Default, theme.surface, theme.accent),
            vec![Paint::Solid(theme.surface)],
            "a free-standing chip rests on the surface its bar hands it"
        );
        assert_eq!(
            fills(Variant::Filled, Color::TRANSPARENT, theme.orange),
            vec![Paint::Solid(theme.orange)],
            "a filled chip rests on its accent"
        );
    }

    #[test]
    fn module_foreground_default_is_text_filled_is_contrast() {
        let theme = NordTheme::new();
        assert_eq!(
            module_foreground(Variant::Default, theme.orange, theme),
            theme.text,
            "default variant paints with the plain text token"
        );
        let filled = module_foreground(Variant::Filled, theme.orange, theme);
        assert!(
            filled == theme.text || filled == theme.base,
            "filled foreground is one of the two theme foregrounds"
        );
        assert_eq!(
            filled, theme.base,
            "over the light orange accent, the dark base wins the contrast"
        );
    }
}
