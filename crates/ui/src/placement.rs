//! Where a surface sits, as one vocabulary.
//!
//! Every window this shell puts on screen used to describe itself: five modules built a whole `LayerConfig` by hand, three more went through the surface host's own placement type, and each named its layer-shell namespace in a string literal of its own. The result was four ways to say "a panel anchored to an edge" and no way to tell, from one of them, what the others had decided.
//!
//! There is one way here. A [`Placement`] is built from a **named primitive** — the shape the surface takes on screen — and adjusted with the handful of modifiers a surface actually varies: its size, its margin, how much of the keyboard it wants, what it does with the pointer. The primitives are the shell's whole taxonomy of windows:
//!
//! | Primitive | The shape | Who takes it |
//! | --- | --- | --- |
//! | [`bar`](Placement::bar) | spans an edge, reserves nothing itself | the bars |
//! | [`reservation`](Placement::reservation) | invisible, carves an edge's strip | the bars' strips |
//! | [`backdrop`](Placement::backdrop) | the whole screen, click-through, reserving nothing | wallpaper, frame ring |
//! | [`desktop`](Placement::desktop) | what the bars left free, click-through | the desktop widgets |
//! | [`dock`](Placement::dock) | spans an edge, over the windows | the notification centre |
//! | [`stack`](Placement::stack) | pinned to a spot along an edge, input from its content | toasts, notification popups |
//! | [`off_chip`](Placement::off_chip) | hangs off the chip that opened it | popouts, drawers, the tray menu |
//! | [`centred`](Placement::centred) | a window in the middle of the screen | a module's float, the launcher |
//! | [`screen`](Placement::screen) | the whole screen, over everything, takes the keyboard | the region picker |
//!
//! Two of those rows used to be four. A popout, a drawer and the tray's menu are one *position* — the chip's rect and the bar's edge — asked for by three callers, and describing them apart is what let the menu be built as a card that had been made dismissable, which quietly turned it into a drawer built through the wrong door and announcing a namespace nobody chose. A float and the launcher are likewise one shape, differing in how they go away. What actually varies in each pair is [`OffChip`] and [`Centred`], and those say what the surface *is*, not where it goes.
//!
//! **The namespace belongs to the shape, not to the surface.** It is what a compositor rule matches (`layer_rule = blur, hogar-shell-drawer`), so it is a public interface: the strings below are the ones hogar-shell has always announced, and a primitive owns its own rather than each call site spelling it out.
//!
//! A placement is lowered at the point of use: to a [`LayerConfig`] for a surface that owns its rendering, or to the surface host's [`SurfacePlacement`] for one that wants the scaffold — a scrim, a dismiss-on-outside, an entrance. Which of the two a surface needs is not a property of where it sits, so it is not decided here; see the surface reconciler.

use platform_wayland::{
    Anchor, KeyboardInteractivity, KeyboardMode, Layer, LayerConfig, SurfaceAlign, SurfaceAnchor,
    SurfacePlacement, SurfaceRole, SurfaceSize,
};
use telar::{AlignItems, JustifyContent, LayoutStyle, Rect, SizeDimension};

use config::SurfaceEnv;
use config::{Align, Edge};

/// What a surface does with the pointer.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Input {
    /// Everything inside the surface takes the pointer, which is what a panel wants.
    #[default]
    Solid,
    /// Nothing does: clicks go through to whatever is behind. A wallpaper, a frame ring, an OSD.
    Transparent,
    /// Only where the content actually draws something pressable, recomputed as it changes — a stack of cards with gaps between them, where the gaps belong to the window underneath.
    FromContent,
}

/// Which of the two things that hang off a chip this is.
///
/// They share a position and nothing else, so what this picks is the *kind of surface*: one is opened by resting a pointer and one by pressing, and everything below follows from that.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OffChip {
    /// A reading the pointer opened by resting on the chip. It renders itself, at the size of the tallest card it may be, and carves its input region out of what it actually draws — so the surplus around a short card belongs to the window underneath. There is no way to dismiss it because there was no press to undo.
    Card,
    /// A panel a press opened: sized to its content, dismissed by a press outside it, scaffolded and slid in by the surface host. A module's drawer and the tray's context menu are the same surface asked for twice.
    Panel,
}

impl OffChip {
    fn namespace(self) -> &'static str {
        match self {
            OffChip::Card => "hogar-shell-popout",
            OffChip::Panel => "hogar-shell-drawer",
        }
    }
}

/// Which of the two windows that open in the middle of the screen this is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Centred {
    /// A module's panel as a window of its own, with a title bar, a ✕ and a resize grip. It goes when the user closes it and at no other time, which is the whole reason to choose it over a drawer.
    Float,
    /// A window that takes the screen behind it too: the launcher. A press anywhere outside closes it.
    Modal,
}

impl Centred {
    fn role(self) -> SurfaceRole {
        match self {
            Centred::Float => SurfaceRole::Float,
            Centred::Modal => SurfaceRole::Overlay,
        }
    }

    /// The namespace this shape has always announced. Kept in step with [`role`](Self::role) by hand because a hosted surface is lowered by the surface host, which derives the namespace from the role and never sees this — the two disagreeing is what made the tray's menu announce a name nobody chose.
    fn namespace(self) -> &'static str {
        match self {
            Centred::Float => "hogar-shell-float",
            Centred::Modal => "hogar-shell-overlay",
        }
    }

    /// **A modal asks for the keyboard on demand, not exclusively, and that is about the *pointer*.** An exclusive layer surface is an input grab: the compositor stops delivering pointer events to every other surface while it is up, so with the launcher open the bar went dead — no chip highlighted, no popout opened, and a press on a chip reached the launcher's own scaffold and dismissed it instead of opening that chip's panel. On demand is the same keyboard for a surface that maps focused, without taking the pointer from the rest of the shell. The grab is still right for the region picker, which is selecting an area of the screen and must not have the bar answering clicks inside it — see [`Placement::screen`].
    ///
    /// A float starts at none and is told what its module needs ([`Placement::keyboard`]): a layer surface granted focus takes it from the window the user was in and gives it back when the panel closes, which moves a scrolling layout on the way back. A panel that only shows readings must not provoke that.
    fn keyboard(self) -> KeyboardMode {
        match self {
            Centred::Float => KeyboardMode::None,
            Centred::Modal => KeyboardMode::OnDemand,
        }
    }
}

/// The screen-shape a surface takes, and everything the compositor needs to place it.
#[derive(Clone, Debug)]
pub struct Placement {
    namespace: &'static str,
    anchor: Anchor,
    layer: Layer,
    size: (u32, u32),
    margin: (i32, i32, i32, i32),
    exclusive_zone: i32,
    keyboard: KeyboardMode,
    input: Input,
    output: Option<String>,
    /// The edge this surface hangs off, when it hangs off one.
    ///
    /// Kept rather than re-derived from `anchor`, and that is the whole reason it exists: a card beside a chip on a *left* bar is anchored `LEFT | TOP` — the edge it hangs off and the axis it lines up along — and reading the flags back cannot tell which is which. The scaffold needs the edge, so the edge is remembered.
    edge: Option<Edge>,
    /// Set by the primitives that are hosted rather than self-rendered; read only when lowering to a [`SurfacePlacement`], where it decides the scaffold and the entrance.
    role: SurfaceRole,
    align: SurfaceAlign,
    scrim: bool,
    dismiss_on_outside: bool,
    timeout: Option<std::time::Duration>,
    /// A reservation strip is a surface with no content at all — an exclusive zone and a transparent buffer.
    reserve_only: bool,
    /// Whether this shape is realized by the surface host's scaffold or renders itself. Kept rather than inferred from `role`, which every placement has a value for whether or not it is hosted.
    hosted: bool,
}

impl Placement {
    pub fn namespace(&self) -> &'static str {
        self.namespace
    }

    fn new(namespace: &'static str, anchor: Anchor, layer: Layer) -> Self {
        Self {
            namespace,
            anchor,
            layer,
            size: (0, 0),
            margin: (0, 0, 0, 0),
            exclusive_zone: 0,
            keyboard: KeyboardMode::None,
            input: Input::Solid,
            output: None,
            edge: None,
            role: SurfaceRole::Popup,
            align: SurfaceAlign::Center,
            scrim: false,
            dismiss_on_outside: false,
            timeout: None,
            reserve_only: false,
            hosted: false,
        }
    }

    /// A bar: spans its edge, and reserves nothing *itself* — `-1` opts out of every other surface's zone so its position does not depend on which surface was created first. Its strip is a separate surface.
    pub fn bar(edge: Edge, thickness: u32) -> Self {
        let mut placement = Self::new(bar_namespace(edge), spanning(edge), Layer::Top).zone(-1);
        placement.edge = Some(edge);
        placement.size = across(edge, thickness);
        placement
    }

    /// The invisible strip that carves an edge's space out of every window's idea of the screen.
    pub fn reservation(edge: Edge, thickness: u32) -> Self {
        let mut placement = Self::new(reserve_namespace(edge), spanning(edge), Layer::Bottom)
            .zone(thickness as i32)
            .input(Input::Transparent);
        placement.size = across(edge, thickness);
        placement.reserve_only = true;
        placement
    }

    /// The whole screen and click-through: something painted across the desktop rather than placed on it. `-1` so a bar's reserved strip does not shrink it.
    ///
    /// The background layer is only the default — the wallpaper's. The frame ring takes the same shape up on the bars' layer ([`layer`](Self::layer)), because it draws the strip a framed bar leaves empty and on the background that strip showed the window through.
    pub fn backdrop(namespace: &'static str) -> Self {
        Self::new(namespace, FULLSCREEN, Layer::Background)
            .zone(-1)
            .input(Input::Transparent)
    }

    /// The part of the screen the bars left free, click-through, under every window: where a desktop widget goes.
    ///
    /// A [`backdrop`](Self::backdrop) but for the zone, and that one number is the whole difference. `-1` opts out of every exclusive zone and takes the screen; `0` respects them, so the compositor sizes this to exactly what the bars did not take. A widget centred in it is centred where the applications are, which is where a user looking at their desktop expects the middle to be — and it costs no arithmetic here, because the compositor already did it for the windows.
    pub fn desktop(namespace: &'static str) -> Self {
        Self::new(namespace, FULLSCREEN, Layer::Background).input(Input::Transparent)
    }

    /// A panel that spans an edge and sits over the windows. Zero zone, not `-1`: the compositor has already cleared the bars, and a dock adds only the shared panel margin beyond them.
    pub fn dock(namespace: &'static str, edge: Edge, thickness: u32) -> Self {
        let mut placement = Self::new(namespace, spanning(edge), Layer::Overlay);
        placement.edge = Some(edge);
        placement.size = across(edge, thickness);
        placement
    }

    /// A run of cards pinned to one spot along an edge. Its input region is carved from the cards, so the gaps between them belong to whatever the user is working in.
    ///
    /// Lay the cards out with [`column`](Self::column) — a stack's surface is sized for a full run, so where a short run sits inside it is the difference between hugging the edge and floating in mid-screen.
    pub fn stack(namespace: &'static str, edge: Edge, align: Align) -> Self {
        let mut placement =
            Self::new(namespace, cornered(edge, align), Layer::Overlay).input(Input::FromContent);
        placement.edge = Some(edge);
        placement.align = surface_align(align);
        placement
    }

    /// The column a [`stack`](Self::stack)'s cards lay out in: the full width of the surface, packed against the same end of it the surface itself is pinned to.
    ///
    /// A stack asks the compositor for room for a *full* run of cards, because a layer surface names its size before it knows what it will hold. So a run of one is a card in a box several times its height, and where it sits in that box is entirely up to this: packed the wrong way, a single toast on a bottom-anchored stack renders a full stack's height above the bar, which reads as floating in the middle of the screen.
    ///
    /// Derived from the placement rather than passed alongside it so the two cannot disagree.
    pub fn column(&self, gap: f32) -> LayoutStyle {
        LayoutStyle::new()
            .flex_column()
            .gap(gap)
            .width(SizeDimension::Percent(1.0))
            .height(SizeDimension::Percent(1.0))
            .align_items(AlignItems::STRETCH)
            .justify_content(self.packing())
    }

    /// Which end of its surface a stack's cards pack against: the edge it hangs off when that edge is the one the cards run along, and otherwise the alignment that pins it along a vertical edge.
    fn packing(&self) -> JustifyContent {
        match self.edge {
            Some(Edge::Top) => JustifyContent::START,
            Some(Edge::Bottom) => JustifyContent::END,
            _ => match self.align {
                SurfaceAlign::Start => JustifyContent::START,
                SurfaceAlign::Center => JustifyContent::CENTER,
                SurfaceAlign::End => JustifyContent::END,
            },
        }
    }

    /// A surface hanging off the chip that opened it: the hover popout, a module's drawer, the tray's context menu. `chip` is that chip's laid-out rect, or `None` for a panel reached with no chip in hand — IPC, a keybind — which has nothing to line up with and takes [`align`](Self::align) instead.
    ///
    /// **One shape, because the position is one piece of arithmetic.** The chip's rect decides where the surface sits along the bar and the bar's edge decides which side it hangs off, for all three ([`chip_margin`](crate::anchor::chip_margin)) — so what a hover opens and what a click opens land in the same place. Only [`OffChip`] differs, and it differs in what the surface *is* rather than in where it goes.
    ///
    /// The alignment is `Start` whenever a chip decided the margin, and that is not a taste: the host lays a hosted surface out inside a full-screen scaffold where the margin is padding, so the distance that lines the panel up with its chip is measured from whichever end the panel packs against. Centre it and the same number pushes it half a screen the other way.
    pub fn off_chip(
        kind: OffChip,
        env: &SurfaceEnv,
        chip: Option<Rect>,
        span: Option<f32>,
    ) -> Self {
        let mut placement = match kind {
            OffChip::Card => Self::new(kind.namespace(), beside_a_chip(env.edge), Layer::Overlay)
                .input(Input::FromContent),
            OffChip::Panel => {
                let mut panel =
                    Self::hosted(SurfaceRole::Drawer, kind.namespace(), KeyboardMode::None);
                panel.anchor = edge_anchor(env.edge);
                panel.dismiss_on_outside = true;
                panel
            }
        };
        placement.edge = Some(env.edge);
        placement.output = env.output.clone();
        placement.margin = match chip {
            Some(chip) => {
                placement.align = SurfaceAlign::Start;
                crate::anchor::chip_margin(env, chip, env.config.panel_gap(env.edge) as f32, span)
            }
            None => env.config.panel_margin(env.edge),
        };
        placement
    }

    /// A window in the middle of the screen, sized by what is in it rather than by an edge of the screen: a module's float, the launcher. Which of the two is [`Centred`]'s to say — they are one shape, and differ only in how they go away.
    ///
    /// A float names its size ([`size`](Self::size)); a modal does not, because `dismiss_on_outside` is not only its way out — it is also what makes its *surface* full-screen with the window positioned inside, since the host scaffolds anything that has to catch a press beyond its content. Without it the surface would be centred, unanchored and unsized, which layer-shell rejects outright (a surface not anchored to both edges of an axis has to name a size on it).
    pub fn centred(kind: Centred) -> Self {
        let mut placement = Self::hosted(kind.role(), kind.namespace(), kind.keyboard());
        placement.dismiss_on_outside = kind == Centred::Modal;
        placement
    }

    /// The whole screen, over everything, holding the keyboard: the region picker. Over a fullscreen window on purpose — the user asked to select a region of what they can see.
    pub fn screen(namespace: &'static str) -> Self {
        Self::new(namespace, FULLSCREEN, Layer::Overlay)
            .zone(-1)
            .keyboard(KeyboardMode::Exclusive)
    }

    fn hosted(role: SurfaceRole, namespace: &'static str, keyboard: KeyboardMode) -> Self {
        let mut placement = Self::new(namespace, Anchor::empty(), Layer::Overlay);
        placement.role = role;
        placement.keyboard = keyboard;
        placement.hosted = true;
        placement
    }

    /// Whether the surface host realizes this shape — a scaffold, a scrim, an entrance — or the surface renders itself. The one thing a caller must not decide for itself: lowering a hosted shape to a [`LayerConfig`] yields a surface anchored to nothing, which the compositor rejects outright.
    pub fn is_hosted(&self) -> bool {
        self.hosted
    }

    /// The edge this surface hangs off, when it hangs off one. What a panel's environment reports, so its content resolves the same per-edge settings a bar module does.
    pub fn hangs_off(&self) -> Option<Edge> {
        self.edge
    }

    /// The monitor this surface opens on; `None` is the compositor's choice.
    pub fn monitor(&self) -> Option<&str> {
        self.output.as_deref()
    }

    pub fn size(mut self, width: u32, height: u32) -> Self {
        self.size = (width, height);
        self
    }

    pub fn margin(mut self, margin: (i32, i32, i32, i32)) -> Self {
        self.margin = margin;
        self
    }

    /// The same gap off every edge — what a surface that floats free of the screen's corners wants.
    pub fn inset(self, px: i32) -> Self {
        self.margin((px, px, px, px))
    }

    fn zone(mut self, zone: i32) -> Self {
        self.exclusive_zone = zone;
        self
    }

    pub fn layer(mut self, layer: Layer) -> Self {
        self.layer = layer;
        self
    }

    pub fn keyboard(mut self, keyboard: KeyboardMode) -> Self {
        self.keyboard = keyboard;
        self
    }

    fn input(mut self, input: Input) -> Self {
        self.input = input;
        self
    }

    pub fn align(mut self, align: Align) -> Self {
        self.align = surface_align(align);
        self
    }

    pub fn output(mut self, output: Option<String>) -> Self {
        self.output = output;
        self
    }

    /// The compositor's view of this surface: what a self-rendering surface is opened with.
    pub fn layer_config(&self) -> LayerConfig {
        LayerConfig {
            output: self.output.clone(),
            layer: self.layer,
            anchor: self.anchor,
            exclusive_zone: self.exclusive_zone,
            size: self.size,
            margin: self.margin,
            keyboard_interactivity: match self.keyboard {
                KeyboardMode::None => KeyboardInteractivity::None,
                KeyboardMode::OnDemand => KeyboardInteractivity::OnDemand,
                KeyboardMode::Exclusive => KeyboardInteractivity::Exclusive,
            },
            namespace: self.namespace.to_string(),
            reserve_only: self.reserve_only,
            input_transparent: self.input == Input::Transparent,
            interactive_input_region: self.input == Input::FromContent,
        }
    }

    /// The surface host's view: what a surface that wants the scaffold — a scrim, a dismiss-on-outside, an entrance — is opened with. The host derives its own layer config from this, which is why the namespaces above have to agree with the ones it uses.
    pub fn hosted_placement(&self) -> SurfacePlacement {
        let mut placement = SurfacePlacement::new(self.role, host_anchor(self.edge))
            .align(self.align)
            .keyboard_mode(self.keyboard)
            .output(self.output.clone())
            .margin(self.margin)
            .input_transparent(self.input == Input::Transparent);
        if self.size != (0, 0) {
            placement = placement.size(SurfaceSize::Fixed(self.size.0, self.size.1));
        }
        if self.scrim {
            placement = placement.scrim(true);
        }
        if self.dismiss_on_outside {
            placement = placement.dismiss_on_outside(true);
        }
        if let Some(after) = self.timeout {
            placement = placement.timeout(after);
        }
        placement
    }
}

const FULLSCREEN: Anchor = Anchor::TOP
    .union(Anchor::BOTTOM)
    .union(Anchor::LEFT)
    .union(Anchor::RIGHT);

/// One bar per edge, so the edge is what names it — the string a `layer_rule` in the user's compositor config matches, which is why it is spelled out rather than derived from a debug format.
fn bar_namespace(edge: Edge) -> &'static str {
    match edge {
        Edge::Top => "hogar-shell-top",
        Edge::Bottom => "hogar-shell-bottom",
        Edge::Left => "hogar-shell-left",
        Edge::Right => "hogar-shell-right",
    }
}

fn reserve_namespace(edge: Edge) -> &'static str {
    match edge {
        Edge::Top => "hogar-shell-reserve-top",
        Edge::Bottom => "hogar-shell-reserve-bottom",
        Edge::Left => "hogar-shell-reserve-left",
        Edge::Right => "hogar-shell-reserve-right",
    }
}

/// Anchored to `edge` and stretched along it — a bar, a strip, a dock.
fn spanning(edge: Edge) -> Anchor {
    match edge {
        Edge::Top => Anchor::TOP.union(Anchor::LEFT).union(Anchor::RIGHT),
        Edge::Bottom => Anchor::BOTTOM.union(Anchor::LEFT).union(Anchor::RIGHT),
        Edge::Left => Anchor::LEFT.union(Anchor::TOP).union(Anchor::BOTTOM),
        Edge::Right => Anchor::RIGHT.union(Anchor::TOP).union(Anchor::BOTTOM),
    }
}

/// Pinned to one spot along `edge`: the edge itself, plus the end `align` names. Centre pins nothing more, so the compositor centres it along that edge.
fn cornered(edge: Edge, align: Align) -> Anchor {
    let mut anchor = edge_anchor(edge);
    let (start, end) = if edge.is_horizontal() {
        (Anchor::LEFT, Anchor::RIGHT)
    } else {
        (Anchor::TOP, Anchor::BOTTOM)
    };
    match align {
        Align::Start => anchor |= start,
        Align::End => anchor |= end,
        Align::Center => {}
    }
    anchor
}

/// The two edges a chip-anchored card pins itself to: the bar's own, so it hangs off it, and the one it runs along, so the margin that lines it up with the chip means something.
fn beside_a_chip(edge: Edge) -> Anchor {
    match edge {
        Edge::Top => Anchor::TOP.union(Anchor::LEFT),
        Edge::Bottom => Anchor::BOTTOM.union(Anchor::LEFT),
        Edge::Left => Anchor::LEFT.union(Anchor::TOP),
        Edge::Right => Anchor::RIGHT.union(Anchor::TOP),
    }
}

fn edge_anchor(edge: Edge) -> Anchor {
    match edge {
        Edge::Top => Anchor::TOP,
        Edge::Bottom => Anchor::BOTTOM,
        Edge::Left => Anchor::LEFT,
        Edge::Right => Anchor::RIGHT,
    }
}

/// The host describes an anchor as the single edge a panel hangs off — it positions the panel inside a full-screen scaffold — so a surface that hangs off none is centred.
fn host_anchor(edge: Option<Edge>) -> SurfaceAnchor {
    match edge {
        Some(Edge::Top) => SurfaceAnchor::Top,
        Some(Edge::Bottom) => SurfaceAnchor::Bottom,
        Some(Edge::Left) => SurfaceAnchor::Left,
        Some(Edge::Right) => SurfaceAnchor::Right,
        None => SurfaceAnchor::Center,
    }
}

fn surface_align(align: Align) -> SurfaceAlign {
    match align {
        Align::Start => SurfaceAlign::Start,
        Align::Center => SurfaceAlign::Center,
        Align::End => SurfaceAlign::End,
    }
}

/// A surface's own thickness on `edge`, as a layer-shell size: the axis it spans is handed back to the compositor with a zero.
fn across(edge: Edge, thickness: u32) -> (u32, u32) {
    if edge.is_horizontal() {
        (0, thickness)
    } else {
        (thickness, 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_on(edge: Edge) -> SurfaceEnv {
        SurfaceEnv::for_edge(std::sync::Arc::new(config::Config::starter()), edge, None)
    }

    fn a_chip() -> Rect {
        Rect {
            x: 200.0,
            y: 200.0,
            width: 30.0,
            height: 30.0,
        }
    }

    /// What a press on a chip on `edge` opens — a drawer, or the tray's menu, which are one shape.
    fn chip_panel(edge: Edge) -> Placement {
        Placement::off_chip(OffChip::Panel, &env_on(edge), Some(a_chip()), Some(260.0))
    }

    /// A stack's cards pack against the very edge its surface is pinned to.
    ///
    /// The surface is sized for a *full* run of cards, because a layer surface names its size before it knows what it will hold. Pack the column the wrong way and a run of one sits a full stack's height away from the edge it belongs to — a single toast on a bottom-anchored stack rendering mid-screen, which is what this looked like before the column came from the placement instead of from a default.
    #[test]
    fn a_stack_packs_its_cards_against_the_edge_it_hangs_off() {
        let stack = |edge, align| Placement::stack("hogar-shell-toasts", edge, align).packing();
        assert_eq!(stack(Edge::Bottom, Align::Center), JustifyContent::END);
        assert_eq!(stack(Edge::Top, Align::Center), JustifyContent::START);
        // The alignment decides only where the surface sits *along* a horizontal edge, never which way its cards pile up from it.
        assert_eq!(stack(Edge::Bottom, Align::Start), JustifyContent::END);

        // On a vertical edge the cards run along the edge itself, so the alignment is the only thing that says which end of it they start from.
        assert_eq!(stack(Edge::Left, Align::Start), JustifyContent::START);
        assert_eq!(stack(Edge::Left, Align::End), JustifyContent::END);
        assert_eq!(stack(Edge::Right, Align::Center), JustifyContent::CENTER);
    }

    /// Every primitive must produce a surface the compositor will accept.
    ///
    /// Layer-shell rejects a surface that names no size on an axis it is not anchored to *both* edges of — and rejects it by killing the surface, which reaches the user as a flood of `Protocol error` and a window that never appears. A scaffolded placement is exempt because the host makes it full-screen and positions the panel inside it, so this is the same question the host asks, asked here over the whole taxonomy.
    ///
    /// It is not hypothetical: `modal` shipped without the flag that scaffolds it for exactly one build, which took it out of the scaffolded branch and left the launcher unanchored and unsized. Pressing the search chip killed it.
    #[test]
    fn every_primitive_is_a_surface_the_compositor_will_accept() {
        let chip = Rect {
            x: 200.0,
            y: 0.0,
            width: 30.0,
            height: 30.0,
        };
        let env = SurfaceEnv::for_edge(
            std::sync::Arc::new(config::Config::starter()),
            Edge::Top,
            None,
        );
        let every = [
            ("bar", Placement::bar(Edge::Top, 34)),
            ("reservation", Placement::reservation(Edge::Top, 34)),
            ("backdrop", Placement::backdrop("hogar-shell-wallpaper")),
            (
                "dock",
                Placement::dock("hogar-shell-sidebar", Edge::Right, 380),
            ),
            (
                "stack",
                Placement::stack("hogar-shell-toasts", Edge::Top, Align::End).size(320, 200),
            ),
            (
                "off_chip card",
                Placement::off_chip(OffChip::Card, &env, Some(chip), Some(260.0)).size(260, 180),
            ),
            (
                "off_chip panel",
                Placement::off_chip(OffChip::Panel, &env, Some(chip), Some(260.0)),
            ),
            (
                "centred float",
                Placement::centred(Centred::Float).size(920, 680),
            ),
            ("centred modal", Placement::centred(Centred::Modal)),
            ("screen", Placement::screen("hogar-shell-picker")),
        ];

        for (name, placement) in every {
            if placement.is_hosted() {
                continue;
            }
            let layer = placement.layer_config();
            let spans_x =
                layer.anchor.contains(Anchor::LEFT) && layer.anchor.contains(Anchor::RIGHT);
            let spans_y =
                layer.anchor.contains(Anchor::TOP) && layer.anchor.contains(Anchor::BOTTOM);
            assert!(
                spans_x || layer.size.0 > 0,
                "`{name}` names no width and is not anchored to both side edges: the compositor rejects it"
            );
            assert!(
                spans_y || layer.size.1 > 0,
                "`{name}` names no height and is not anchored to top and bottom: the compositor rejects it"
            );
        }
    }

    /// The namespaces are a public interface — a user's `layer_rule` matches on them — so they are asserted, not left to whatever a refactor happens to produce.
    ///
    /// **And a hosted shape is asserted twice**, because it announces its namespace through a second path: the surface host derives it from the [`SurfaceRole`], which never sees the string this carries. The two are kept in step by hand, and while they were not, the tray's menu — a card that had been made dismissable — went out as `hogar-shell-popup`, a name no primitive claims and no `layer_rule` in anyone's config mentions.
    #[test]
    fn every_primitive_announces_the_namespace_it_always_has() {
        let env = SurfaceEnv::for_edge(
            std::sync::Arc::new(config::Config::starter()),
            Edge::Top,
            None,
        );
        let chip = Rect {
            x: 200.0,
            y: 0.0,
            width: 30.0,
            height: 30.0,
        };

        assert_eq!(Placement::bar(Edge::Top, 34).namespace, "hogar-shell-top");
        assert_eq!(
            Placement::reservation(Edge::Left, 40).namespace,
            "hogar-shell-reserve-left"
        );
        assert_eq!(
            Placement::off_chip(OffChip::Card, &env, Some(chip), None).namespace,
            "hogar-shell-popout"
        );
        assert_eq!(
            Placement::off_chip(OffChip::Panel, &env, Some(chip), None).namespace,
            "hogar-shell-drawer"
        );
        assert_eq!(
            Placement::centred(Centred::Float).namespace,
            "hogar-shell-float"
        );
        assert_eq!(
            Placement::centred(Centred::Modal).namespace,
            "hogar-shell-overlay"
        );

        // The role every hosted shape is realized as, which is what the host turns into the same string.
        let role_of = |placement: Placement| placement.hosted_placement().role;
        assert_eq!(
            role_of(Placement::off_chip(OffChip::Panel, &env, Some(chip), None)),
            SurfaceRole::Drawer,
            "the drawer's namespace and the tray menu's now come from one place, so they cannot disagree"
        );
        assert_eq!(
            role_of(Placement::centred(Centred::Float)),
            SurfaceRole::Float
        );
        assert_eq!(
            role_of(Placement::centred(Centred::Modal)),
            SurfaceRole::Overlay
        );
    }

    #[test]
    fn a_bar_spans_its_edge_and_a_stack_pins_to_a_corner() {
        let bar = Placement::bar(Edge::Top, 34).layer_config();
        assert!(bar.anchor.contains(Anchor::LEFT) && bar.anchor.contains(Anchor::RIGHT));
        assert_eq!(bar.exclusive_zone, -1, "a bar reserves through its strip");

        for edge in Edge::ALL {
            assert!(
                Placement::stack("hogar-shell-toasts", edge, Align::Center)
                    .layer_config()
                    .anchor
                    .contains(edge_anchor(edge)),
                "a stack anchors to its own edge, on all four of them"
            );
        }

        let stack = Placement::stack("hogar-shell-toasts", Edge::Top, Align::End).layer_config();
        assert!(
            stack.anchor.contains(Anchor::TOP) && stack.anchor.contains(Anchor::RIGHT),
            "an end-aligned stack on a horizontal edge pins to that edge's right"
        );
        assert!(
            !stack.anchor.contains(Anchor::LEFT),
            "and not to both, or it would stretch instead of pinning"
        );
        // The same word means a different anchor along a vertical edge, which is why this is not one arm.
        let side = Placement::stack("hogar-shell-toasts", Edge::Left, Align::End).layer_config();
        assert!(
            side.anchor.contains(Anchor::BOTTOM) && !side.anchor.contains(Anchor::RIGHT),
            "along a vertical edge, the end is its bottom"
        );
        assert!(
            stack.interactive_input_region,
            "the gaps between cards belong to whatever is underneath"
        );
    }

    /// Every surface that carries no content is click-through, and only those.
    #[test]
    fn what_takes_the_pointer_is_decided_once() {
        assert!(
            Placement::backdrop("hogar-shell-wallpaper")
                .layer_config()
                .input_transparent
        );
        assert!(
            Placement::reservation(Edge::Top, 30)
                .layer_config()
                .input_transparent
        );
        let dock = Placement::dock("hogar-shell-sidebar", Edge::Right, 380).layer_config();
        assert!(!dock.input_transparent && !dock.interactive_input_region);
    }

    /// A panel paints nothing outside itself, and that is a requirement rather than a taste.
    ///
    /// A scaffolded surface is full-screen — that is how a press beside the panel reaches it — so anything it paints out there covers the screen. A scrim is exactly that, and it made a compositor `layer_rule = blur` unusable: blur follows the alpha the surface writes, so a 35%-black wash meant opening a drawer blurred the whole desktop instead of the panel. The surface stays full-screen and stays dismissible; it just leaves the rest of the screen at alpha zero, where `ignorealpha` skips it.
    #[test]
    fn a_scaffolded_panel_leaves_the_rest_of_the_screen_untouched() {
        for hosted in [chip_panel(Edge::Top), Placement::centred(Centred::Modal)] {
            let placement = hosted.hosted_placement();
            assert!(
                !placement.scrim,
                "a panel that washes the screen behind it makes every pixel of it blurrable"
            );
            assert!(
                placement.dismiss_on_outside,
                "and it is this, not the wash, that scaffolds the surface full-screen"
            );
        }
    }

    /// The host positions a panel inside a full-screen scaffold, so it wants the one edge the panel hangs off — a spanning anchor has to collapse to it rather than confusing the scaffold with two.
    #[test]
    fn a_hosted_placement_keeps_the_edge_it_hangs_off() {
        let panel = chip_panel(Edge::Right)
            .keyboard(KeyboardMode::OnDemand)
            .hosted_placement();
        assert_eq!(panel.anchor, SurfaceAnchor::Right);
        assert!(panel.dismiss_on_outside);
        assert_eq!(panel.keyboard, KeyboardMode::OnDemand);

        let modal = Placement::centred(Centred::Modal).hosted_placement();
        assert_eq!(
            modal.anchor,
            SurfaceAnchor::Center,
            "a modal is not anchored to an edge at all"
        );
        assert_eq!(modal.keyboard, KeyboardMode::OnDemand);
    }

    /// **Only the surface that is selecting part of the screen may grab the pointer.**
    ///
    /// An exclusive layer surface is an input grab: while one is up the compositor delivers pointer events to nothing else, so the bar stops highlighting chips, stops opening popouts, and answers a press by handing it to the grabbing surface. The launcher held that grab, which is why opening it made the bar dead and a press on a chip dismissed the launcher instead of opening that chip's panel.
    ///
    /// The region picker keeps it, and is the one shape that should: it is drawn over the whole screen to select an area *of* it, and a bar answering clicks inside that area would be answering clicks meant for the selection.
    #[test]
    fn nothing_but_the_region_picker_takes_the_pointer_from_the_rest_of_the_shell() {
        let grabs = |placement: &Placement| placement.keyboard == KeyboardMode::Exclusive;
        assert!(grabs(&Placement::screen("hogar-shell-picker")));
        for shared in [
            Placement::centred(Centred::Modal),
            Placement::centred(Centred::Float).keyboard(KeyboardMode::OnDemand),
            chip_panel(Edge::Top).keyboard(KeyboardMode::OnDemand),
            Placement::dock("hogar-shell-sidebar", Edge::Right, 380),
        ] {
            assert!(
                !grabs(&shared),
                "this shape shares the screen with the bar, so it must not hold the pointer away from it"
            );
        }
    }
}
