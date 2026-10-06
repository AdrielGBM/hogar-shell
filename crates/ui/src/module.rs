use std::any::Any;
use std::cell::RefCell;
use std::fmt;
use std::rc::Rc;

use telar::{Color, Rect};

use config::theme::NordTheme;
use config::{Edge, Variant};

thread_local! {
    static PRESSED_CHIP: RefCell<Option<Pressed>> = const { RefCell::new(None) };
}

/// Runs `act` — a chip's press or drag-open handler — with the chip's own laid-out rect in scope.
///
/// The rect is what a drawer hangs off, on the same terms as the card a hover opens over the same chip, and only the chip knows it. What travelled here before was the *zone* the chip sat in, which could say no more than which end of the bar to align to — and could not always say that: an id placed in more than one zone resolves to whichever the config search reaches first, and a module the layout places on the desktop grid sits in no bar zone at all despite being laid out at a very definite place on screen. A rect answers both without asking the config anything.
///
/// Ambient rather than a parameter because the handler that reads it may be a `ModuleClick::Action` — a bare `fn()` that opens someone else's panel — which no signature change reaches. Scoped strictly to the synchronous dispatch, so nothing can read a stale rect afterwards.
/// The chip a press or a drag came from: its rect, the output it is on, which two identical bars on two identical screens need to tell apart, and where the surface that built it placed it.
#[derive(Clone, Debug)]
pub struct Pressed {
    pub rect: Rect,
    pub output: Option<String>,
    pub placement: Option<Placement>,
}

/// Where a surface placed the chip, as that surface names it, handed back with a press of it so the surface need not find the chip again by where it is drawn.
#[derive(Clone)]
pub struct Placement(Rc<dyn Any>);

impl Placement {
    pub fn new(at: impl Any) -> Self {
        Self(Rc::new(at))
    }

    pub fn get<T: Any>(&self) -> Option<&T> {
        self.0.downcast_ref()
    }
}

impl fmt::Debug for Placement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Placement")
    }
}

pub fn from_chip<R>(chip: Pressed, act: impl FnOnce() -> R) -> R {
    let previous = PRESSED_CHIP.with(|pressed| pressed.replace(Some(chip)));
    let done = act();
    PRESSED_CHIP.with(|pressed| *pressed.borrow_mut() = previous);
    done
}

/// The rect of the chip whose press is being dispatched, if a press is what is running. `None` for a panel reached from anywhere else — IPC, a keybind — where there is no chip to hang off.
pub fn pressed_chip() -> Option<Pressed> {
    PRESSED_CHIP.with(|pressed| pressed.borrow().clone())
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

/// A chip's drag-to-open gesture: pulling it away from the bar runs `open`, which opens what its press would otherwise toggle.
#[derive(Clone)]
pub struct DragOpen {
    pub open: Rc<dyn Fn()>,
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

#[cfg(test)]
mod tests {
    use super::*;
    use telar::LayoutItem;
    #[test]
    fn a_drag_opens_a_panel_only_when_it_pulls_away_from_the_bar() {
        let gesture = |edge| DragOpen {
            open: Rc::new(|| ()),
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
            AvailableSpace, Container, LayoutStyle, compute_layout, reset_layout_runtime,
            set_theme, track_layout,
        };

        const ROOM: f32 = 120.0;
        const LABEL: f32 = 300.0;

        let measure = |elastic: bool| {
            reset_layout_runtime();
            set_theme(NordTheme::new());
            let label = Container::new(LayoutStyle::new().width(LABEL).height(20.0), vec![])
                .expect("the label builds");
            let chip = module_shell(
                ModuleShellProps::props().elastic(elastic).build(),
                crate::layout::built_once(Box::new(label)),
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
            AvailableSpace, ComponentList, Container, DrawCommand, LayoutStyle, Paint,
            compute_layout, reset_layout_runtime, set_theme,
        };

        let fills = |variant: Variant, rest: Color, accent: Color| {
            reset_layout_runtime();
            set_theme(NordTheme::new());
            let label = Container::new(LayoutStyle::new().width(20.0).height(20.0), vec![])
                .expect("the label builds");
            let chip = module_shell(
                ModuleShellProps::props()
                    .variant(variant)
                    .rest(rest)
                    .accent(accent)
                    .build(),
                crate::layout::built_once(Box::new(label)),
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
