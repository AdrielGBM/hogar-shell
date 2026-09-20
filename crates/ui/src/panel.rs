//! Every window the shell opens that is not the wallpaper, the frame or a bar.
//!
//! [`Placement`] settled where a surface *sits*. This settles what a surface *is*: eleven windows that each resolved their own config, set their own theme, declared their own transparency and answered their own `clear_color`, in eleven copies of the same eight lines — and each one free to leave a line out.
//!
//! Eight of them left out the same one. None installed a [`SurfaceEnv`], so their content resolved the *global* config through [`surface_env`](config::surface_env) instead of their own, per-monitor overrides never reached them, and [`panel_fill`] answered a solid colour to panels the user had configured translucent — silently, because falling back is not an error. That was not eleven bugs waiting to happen; it was one bug that had already happened eleven times over and only showed once.
//!
//! So a panel names the four things that actually differ between one window and the next — where it sits ([`Placement`]), which edge its content resolves against, whether it slides in, and what it draws — and this does the rest. The next setting like `[panels] opacity` lands in one place.
//!
//! **What is deliberately not a panel.** The wallpaper is opaque by definition and clears to a colour. The frame paints a ring and has no content to resolve anything for. A bar has zones, a reserved strip, per-edge shape and auto-hide. The lock screen is mounted by the compositor's session rather than opened here, and is the one surface that must never be translucent. Those four earn their own types; nothing else does.

use std::rc::Rc;
use std::sync::Arc;

use telar::motion::Animated;
use telar::{
    App, Color, Component, Container, LayoutError, LayoutItem, LayoutStyle, RectStyle,
    StyledContainer, SurfaceToken, WindowConfig, reset_layout_runtime, set_theme, surface_content,
};

use config::{AnimationConfig, Config, Edge, SurfaceEnv, set_surface_env, surface_env};
use platform_wayland::SurfaceHandle;
use util::state::kept;

use crate::descriptor::Built;
use crate::placement::Placement;
use telar::WindowRoot;

/// What a panel draws, given the environment this build resolved. `Fn` rather than `FnOnce`: a surface outlives the config it opened under, and a rebuild is how it follows an edit.
pub type PanelContent = Rc<dyn Fn(&SurfaceEnv) -> Built>;

/// A window that is not a bar: a drawer, a float, a card, the launcher, an OSD, a toast stack, the region picker. Built from a [`Placement`] and its content, opened with [`open`](Self::open).
pub struct PanelSurface {
    placement: Placement,
    edge: Option<Edge>,
    content: PanelContent,
    transition: bool,
}

impl PanelSurface {
    pub fn new(placement: Placement, content: impl Fn(&SurfaceEnv) -> Built + 'static) -> Self {
        Self {
            placement,
            edge: None,
            content: Rc::new(content),
            transition: false,
        }
    }

    /// The edge this panel's content resolves against, for a placement that hangs off none — a float, the launcher, the tray menu. It is the bar the panel came from, so a chip on the left bar opens a window that reads the left bar's settings. Without one, the first edge the config draws a bar on.
    pub fn edge(mut self, edge: Edge) -> Self {
        self.edge = Some(edge);
        self
    }

    /// Slides and fades the panel in from the edge it hangs off, and back out to it on close.
    pub fn animated(mut self) -> Self {
        self.transition = true;
        self
    }

    /// Puts the panel on screen. Which of the two ways that happens — the surface host's scaffold, or a surface that renders itself — is the placement's to answer, not the caller's.
    pub fn open(self) -> SurfaceToken {
        if self.placement.is_hosted() {
            let placement = self.placement.hosted_placement();
            let panel = Rc::new(self);
            return telar::open_surface(placement, surface_content(move || panel.build()));
        }
        SurfaceToken::new(Box::new(self.open_handle()))
    }

    /// The same, handing back the compositor's own handle — for a panel the shell renegotiates in place rather than reopening (the notification popup follows a config edit the way a bar does). Self-rendered placements only: a hosted shape lowered to a layer config is anchored to nothing, and the compositor kills it.
    pub fn open_handle(self) -> SurfaceHandle {
        debug_assert!(
            !self.placement.is_hosted(),
            "a hosted placement has no layer config of its own; open it with `open`"
        );
        let layer = self.placement.layer_config();
        platform_wayland::open_surface(layer, PanelApp { panel: self })
    }

    /// One build of the panel: resolve this screen's config, put it in scope, and draw.
    ///
    /// The environment is *installed*, not merely read, because that is the whole contract — every module, icon lookup and `panel_fill` inside the tree reads it back through [`surface_env`], the way a chip does inside its bar.
    fn build(&self) -> Box<dyn LayoutItem> {
        let output = self.placement.monitor().map(str::to_string);
        let config = config::config_for(output.as_deref());
        let edge = self
            .edge
            .or_else(|| self.placement.hangs_off())
            .unwrap_or_else(|| drawn_edge(&config));
        set_theme(config.resolve_theme());
        services::locale::attach(config.language());
        let env = SurfaceEnv::for_edge(Arc::clone(&config), edge, output);
        set_surface_env(env.clone());
        let content = (self.content)(&env).and_then(|content| match self.transition {
            true => panel_transition(content, edge, &config.animation),
            false => Ok(content),
        });
        or_empty(self.placement.namespace(), content)
    }
}

/// A self-rendered panel. Transparent and clearing to nothing, both of which every panel wants: what is behind a translucent surface is the desktop, and a colour cleared under it is what would hide it.
struct PanelApp {
    panel: PanelSurface,
}

impl App for PanelApp {
    fn root(&self) -> Box<dyn Component> {
        reset_layout_runtime();
        Box::new(WindowRoot::wrapping(self.panel.build()).expect("panel surface root"))
    }

    fn clear_color(&self) -> Option<Color> {
        None
    }

    fn window_config(&self) -> Option<WindowConfig> {
        Some(WindowConfig {
            is_transparent: true,
            ..WindowConfig::default()
        })
    }
}

/// `built`, or an empty tree and the error in the log when a surface's own chrome failed to build. Module builds inside it are already guarded ([`crate::descriptor::guard`]); what reaches here is the shell's own layout failing, which costs that surface rather than the shell.
pub fn or_empty(surface: &str, built: Built) -> Box<dyn LayoutItem> {
    built.unwrap_or_else(|e| {
        tracing::error!("the {surface} surface failed to build: {e}");
        Box::new(Container::new(LayoutStyle::new(), Vec::new()).expect("an empty container"))
    })
}

/// The background a panel paints, at `[panels] opacity` — or `[theme] opacity` where the panel names none.
///
/// The surface's own config first, so a per-monitor override reaches it; the global config next, so a caller outside a surface still gets the *configured* opacity. Falling straight back to the theme token is what this used to do, and it answered a solid colour to every panel whose surface forgot to install its env — a translucent shell that silently was not one, with no error anywhere.
pub fn panel_fill() -> Color {
    if let Some(env) = surface_env() {
        return env.config.panel_fill();
    }
    match config::config() {
        Some(config) => config.panel_fill(),
        None => telar::use_theme::<config::theme::NordTheme>().surface,
    }
}

/// The corner radius content inside a panel rounds to — the bar's, so a notification card in a drawer matches the bar the drawer hangs off.
///
/// Derived from the panel's own environment rather than carried beside it. Two copies of one number is how a float ends up rounding its cards to a radius the drawer showing the same panel does not.
pub fn content_radius() -> f32 {
    surface_env()
        .map(|env| env.config.panel_radius(env.edge))
        .unwrap_or(0.0)
}

/// The base distance content inside a panel is spaced by — the bar's `spacing`, so a card in a drawer breathes like the bar the drawer hangs off. The base of the [`crate::scale::space`] scale, exactly as [`content_radius`] is the base of [`crate::scale::corner`].
///
/// It falls back to the theme's own `spacing` rather than to zero, unlike the radius: a shell with no radius is square, and a shell with no spacing is illegible. Same three steps as [`card_gap`] — the surface's env, then the global config, then the token's default, which is what a preview and a headless test get.
pub fn content_spacing() -> f32 {
    if let Some(env) = surface_env() {
        return env.config.resolved_spacing(env.edge);
    }
    config::config().map_or_else(
        || config::theme::NordTheme::new().spacing,
        |config| {
            let edge = drawn_edge(&config);
            config.resolved_spacing(edge)
        },
    )
}

/// The space between two stacked cards, from the panel's own environment — [`Config::card_gap`], the shell's `spacing` token. A toast stack and a notification stack are the same shape of thing and keep the same rhythm; each carrying its own key is how they stopped. The last resort is the token's own default rather than [`telar::use_theme`], unlike [`panel_fill`]: a stack asks for this while deciding how tall a surface to open, which is before any tree — and therefore any theme — exists.
pub fn card_gap() -> f32 {
    if let Some(env) = surface_env() {
        return env.config.card_gap();
    }
    config::config().map_or_else(
        || config::theme::NordTheme::new().spacing,
        |config| config.card_gap(),
    )
}

/// The first edge the config puts modules on — the bar the user looks at — falling back to the top for a config that has no bars at all. What a panel hanging off no edge in particular resolves against.
pub fn drawn_edge(config: &Config) -> Edge {
    Edge::ALL
        .into_iter()
        .find(|edge| !config.bars.get(*edge).is_empty())
        .unwrap_or(Edge::Top)
}

/// Slides and fades `content` in from the bar edge it hangs off, and back out to it when the surface is asked to close, over `[animation] panel_duration_ms` and the configured easing.
///
/// One progress carries both halves — 1 is off the bar edge and transparent, 0 is settled — so the exit is the entrance reversed rather than a second animation that has to be kept in step with the first.
///
/// The exit only reaches the screen because the driver holds a closing surface mapped for as long as [`on_close`](platform_wayland::on_close) says to. Without that it would animate a surface that was torn down on the loop's next turn, which is exactly what this could not do before.
///
/// Constructed away from its goal and retargeted at once, never at the goal: an `Animated` born settled never registers with the ticker, so nothing would schedule the frames that carry it in — the same trap the workspace indicator hit.
///
/// Kept across rebuilds ([`kept`]), because arriving is something the panel did once: a fresh `Animated` would start at 1 again and slide the panel back in, so every config edit would look like the drawer reopening. The one this finds on a rebuild has already settled at 0, which is exactly where the panel is.
///
/// The box claims nothing for the input region, and must not: it paints nothing, and the panel inside it is what carries the claim. A wrapper that claimed its own rect would go on catching presses through the whole exit, which is the "faded layer still takes the touch" bug — and it would claim the travel as well as the panel, since the box is where the panel *would* sit rather than where it has slid to.
pub fn panel_transition(
    content: Box<dyn LayoutItem>,
    edge: Edge,
    animation: &AnimationConfig,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let tween = animation.panel_tween();
    if tween.duration.is_zero() {
        return Ok(content);
    }
    // The distance is in the panel's own travel, not the screen's: a drawer arrives from the edge it hangs off.
    let travel = 24.0;
    let progress = kept("drawer.transition", || {
        let progress = Animated::new(1.0f32, tween);
        progress.retarget(0.0);
        progress
    });
    platform_wayland::on_close(tween.duration, move || progress.retarget(1.0));
    let slide = progress;
    let fade = progress;
    let (dx, dy) = match edge {
        Edge::Top => (0.0, -travel),
        Edge::Bottom => (0.0, travel),
        Edge::Left => (-travel, 0.0),
        Edge::Right => (travel, 0.0),
    };
    // Shrink-wrapped, and that is load-bearing rather than tidy: this box is the node the scaffold measures to decide what counts as "outside the panel", and it is the child the scaffold's `align_items` positions. A `width: 100%` here made both wrong at once — every press in the panel's whole horizontal band read as a press *on* it, and the panel sat at the start of a full-width box instead of at the end of the bar its module lives on.
    Ok(Box::new(
        StyledContainer::new(LayoutStyle::new(), |_| RectStyle::default(), vec![content])?
            .with_transform(move |_| {
                let at = slide.get();
                (at != 0.0).then_some([1.0, 0.0, 0.0, 1.0, dx * at, dy * at])
            })
            .with_opacity(move || 1.0 - fade.get()),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use config::theme::NordTheme;

    fn content() -> Box<dyn LayoutItem> {
        telar::box_item(telar::Container::new(LayoutStyle::new(), vec![]).unwrap())
    }

    #[test]
    fn a_panel_enters_on_every_edge_and_skips_the_wrapper_when_animation_is_off() {
        for edge in Edge::ALL {
            telar::reset_layout_runtime();
            telar::set_theme(NordTheme::new());
            assert!(
                panel_transition(content(), edge, &AnimationConfig::default()).is_ok(),
                "the panel transition builds on {edge:?}"
            );
        }

        // Switched off, the panel is handed back untouched rather than wrapped in a box that animates nothing — an extra container around every panel is a layout change nobody asked for.
        telar::reset_layout_runtime();
        telar::set_theme(NordTheme::new());
        let off = AnimationConfig {
            enabled: false,
            ..AnimationConfig::default()
        };
        assert!(off.panel_tween().duration.is_zero());
        assert!(panel_transition(content(), Edge::Top, &off).is_ok());
    }

    /// The transition box is the node the scaffold measures, so its width is the drawer's dismiss area.
    ///
    /// It was `width: 100%`, which made two things wrong at once and neither of them visible: a press anywhere in the panel's horizontal band read as a press *on* the panel, so the only way to dismiss a drawer was to click above or below it — and the panel was positioned at the start of a full-width box rather than by the scaffold's own alignment, which is what puts it at the end of the bar its module sits on.
    ///
    /// Building proves none of that; the wrapper builds happily either way. This lays out the real tree the surface host mounts and presses next to the panel.
    #[test]
    fn a_press_beside_the_panel_dismisses_the_drawer() {
        use crate::placement::{OffChip, Placement};
        use std::cell::Cell;
        use telar::{
            AvailableSpace, Component, Event, PointerButton, PointerSource, SurfaceScaffold,
            compute_layout,
        };

        const PANEL_WIDTH: f32 = 320.0;
        const SURFACE: f32 = 1280.0;

        let env = SurfaceEnv::for_edge(Arc::new(Config::starter()), Edge::Top, None);

        for align in [
            platform_wayland::SurfaceAlign::Start,
            platform_wayland::SurfaceAlign::Center,
            platform_wayland::SurfaceAlign::End,
        ] {
            telar::reset_layout_runtime();
            telar::set_theme(NordTheme::new());

            let panel = telar::box_item(
                telar::Container::new(LayoutStyle::new().width(PANEL_WIDTH).height(200.0), vec![])
                    .unwrap(),
            );
            let wrapped = panel_transition(panel, Edge::Top, &AnimationConfig::default()).unwrap();

            let dismissed = Rc::new(Cell::new(0u32));
            let sink = Rc::clone(&dismissed);
            let placement = Placement::off_chip(OffChip::Panel, &env, None, None)
                .margin((8, 8, 8, 8))
                .hosted_placement()
                .align(align);
            let mut scaffold = SurfaceScaffold::new(
                telar::Edge::Top,
                match align {
                    platform_wayland::SurfaceAlign::Start => telar::AlignItems::START,
                    platform_wayland::SurfaceAlign::Center => telar::AlignItems::CENTER,
                    platform_wayland::SurfaceAlign::End => telar::AlignItems::END,
                },
                placement.margin,
                Some(telar::DEFAULT_SCRIM),
                Some(Rc::new(move || sink.set(sink.get() + 1))),
                wrapped,
            )
            .unwrap();
            compute_layout(
                scaffold.layout_node(),
                AvailableSpace::Definite(SURFACE),
                AvailableSpace::Definite(720.0),
            )
            .unwrap();
            scaffold.on_event(&Event::WindowResized {
                width: SURFACE as u32,
                height: 720,
            });

            let press = |x: f64| Event::PointerPressed {
                x,
                y: 100.0,
                button: PointerButton::Primary,
                source: PointerSource::Mouse,
            };
            // One of the two far edges is always scrim whichever end the panel is aligned to, so pressing both and requiring one to dismiss holds for every alignment without restating the layout.
            let before = dismissed.get();
            scaffold.on_event(&press(16.0));
            scaffold.on_event(&press(SURFACE as f64 - 16.0));
            assert!(
                dismissed.get() > before,
                "{align:?}: a press beside the panel must dismiss the drawer — a full-width wrapper makes the \
                 whole row read as the panel, leaving no way out but clicking past its top or bottom edge"
            );
        }
    }

    /// **The test this module exists for.** Every panel-shaped window, built, and asked afterwards what it could see: its own screen's config, on the edge it belongs to.
    ///
    /// Eight of the eleven surfaces answered `None` here for as long as they had existed. Nothing failed — every reader of [`surface_env`] falls back, which is exactly why it went unnoticed until `[panels] opacity` was configured and did nothing. A fallback is not an error, so only a test can say the environment is *there*.
    #[test]
    fn every_shape_a_panel_takes_can_see_its_own_config() {
        use crate::placement::{Centred, OffChip, Placement};
        use config::Align;
        use std::cell::RefCell;

        let chip = telar::Rect {
            x: 200.0,
            y: 0.0,
            width: 30.0,
            height: 30.0,
        };
        let env = SurfaceEnv::for_edge(Arc::new(Config::starter()), Edge::Top, None);
        // Every primitive a window that is not a bar is built from, and the edge each must report: the one it hangs off, or the one named for a shape that hangs off none.
        let every: Vec<(&str, Placement, Option<Edge>)> = vec![
            (
                "drawer",
                Placement::off_chip(OffChip::Panel, &env, Some(chip), Some(260.0)),
                None,
            ),
            (
                "float",
                Placement::centred(Centred::Float).size(640, 480),
                Some(Edge::Left),
            ),
            ("launcher", Placement::centred(Centred::Modal), None),
            (
                "card column",
                Placement::stack("hogar-shell-stack", Edge::Bottom, Align::End).size(320, 200),
                None,
            ),
            (
                "popout",
                Placement::off_chip(OffChip::Card, &env, Some(chip), Some(260.0)).size(260, 180),
                None,
            ),
            (
                "notification centre",
                Placement::dock("hogar-shell-sidebar", Edge::Right, 380),
                None,
            ),
            (
                "region picker",
                Placement::screen("hogar-shell-picker"),
                None,
            ),
        ];

        for (name, placement, edge) in every {
            telar::reset_layout_runtime();
            telar::set_theme(NordTheme::new());
            let want = edge.or_else(|| placement.hangs_off());
            let seen: Rc<RefCell<Option<SurfaceEnv>>> = Rc::new(RefCell::new(None));
            let sink = Rc::clone(&seen);
            let mut panel = PanelSurface::new(placement, move |_| {
                *sink.borrow_mut() = surface_env();
                Ok(content())
            });
            if let Some(edge) = edge {
                panel = panel.edge(edge);
            }
            // A surface that installs nothing would read back whatever the last one left in scope, so what it must not answer is planted first: only an env this build wrote can fail to be the sentinel.
            let planted = SurfaceEnv {
                config: Arc::new(Config::starter()),
                ..env.clone()
            };
            set_surface_env(planted.clone());
            panel.build();

            let seen = seen
                .borrow()
                .clone()
                .expect("a panel builds inside a surface scope");
            assert!(
                !Arc::ptr_eq(&seen.config, &planted.config),
                "the {name} built without installing a `SurfaceEnv`: every module, icon lookup and \
                 `panel_fill` inside it silently resolves the global config instead of this screen's, and no \
                 per-monitor override ever reaches it"
            );
            if let Some(want) = want {
                assert_eq!(seen.edge, want, "the {name} resolves against the wrong bar");
            }
        }
    }

    /// The other half: the test above proves a [`PanelSurface`] installs its environment, and this proves nothing opens a window without being one.
    ///
    /// The check that would have caught the original bug on the day it was written. Nothing about a raw `open_surface` call is wrong-looking — it is how a surface was opened for as long as there have been surfaces — so eleven of them accumulated, each a little different, and the difference that mattered was invisible in every one.
    ///
    /// One exception, and it is the three non-panels in the module doc: the reconciler mounts the bars, the wallpaper and the frame. The fourth, the lock screen, is mounted by the compositor's lock session and never opens a surface of its own.
    #[test]
    fn nothing_opens_a_window_except_through_a_panel_surface() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(std::path::Path::parent)
            .expect("the workspace root is two levels above this crate")
            .to_path_buf();
        // Where a surface that is *not* a panel is opened. The reconciler owns the three the shell reconciles against the config; the platform crate is the door itself.
        let allowed = [
            "crates/surfaces/src/reconcile.rs",
            "crates/ui/src/panel.rs",
            "crates/platform-wayland/src",
            // The benchmark's per-surface mode measures today's raw layer surfaces, deliberately outside the shell's config environment; its merged mode opens a layer window instead.
            "apps/spike/",
        ];
        let mut offenders = Vec::new();
        let mut stack = vec![root.join("crates"), root.join("apps")];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = dir.read_dir() else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                // `.telar/build` is the transpiler's own output, not source.
                if path.is_dir() {
                    if path.file_name().and_then(|n| n.to_str()) != Some(".telar") {
                        stack.push(path);
                    }
                    continue;
                }
                let relative = path
                    .strip_prefix(&root)
                    .unwrap_or(&path)
                    .display()
                    .to_string();
                let is_source = path
                    .extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| e == "rs" || e == "rsx");
                if !is_source || allowed.iter().any(|ok| relative.starts_with(ok)) {
                    continue;
                }
                let Ok(text) = std::fs::read_to_string(&path) else {
                    continue;
                };
                if text.contains("open_surface(") {
                    offenders.push(relative);
                }
            }
        }
        offenders.sort();
        assert!(
            offenders.is_empty(),
            "these open a window without going through `PanelSurface`, so nothing installs a `SurfaceEnv` for \
             it and its content resolves the global config: {offenders:#?}"
        );
    }

    /// The radius comes from the panel's own environment, so the drawer and the float showing the same panel round their cards the same way — which they did not while each carried its own copy of the number.
    #[test]
    fn content_rounds_to_the_bar_of_the_edge_the_panel_hangs_off() {
        let config = Arc::new(Config::starter());
        for edge in Edge::ALL {
            set_surface_env(SurfaceEnv::for_edge(Arc::clone(&config), edge, None));
            assert_eq!(content_radius(), config.panel_radius(edge), "{edge:?}");
        }
    }
}
