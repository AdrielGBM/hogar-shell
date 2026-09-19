//! What the config says should be on screen, and keeping the screen in step with it.
//!
//! The shell puts up five kinds of surface on its own: a wallpaper, the widgets drawn over it, a bar per edge, the invisible strip each bar reserves, and the frame ring. Which of them exist, how big they are and where they sit are all answers to the config, and the config is a file the user edits while the shell is running — so this module holds both halves: [`plan`], which reads the config as a set of surfaces, and [`Surfaces`], which owns the live ones and brings them in line with a new plan.
//!
//! **A reload reuses, it does not replace.** Every surface here has a [`Key`] that survives an edit — what it is, and which screen it is on — so a config change reaches the surface that is already up: its layer-shell state is renegotiated in place and its content is built again on the same surface. A surface is created only when the config asks for one that is not there (a bar given its first module, a monitor plugged in) and destroyed only when the config stops asking for it. Nothing blinks in between, which is what makes editing the config with the settings window open bearable.

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use platform_wayland::{Layer, LayerConfig, OutputDescriptor, SurfaceHandle};

use crate::bar::BarApp;
use crate::frame::FrameApp;
use crate::wallpaper::WallpaperApp;
use crate::widgets::WidgetsApp;
use config::{Config, Edge};
use ui::placement::Placement;

/// What a shell-owned surface is for. Its whole identity beyond the screen it is on — two surfaces with the same role on the same output are the same surface, before and after any edit.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Role {
    Wallpaper,
    /// What is drawn on the desktop rather than behind it — the clock face, the visualiser. Its own surface, so a widget that repaints with the music does not repaint the wallpaper under it.
    Widgets,
    Bar(Edge),
    /// The invisible strip that carves an edge's space out of the screen. A separate surface from the bar it reserves for, so the two are reconciled independently — an auto-hiding bar keeps its ring's strip while giving up its own.
    Reserve(Edge),
    Frame,
}

impl Role {
    /// Whether this surface draws anything. A reservation strip is an exclusive zone and a transparent buffer, so there is nothing in it for a config change to rebuild.
    fn draws(self) -> bool {
        !matches!(self, Role::Reserve(_))
    }
}

/// A surface's identity across a reload.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
struct Key {
    output: Option<String>,
    role: Role,
}

/// One surface the config calls for: what it is, how the compositor should place it, and the config its content resolves against — the global one merged with this monitor's override.
struct Planned {
    key: Key,
    layer: LayerConfig,
    config: Arc<Config>,
}

/// What a reconcile does to the surfaces that stay.
///
/// A config edit changes what a bar draws, so every surface that survives it builds again. A monitor being plugged in does not: rebuilding the other screens' bars for it would throw away their state to redraw exactly what was already there.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Content {
    Rebuild,
    Keep,
}

/// The live surfaces, keyed so a reload finds the one it is about.
#[derive(Default)]
pub struct Surfaces {
    live: Vec<(Key, Live)>,
}

/// What adopting a surface actually did to it: renegotiated its layer-shell state with the compositor, rebuilt its content, both, or — for a strip that neither draws nor moved — neither.
struct Adopted {
    renegotiated: bool,
    rebuilt: bool,
}

struct Live {
    handle: SurfaceHandle,
    /// The layer-shell state the compositor currently has for this surface, to diff the next plan against.
    layer: LayerConfig,
    config: config::LiveConfig,
}

impl Surfaces {
    /// Brings the screen in line with `config`: renegotiates and rebuilds what is already up, opens what the config newly asks for, and drops what it no longer does.
    ///
    /// Closing runs first so an edge that lost its bar gives its exclusive zone back before the surfaces that stay are measured against it — otherwise every one of them would be configured once against the old zone and again a frame later.
    pub fn reconcile(
        &mut self,
        path: &Path,
        config: &Arc<Config>,
        outputs: &[OutputDescriptor],
        content: Content,
    ) {
        let plan = plan(path, config, outputs);
        let wanted: HashSet<&Key> = plan.iter().map(|planned| &planned.key).collect();
        let before = self.live.len();
        self.live.retain(|(key, _)| wanted.contains(key));
        let closed = before - self.live.len();

        // Layer-shell stacks surfaces of one layer in the order they were created, so a surface created under one that already exists would come out on top of it — a wallpaper switched on while the frame ring is up would cover the ring. Whatever the plan puts *after* a newly created surface in the same layer on the same screen is therefore created again, in its place in the order.
        let mut restacking: HashSet<(Option<String>, Layer)> = HashSet::new();
        let (mut opened, mut renegotiated, mut rebuilt) = (0, 0, 0);
        for planned in plan {
            let stack = (planned.key.output.clone(), planned.layer.layer);
            match self.index_of(&planned.key) {
                Some(index) if !restacking.contains(&stack) => {
                    let done = self.live[index].1.adopt(planned, content);
                    renegotiated += done.renegotiated as usize;
                    rebuilt += done.rebuilt as usize;
                }
                Some(index) => {
                    self.live.remove(index);
                    self.open(planned);
                    opened += 1;
                }
                None => {
                    restacking.insert(stack);
                    self.open(planned);
                    opened += 1;
                }
            }
        }
        // What the pass did to the screen, in one line, at a level the user already sees. A change that reaches the config but not the screen is either a surface that was not rebuilt or one the compositor did not act on, and those are indistinguishable without this. Per-surface detail is a `debug!` below.
        tracing::info!(
            surfaces = self.live.len(),
            opened,
            closed,
            renegotiated,
            rebuilt,
            "surfaces reconciled"
        );
    }

    /// How many surfaces are up. `hogar-shell` logs it at startup and the tests read it.
    pub fn len(&self) -> usize {
        self.live.len()
    }

    pub fn is_empty(&self) -> bool {
        self.live.is_empty()
    }

    fn index_of(&self, key: &Key) -> Option<usize> {
        self.live.iter().position(|(candidate, _)| candidate == key)
    }

    #[cfg(test)]
    fn roles(&self) -> Vec<Role> {
        self.live.iter().map(|(key, _)| key.role).collect()
    }

    /// The cell a live surface reads its config from, which is the identity a test compares: the same cell before and after an edit means the same surface.
    #[cfg(test)]
    fn config_of(&self, role: Role) -> Option<config::LiveConfig> {
        self.live
            .iter()
            .find(|(key, _)| key.role == role)
            .map(|(_, live)| live.config.clone())
    }

    fn open(&mut self, planned: Planned) {
        self.live.push((planned.key.clone(), Live::open(planned)));
    }
}

impl Live {
    fn open(planned: Planned) -> Self {
        let Planned { key, layer, config } = planned;
        let config = config::LiveConfig::new(config);
        let output = key.output.clone();
        let handle = match key.role {
            Role::Reserve(_) => platform_wayland::open_reservation(layer.clone()),
            Role::Bar(edge) => platform_wayland::open_surface(
                layer.clone(),
                BarApp {
                    config: config.clone(),
                    edge,
                    output,
                },
            ),
            Role::Wallpaper => platform_wayland::open_surface(
                layer.clone(),
                WallpaperApp {
                    config: config.clone(),
                    output,
                },
            ),
            Role::Widgets => platform_wayland::open_surface(
                layer.clone(),
                WidgetsApp {
                    config: config.clone(),
                },
            ),
            Role::Frame => platform_wayland::open_surface(
                layer.clone(),
                FrameApp {
                    config: config.clone(),
                },
            ),
        };
        Self {
            handle,
            layer,
            config,
        }
    }

    /// Takes on what the plan now says this surface should be: the config its content resolves against, the layer-shell state the compositor holds for it, and — for a config change — a rebuild of its content.
    fn adopt(&mut self, planned: Planned, content: Content) -> Adopted {
        self.config.set(planned.config);
        let change = self.layer.delta(&planned.layer);
        let renegotiated = !change.is_empty();
        if renegotiated {
            self.handle.update(change);
        }
        self.layer = planned.layer;
        let rebuilt = content == Content::Rebuild && planned.key.role.draws();
        if rebuilt {
            self.handle.rebuild();
        }
        tracing::debug!(
            role = ?planned.key.role,
            size = ?self.layer.size,
            renegotiated,
            rebuilt,
            "surface adopted"
        );
        Adopted {
            renegotiated,
            rebuilt,
        }
    }
}

/// Every surface `config` calls for, across every output, in the order they must be created.
fn plan(path: &Path, config: &Arc<Config>, outputs: &[OutputDescriptor]) -> Vec<Planned> {
    outputs
        .iter()
        .flat_map(|out| {
            // Every surface on this output resolves against the same merged config, so a per-monitor override reaches the bar, its reservation strip, the wallpaper and the frame together — a bar sized by one config and a reservation strip sized by another would carve the wrong zone out of the screen.
            let config = output_config(path, config, out.name.as_deref());
            if let Some(name) = out.name.as_deref() {
                config::set_output_config(name, Arc::clone(&config));
            }
            plan_output(&config, out.name.as_deref())
        })
        .collect()
}

/// One output's surfaces, in stacking order within each layer: the wallpaper first so it sits at the bottom of the background layer, then its widgets over it, then the frame ring and the bars on the chrome layer.
fn plan_output(config: &Arc<Config>, output: Option<&str>) -> Vec<Planned> {
    let mut planned = Vec::new();
    let mut push = |role: Role, placement: Placement| {
        planned.push(Planned {
            key: Key {
                output: output.map(str::to_string),
                role,
            },
            layer: placement.layer_config(),
            config: Arc::clone(config),
        });
    };
    if config.background.is_enabled() {
        push(Role::Wallpaper, wallpaper_placement(output));
    }
    // After the wallpaper, so it stacks over it in the same layer.
    if config.widgets.is_enabled() {
        push(Role::Widgets, widgets_placement(config, output));
    }
    // Before the bars, and on their layer rather than the wallpaper's. The ring *is* the bars' own edge continued around the screen, so it has to be where they are: on the background it was painted behind every window, which left the strip a framed bar draws nothing in showing the app through it. Before them, because the ring covers exactly the strips the bars occupy — drawn after, it would paint over the chips in `chips` and `sections` mode.
    if config.shape.frame {
        push(Role::Frame, frame_placement(config, output));
    }
    for edge in Edge::ALL {
        if !config.edge_present(edge) || config.bars.excludes(output) {
            continue;
        }
        push(Role::Bar(edge), bar_placement(config, edge, output));
        // Driven off what the edge reserves rather than off whether its bar hides: an auto-hidden bar under `[shape] frame` still reserves its ring, and an edge that reserves nothing gets no strip at all rather than one sized zero — a mapped surface with an empty exclusive zone is still a surface for the compositor to configure and the driver to drive.
        if config.edge_reserved(edge) > 0 {
            push(
                Role::Reserve(edge),
                reservation_placement(config, edge, output),
            );
        }
    }
    planned
}

/// The config `output` runs under: its `monitors/<output>/config.toml` merged over the global one, falling back to the global config when it has no override or that override will not parse. A broken per-monitor file costs that one screen its overrides and a log line, never the whole shell's layout.
fn output_config(path: &Path, global: &Arc<Config>, output: Option<&str>) -> Arc<Config> {
    let Some(output) = output else {
        return Arc::clone(global);
    };
    match Config::for_output(path, Some(output)) {
        Ok(config) => Arc::new(config),
        Err(e) => {
            tracing::warn!("monitor '{output}': {e}; using the global config");
            Arc::clone(global)
        }
    }
}

/// The layer the shell's own chrome sits on: `Overlay` keeps the bars above a fullscreen window when `[general] show_over_fullscreen` asks for it, `Top` (the default) lets fullscreen cover them.
fn chrome_layer(config: &Config) -> Layer {
    if config.general.show_over_fullscreen {
        Layer::Overlay
    } else {
        Layer::Top
    }
}

/// A bar: spans its edge, reserves nothing of its own (its strip does that), and sits on whichever layer `[general] show_over_fullscreen` asks for.
///
/// An auto-hidden bar is created at its hidden margin — off its own edge but for its peek strip — rather than being placed on screen and moved a frame later, which the user would see as a bar that flashes on at every reload before deciding to leave.
fn bar_placement(config: &Config, edge: Edge, output: Option<&str>) -> Placement {
    let shown = config::bar_margin_for(config, edge);
    let margin = if config.bar_is_persistent(edge) {
        shown
    } else {
        crate::bar::RevealMargins::new(config, edge, shown).hidden
    };
    Placement::bar(edge, config.edge_thickness(edge))
        .layer(chrome_layer(config))
        .margin(margin)
        .output(output.map(str::to_string))
}

fn reservation_placement(config: &Config, edge: Edge, output: Option<&str>) -> Placement {
    Placement::reservation(edge, config.edge_reserved(edge)).output(output.map(str::to_string))
}

/// The picture behind the desktop: the whole screen, under every window, click-through.
fn wallpaper_placement(output: Option<&str>) -> Placement {
    Placement::backdrop("hogar-shell-wallpaper").output(output.map(str::to_string))
}

/// The desktop's widgets: the area the bars left free, over the wallpaper and under every window.
///
/// The placement is the whole design. `Placement::desktop` respects the bars' exclusive zones where the wallpaper opts out of them, so the compositor hands this surface the hole the bars leave rather than the screen — no arithmetic here, and it follows a bar appearing or an edge being emptied on its own. The margin on top is the gap every panel keeps off its edge, read per edge so the widgets sit exactly where an application's window would.
fn widgets_placement(config: &Config, output: Option<&str>) -> Placement {
    let gap = |edge| config.panel_gap(edge) as i32;
    Placement::desktop("hogar-shell-widgets")
        .margin((
            gap(Edge::Top),
            gap(Edge::Right),
            gap(Edge::Bottom),
            gap(Edge::Left),
        ))
        .output(output.map(str::to_string))
}

/// The frame ring: the same full-screen click-through shape as the wallpaper, on the *bars'* layer.
///
/// It is the bars' own edge continued around the screen — under `[shape] frame` a bar paints no background at all and the ring draws that strip instead — so a ring on the background layer is a bar that disappears behind whatever window is under it. Click-through, so sharing the bars' layer costs the bars nothing.
fn frame_placement(config: &Config, output: Option<&str>) -> Placement {
    Placement::backdrop("hogar-shell-frame")
        .layer(chrome_layer(config))
        .output(output.map(str::to_string))
}

#[cfg(test)]
mod tests {
    use super::*;
    use platform_wayland::Anchor;

    fn config(toml: &str) -> Arc<Config> {
        Arc::new(toml::from_str(toml).unwrap())
    }

    fn roles(config: &Arc<Config>) -> Vec<Role> {
        plan_output(config, None)
            .into_iter()
            .map(|p| p.key.role)
            .collect()
    }

    #[test]
    fn the_plan_is_what_the_config_asks_for() {
        let bare = config("[bars.top]\ncenter=[\"clock\"]\n");
        assert_eq!(
            roles(&bare),
            vec![Role::Bar(Edge::Top), Role::Reserve(Edge::Top)],
            "one bar is a bar and the strip it reserves; nothing else is asked for"
        );

        let dressed = config(
            "[background]\nenabled=true\n[shape]\nframe=true\n[bars.top]\ncenter=[\"clock\"]\n",
        );
        assert_eq!(
            roles(&dressed).first(),
            Some(&Role::Wallpaper),
            "the wallpaper is planned first so it stacks under everything on the background layer"
        );
        let order = roles(&dressed);
        let at = |role: Role| order.iter().position(|planned| *planned == role);
        assert!(
            at(Role::Frame) < at(Role::Bar(Edge::Top)),
            "the ring is planned before the bars it shares a layer with: it covers exactly the strips they \
             occupy, so drawn after them it would paint over the chips"
        );
    }

    /// The ring belongs on the bars' layer, not the wallpaper's.
    ///
    /// It shipped on the background, which looked right on an empty desktop and was wrong the moment a window covered it: a framed bar paints no background of its own — the ring draws that strip — so the strip showed the *application* through, and there was nothing on the bar's layer for a compositor blur rule to find either.
    #[test]
    fn the_frame_ring_sits_with_the_bars_rather_than_behind_the_windows() {
        let framed = config("[shape]\nframe=true\n[bars.top]\ncenter=[\"clock\"]\n");
        let layer_of = |role: Role| {
            plan_output(&framed, None)
                .into_iter()
                .find(|planned| planned.key.role == role)
                .map(|planned| planned.layer.layer)
        };
        assert_eq!(layer_of(Role::Frame), layer_of(Role::Bar(Edge::Top)));

        // And it follows the bars when they move over a fullscreen window, or it would be left behind on a layer they no longer share.
        let over = config(
            "[general]\nshow_over_fullscreen=true\n[shape]\nframe=true\n[bars.top]\ncenter=[\"clock\"]\n",
        );
        let frame_layer = plan_output(&over, None)
            .into_iter()
            .find(|planned| planned.key.role == Role::Frame)
            .map(|planned| planned.layer.layer);
        assert_eq!(frame_layer, Some(Layer::Overlay));

        // The wallpaper stays where it belongs: a picture behind the desktop, not chrome over it.
        let papered = config("[background]\nenabled=true\n[shape]\nframe=true\n");
        let wallpaper = plan_output(&papered, None)
            .into_iter()
            .find(|planned| planned.key.role == Role::Wallpaper)
            .map(|planned| planned.layer.layer);
        assert_eq!(wallpaper, Some(Layer::Background));
    }

    /// The widgets surface exists on its own terms, and is placed against the space the bars left.
    ///
    /// Both halves are the point of splitting it off the wallpaper. A clock is not a reason to paint the desktop, so the two surfaces are asked for separately — and where the wallpaper opts out of every exclusive zone to take the screen, this one respects them, which is what makes the compositor size it to the hole the bars leave instead of this file working that out by hand.
    #[test]
    fn the_widgets_surface_is_asked_for_on_its_own_and_measures_what_the_bars_left() {
        let widgets_only =
            config("[widgets.clock]\nenabled=true\n[bars.top]\ncenter=[\"clock\"]\n");
        assert_eq!(
            roles(&widgets_only),
            vec![
                Role::Widgets,
                Role::Bar(Edge::Top),
                Role::Reserve(Edge::Top)
            ],
            "a widget asks for its own surface and for no wallpaper behind it"
        );

        let both = config("[background]\nenabled=true\n[widgets.visualiser]\nenabled=true\n");
        assert_eq!(
            roles(&both),
            vec![Role::Wallpaper, Role::Widgets],
            "and when there is a wallpaper, the widgets are created after it so they stack over it"
        );

        let planned = |cfg: &Arc<Config>, role: Role| {
            plan_output(cfg, None)
                .into_iter()
                .find(|p| p.key.role == role)
                .map(|p| p.layer)
                .expect("the surface is planned")
        };
        let widgets = planned(&both, Role::Widgets);
        assert_eq!(widgets.layer, Layer::Background);
        assert!(
            widgets.input_transparent,
            "click-through, like the wallpaper"
        );
        assert_eq!(
            widgets.exclusive_zone, 0,
            "zero respects the bars' strips where the wallpaper's -1 ignores them"
        );
        assert_eq!(
            planned(&both, Role::Wallpaper).exclusive_zone,
            -1,
            "and the wallpaper still takes the whole screen"
        );

        // The gap off each edge is the one every panel keeps there, so a widget lines up with the applications.
        let floating = config(
            "[shape]\ngap=14\n[widgets.clock]\nenabled=true\n[bars.top]\ncenter=[\"clock\"]\n",
        );
        assert_eq!(planned(&floating, Role::Widgets).margin, (14, 14, 14, 14));
    }

    /// `[shape] inactive_size` is what an edge with nothing on it is worth: the strip it reserves, the surface its empty bar takes, and the ring the frame draws there. All three follow an edit to it, or the space the windows are kept out of and the ring painted in it disagree about where the screen ends.
    #[test]
    fn inactive_size_resizes_every_surface_on_an_empty_edge() {
        let thin = config("[shape]\nframe=true\ninactive_size=8\n[bars.top]\ncenter=[\"clock\"]\n");
        let thick =
            config("[shape]\nframe=true\ninactive_size=20\n[bars.top]\ncenter=[\"clock\"]\n");
        let planned = |cfg: &Arc<Config>, role: Role| {
            plan_output(cfg, None)
                .into_iter()
                .find(|p| p.key.role == role)
                .map(|p| p.layer)
                .expect("the edge is planned")
        };
        assert_ne!(
            planned(&thin, Role::Bar(Edge::Right)).size,
            planned(&thick, Role::Bar(Edge::Right)).size,
            "the empty edge's own surface is sized by inactive_size"
        );
        assert_ne!(
            planned(&thin, Role::Reserve(Edge::Right)).exclusive_zone,
            planned(&thick, Role::Reserve(Edge::Right)).exclusive_zone,
            "and so is the strip it carves out of the screen"
        );
        assert_ne!(
            thin.edge_thickness(Edge::Right),
            thick.edge_thickness(Edge::Right),
            "which is what the frame ring draws its inner edge from"
        );
    }

    /// `[shape] frame` gives every edge a thickness of its own, so all four bars exist whatever is on them — which is why turning a module off there is a re-render and not a surface being destroyed.
    #[test]
    fn a_frame_puts_a_surface_on_every_edge() {
        let framed =
            config("[shape]\nframe=true\ninactive_size=6\n[bars.top]\ncenter=[\"clock\"]\n");
        for edge in Edge::ALL {
            assert!(
                roles(&framed).contains(&Role::Bar(edge)),
                "{edge:?} is present under a frame even with nothing on it"
            );
        }

        let bare = config("[bars.top]\ncenter=[\"clock\"]\n");
        assert!(
            !roles(&bare).contains(&Role::Bar(Edge::Left)),
            "without one, an empty edge has no surface at all"
        );
    }

    #[test]
    fn a_screen_the_bars_exclude_keeps_its_wallpaper_and_loses_its_bars() {
        let config = config(
            "[background]\nenabled=true\n\
             [bars]\nexcluded_screens=[\"HDMI-*\"]\n\
             [bars.top]\ncenter=[\"clock\"]\n",
        );
        let excluded: Vec<Role> = plan_output(&config, Some("HDMI-A-1"))
            .into_iter()
            .map(|p| p.key.role)
            .collect();
        assert_eq!(
            excluded,
            vec![Role::Wallpaper],
            "an excluded screen is excluded from the bars, not from the desktop"
        );
    }

    /// The whole point of the reconcile: an edit reaches the surfaces that are already up.
    ///
    /// Reusing is not a detail of how it is implemented — it is what the user sees. A bar replaced by a new bar is a bar that blinks off and on at every keystroke in the settings window, which is what this used to do.
    #[test]
    fn an_edit_reuses_the_surfaces_that_stay_and_only_adds_or_drops_at_the_edges() {
        let path = Path::new("config.toml");
        let screen = [OutputDescriptor {
            name: None,
            logical_size: Some((1920, 1080)),
            position: (0, 0),
            scale: 1,
        }];
        let mut surfaces = Surfaces::default();
        surfaces.reconcile(
            path,
            &config("[bars.top]\nsize=34\ncenter=[\"clock\"]\n"),
            &screen,
            Content::Rebuild,
        );
        let before = surfaces
            .config_of(Role::Bar(Edge::Top))
            .expect("the top bar is up");
        assert_eq!(
            surfaces.roles(),
            vec![Role::Bar(Edge::Top), Role::Reserve(Edge::Top)]
        );

        surfaces.reconcile(
            path,
            &config("[bars.top]\nsize=48\ncenter=[\"clock\"]\n[bars.bottom]\nstart=[\"clock\"]\n"),
            &screen,
            Content::Rebuild,
        );
        let after = surfaces
            .config_of(Role::Bar(Edge::Top))
            .expect("and is still up");
        assert!(
            before.ptr_eq(&after),
            "a thicker bar is the same bar: the surface is renegotiated and rebuilt, never replaced"
        );
        assert_eq!(
            after.get().bars.get(Edge::Top).size,
            48,
            "and its next build reads the config the edit produced"
        );
        assert_eq!(
            surfaces.roles(),
            vec![
                Role::Bar(Edge::Top),
                Role::Reserve(Edge::Top),
                Role::Bar(Edge::Bottom),
                Role::Reserve(Edge::Bottom)
            ],
            "the bottom bar the config newly asks for is the only thing opened"
        );

        surfaces.reconcile(
            path,
            &config("[bars.bottom]\nstart=[\"clock\"]\n"),
            &screen,
            Content::Rebuild,
        );
        assert_eq!(
            surfaces.roles(),
            vec![Role::Bar(Edge::Bottom), Role::Reserve(Edge::Bottom)],
            "an edge the config stopped asking for is the one thing closed"
        );
    }

    /// A monitor that goes away takes its surfaces with it.
    ///
    /// The other way a surface stops being wanted: not the config dropping it, but the screen it was on. The plan is built from the outputs it is handed, so an output that stops being one has nothing planned against it — and everything the plan does not name is dropped, per-output surfaces included. Left behind, they would be layer surfaces on a screen the compositor no longer has.
    #[test]
    fn a_disconnected_output_takes_its_surfaces_with_it() {
        // A real path, unlike the relative one the other tests hand over: a *named* output sends the reconcile through `Config::for_output`, which falls back to `Config::load` — and loading a config that is not there writes a starter one, next to whatever the path pointed at.
        let dir = std::env::temp_dir().join(format!("hogar-shell-outputs-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        let path = dir.join("config.toml");
        let toml = "[bars.top]\nsize=34\ncenter=[\"clock\"]\n";
        std::fs::write(&path, toml).expect("a config to load");

        let screen = |name: &str, x: i32| OutputDescriptor {
            name: Some(name.to_string()),
            logical_size: Some((1920, 1080)),
            position: (x, 0),
            scale: 1,
        };
        let laptop = screen("eDP-1", 0);
        let config = config(toml);

        let mut surfaces = Surfaces::default();
        surfaces.reconcile(
            &path,
            &config,
            &[laptop.clone(), screen("DP-1", 1920)],
            Content::Rebuild,
        );
        let on_two_screens = surfaces.len();
        assert!(on_two_screens > 0, "two screens put surfaces up");

        surfaces.reconcile(
            &path,
            &config,
            std::slice::from_ref(&laptop),
            Content::Rebuild,
        );
        assert_eq!(
            surfaces.len(),
            on_two_screens / 2,
            "the unplugged screen's surfaces went with it, and the laptop's stayed"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn only_what_changed_is_renegotiated() {
        let before = config("[bars.top]\nsize=34\ncenter=[\"clock\"]\n");
        let after = config("[bars.top]\nsize=48\ncenter=[\"clock\"]\n");
        let change = bar_placement(&before, Edge::Top, None)
            .layer_config()
            .delta(&bar_placement(&after, Edge::Top, None).layer_config());
        assert_eq!(change.size, Some((0, 48)), "the bar got thicker");
        assert_eq!(
            (change.margin, change.exclusive_zone, change.anchor),
            (None, None, None),
            "and nothing else moved, so nothing else is committed"
        );

        assert!(
            bar_placement(&before, Edge::Top, None)
                .layer_config()
                .delta(&bar_placement(&before, Edge::Top, None).layer_config())
                .is_empty(),
            "an edit that misses this bar costs it no commit at all"
        );
    }

    /// `[general] show_over_fullscreen` is the one non-geometry thing a live bar renegotiates.
    #[test]
    fn raising_the_bars_over_fullscreen_moves_the_live_surface() {
        let under = config("[bars.top]\ncenter=[\"clock\"]\n");
        let over = config("[general]\nshow_over_fullscreen=true\n[bars.top]\ncenter=[\"clock\"]\n");
        let change = bar_placement(&under, Edge::Top, None)
            .layer_config()
            .delta(&bar_placement(&over, Edge::Top, None).layer_config());
        assert_eq!(change.layer, Some(Layer::Overlay));
    }

    #[test]
    fn visible_bars_reserve_nothing_and_pin_deterministically() {
        let cfg = config("[bars.top]\ncenter=[\"clock\"]\n[bars.bottom]\nstart=[\"clock\"]\n");
        for edge in [Edge::Top, Edge::Bottom] {
            let lc = bar_placement(&cfg, edge, None).layer_config();
            assert_eq!(lc.size, (0, 34), "{edge:?} leaves width free, pins height");
            assert_eq!(lc.exclusive_zone, -1, "visible bar reserves nothing");
            assert!(!lc.reserve_only);
            assert_eq!(lc.margin, (0, 0, 0, 0));
            assert!(lc.anchor.contains(Anchor::LEFT) && lc.anchor.contains(Anchor::RIGHT));
        }
        let top = bar_placement(&cfg, Edge::Top, None).layer_config().anchor;
        assert!(top.contains(Anchor::TOP) && !top.contains(Anchor::BOTTOM));
        assert!(
            bar_placement(&cfg, Edge::Bottom, None)
                .layer_config()
                .anchor
                .contains(Anchor::BOTTOM)
        );
    }

    #[test]
    fn reservation_strip_carves_thickness_along_full_edge() {
        let cfg = config("[bars.left]\nsize=44\nstart=[\"workspaces\"]\n");
        let r = reservation_placement(&cfg, Edge::Left, None).layer_config();
        assert!(r.reserve_only);
        assert!(
            r.input_transparent,
            "click-through so it never swallows the bar's input"
        );
        assert!(
            matches!(r.layer, Layer::Bottom),
            "spacers live below the bars, not on Top"
        );
        assert_eq!(r.exclusive_zone, 44, "reserves the bar thickness");
        assert_eq!(r.size, (44, 0));
        assert_eq!(r.margin, (0, 0, 0, 0));
        assert!(r.anchor.contains(Anchor::TOP) && r.anchor.contains(Anchor::BOTTOM));
    }

    #[test]
    fn floating_bar_gains_outer_and_end_margins_reservation_takes_gap() {
        let cfg = config("[shape]\ngap=8\nradius=12\n[bars.top]\nsize=34\ncenter=[\"clock\"]\n");
        let lc = bar_placement(&cfg, Edge::Top, None).layer_config();
        assert_eq!(lc.margin, (8, 8, 0, 8));
        assert_eq!(lc.exclusive_zone, -1);
        let r = reservation_placement(&cfg, Edge::Top, None).layer_config();
        assert_eq!(r.exclusive_zone, 34 + 8);
    }

    #[test]
    fn vertical_bar_ends_inset_by_an_adjacent_bars_thickness() {
        let cfg = config(
            "[bars.top]\nsize=30\ncenter=[\"clock\"]\n\
             [bars.bottom]\nsize=40\nstart=[\"clock\"]\n\
             [bars.left]\nsize=44\nstart=[\"workspaces\"]\n",
        );
        let left = bar_placement(&cfg, Edge::Left, None).layer_config();
        assert_eq!(left.margin, (30, 0, 40, 0));
        let top = bar_placement(&cfg, Edge::Top, None).layer_config();
        assert_eq!(top.margin, (0, 0, 0, 0));
    }

    #[test]
    fn vertical_bar_inset_uses_the_adjacent_bar_gap_not_its_own() {
        // Regression: a floating top bar (gap:8) ends at y=40, so a hugging left bar must inset by the top bar's gap+thickness, not its own — else it rides up over the top bar.
        let cfg = config(
            "[shape]\ngap=0\n\
             [bars.top]\nsize=32\ncenter=[\"clock\"]\n[bars.top.shape]\ngap=8\n\
             [bars.bottom]\nsize=64\nstart=[\"clock\"]\n\
             [bars.left]\nsize=32\nstart=[\"workspaces\"]\n",
        );
        let left = bar_placement(&cfg, Edge::Left, None).layer_config();
        assert_eq!(
            left.margin,
            (40, 0, 64, 0),
            "top inset = top gap(8)+thickness(32); bottom inset = bottom gap(0)+thickness(64)"
        );
    }

    #[test]
    fn frame_forces_hug_even_with_gap() {
        let cfg = config("[shape]\nframe=true\ngap=8\n[bars.top]\ncenter=[\"clock\"]\n");
        let lc = bar_placement(&cfg, Edge::Top, None).layer_config();
        assert_eq!(lc.margin, (0, 0, 0, 0));
        assert_eq!(lc.exclusive_zone, -1);
        let r = reservation_placement(&cfg, Edge::Top, None).layer_config();
        assert_eq!(r.exclusive_zone, 34);
    }
}
