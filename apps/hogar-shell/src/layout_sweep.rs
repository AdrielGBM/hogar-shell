//! Every preview, laid out on all four edges in all three shape modes, measured rather than merely built.
//!
//! `cargo telar test` renders each preview once and fails on a panic or a layout error, which is `assert!(build().is_ok())` — and that passes for every layout bug this tree has actually had: a result list laid out 612×**0** because a `max_height` is not a height, a wallpaper row measuring shorter than the tiles inside it, an avatar drawn outside its scrolling viewport. Each was found by looking at a picture, and each is a fact about *geometry*, which is why this measures the draw commands instead of comparing stored images: a baseline would also have to be told apart from a font that shaped differently on the machine running it, and a suite that cries wolf is one people delete.
//!
//! The twelve combinations are the shell's own hard rule — every surface and every module works on all four edges and in `bar`, `sections` and `chips` — so a module only ever looked at on a top bar is exactly where a shape bug survives.
//!
//! **"Did the preview draw anything at all?" took a correction to become askable.** It reported 79 of 456 combinations blank, and the reading taken from that was that `battery`, `brightness`, `mic`, `network`, `volume` and `lockstatus` have no reading on a machine with no battery and no PipeWire — a hardware-dependent answer, and so a flaky assertion. That was wrong twice over: those chips draw their glyph at a fallback level and are on screen, and what was blank was the *accounting*. Every icon is an SVG, an SVG is a `Path`, and only boxes were counted — so a chip whose whole content is an icon was invisible to this file rather than measured by it. [`paints`] counts artwork too, which makes the question machine-independent rather than merely askable.
//!
//! **What this cannot ask: *"did anything draw outside its surface?"* — the avatar bug.** Not for the reason it looks like. Draw rects being pre-transform was twelve lines of matrix stack, and they are now here ([`under_transform`]) — the markers are in the command stream even though telar keeps `DrawState` to itself; and content legitimately below the fold has a clean discriminator, since a scroll area emits a viewport and being outside one is what scrolling means, so a draw that nothing is clipping has no such reading. What blocks it is that **a preview has no bounds to be outside of**: `PreviewSurface` is a sizing hint rather than a viewport — `surfaces::preview` gives the bar 940 × its thickness on purpose — and entries stack their variants well past it. Measured, the check calls every such gallery a fault. Asking it needs real surfaces, which a sweep over previews does not have.

#![cfg(test)]

use std::sync::Arc;

use telar::{
    AvailableSpace, ComponentList, Container, DrawCommand, LayoutError, LayoutItem, LayoutStyle,
    Paint, Point, PreviewSurface, Rect, Transform, compute_layout, new_container,
    reset_layout_runtime, set_theme,
};

use config::{Config, Edge, Shape};
use ui::descriptor::{ChipFrame, ModuleDescriptor};
use ui::host::{Host, Instance, Representation, Size};

/// The page a preview is measured on when it is a tree rather than a surface. Wide enough that a bar-width module is not the thing under test.
const PAGE: (f32, f32) = (1000.0, 760.0);

/// Below this a rect is not something a user can see. Text shaping and fractional layout both land a fraction under a whole pixel routinely, so the question asked is "did this collapse", not "is this exact".
const COLLAPSED: f32 = 0.5;

const MODES: [Shape; 3] = [Shape::Bar, Shape::Sections, Shape::Chips];

/// The world one combination builds against. Deliberately [`Config::starter`] rather than the user's file: a sweep that read `~/.config/hogar-shell/config.toml` would measure a different shell on every machine.
///
/// `extra` is one more module at the end of the bar, for the check that an unknown id still holds a chip's place.
fn seed_world(edge: Edge, mode: Shape, extra: Option<&str>) {
    let config = Config::starter();
    layout::set_running(Arc::new(swept_layout(edge, mode, extra)));

    let config = Arc::new(config);
    services::locale::init(config.language());
    seed_home();
    ui::icon::init_store(&config.icons);
    set_theme(config.resolve_theme());
    config::set_config(config);
    crate::install_hooks();
    services::windows::seed(open_windows());
}

/// The layout one combination measures: the one the shell ships, with its bar moved to the edge under test and drawn in `mode`, plus `extra` placed at the end of it.
///
/// The built-in layout rather than a fixture written here, for the reason the sweep reads `Config::starter` rather than the user's file: what it measures has to be what this shell draws on a machine nobody has configured. A fixture would be a second description of the default, free to drift from the one in `crates/layout`.
fn swept_layout(edge: Edge, mode: Shape, extra: Option<&str>) -> layout::Layout {
    let shipped = layout::LayoutStore::safe(std::env::temp_dir());
    let mut swept = shipped.active().clone();
    for rule in &mut swept.outputs {
        for area in &mut rule.layers.top.areas {
            let Some(layout::AreaKind::Bar {
                edge: on, shape, ..
            }) = area.kind.as_mut()
            else {
                continue;
            };
            *on = Some(edge);
            shape.mode = Some(mode);
            let Some(extra) = extra else { continue };
            if let Some(end) = area
                .groups
                .iter_mut()
                .find(|group| matches!(group.kind, Some(layout::GroupKind::Zone { zone }) if zone == layout::Zone::End))
            {
                end.children.push(layout::Instance {
                    id: layout::InstanceId::new(extra),
                    module: Some(extra.to_string()),
                    representation: Some(layout::Representation::Chip),
                    ..layout::Instance::default()
                });
            }
        }
    }
    swept
}

/// The home a sweep measures in, checked in under `fixtures/home` and copied into this process's scratch home: the glyphs the previews draw, where a run with a network would have cached them, and the GTK settings that name the application icon theme. A test never downloads and never reads the user's own home, so the sweep brings both.
fn seed_home() {
    static SEEDED: std::sync::Once = std::sync::Once::new();
    SEEDED.call_once(|| {
        assert!(
            util::paths::isolated_root().is_some(),
            "a test seeds only its own scratch home"
        );
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/home");
        let home = util::paths::home_dir().expect("the scratch tree has a home");
        copy_tree(&fixture, &home);
    });
}

fn copy_tree(from: &std::path::Path, to: &std::path::Path) {
    std::fs::create_dir_all(to).expect("the scratch home is writable");
    for entry in from
        .read_dir()
        .expect("the fixture home is checked in")
        .flatten()
    {
        let (source, target) = (entry.path(), to.join(entry.file_name()));
        if source.is_dir() {
            copy_tree(&source, &target);
        } else {
            std::fs::copy(&source, &target).expect("the scratch home is writable");
        }
    }
}

/// One thing a sweep lays out: a preview entry, or one representation of a module in the descriptor table.
struct Subject {
    component_name: &'static str,
    preview_name: String,
    surface: Option<PreviewSurface>,
    source: Source,
}

enum Source {
    Preview(fn() -> Result<Box<dyn LayoutItem>, LayoutError>),
    Module(&'static ModuleDescriptor, Representation),
}

impl Subject {
    /// A filler chip: room on a bar rather than content, so it is the one subject with nothing to draw.
    fn is_room(&self) -> bool {
        matches!(
            self.source,
            Source::Module(module, Representation::Chip)
                if module.representations.chip.is_some_and(|chip| chip.frame == ChipFrame::Filler)
        )
    }

    fn build(&self) -> Result<Box<dyn LayoutItem>, LayoutError> {
        match self.source {
            Source::Preview(build) => build(),
            Source::Module(module, representation) => {
                let host = module_host(module.id, representation);
                telar::batch(|| module.build(&host)).unwrap_or_else(|| {
                    Err(LayoutError::Engine(format!(
                        "{} declares {representation:?} and does not build it",
                        module.id
                    )))
                })
            }
        }
    }
}

fn previews() -> Vec<Subject> {
    crate::preview_entries()
        .into_iter()
        .map(|entry| Subject {
            component_name: entry.component_name,
            preview_name: entry.preview_name.to_string(),
            surface: entry.surface,
            source: Source::Preview(entry.build),
        })
        .collect()
}

fn drawn_bar() -> (Arc<Config>, Edge, f32, config::ResolvedShape) {
    let config = config::config().expect("the sweep published a config");
    let running = layout::running().expect("the sweep published a layout");
    let (resolved, _) = layout::resolve(
        &running,
        &layout::Library::default(),
        layout::NOMINAL_OUTPUT,
        None,
    );
    let bar = resolved
        .areas()
        .find_map(|(_, area)| match area.kind {
            layout::ResolvedAreaKind::Bar {
                edge,
                thickness,
                shape,
                ..
            } => Some((edge, thickness, shape)),
            _ => None,
        })
        .expect("the seeded layout has a bar");
    let (edge, thickness, shape) = bar;
    let shape = surfaces::bar::bar_shape(&config, shape);
    (config, edge, thickness, shape)
}

/// A chip on the bar the seeded world draws; any other representation hosted at the size [`module_surface`] declares.
fn module_host(id: &str, representation: Representation) -> Host {
    let (config, edge, thickness, shape) = drawn_bar();
    let theme = config.resolve_theme();
    match representation {
        Representation::Chip => Host::placed(
            Instance::of_module(id),
            config,
            Representation::Chip,
            match edge.is_vertical() {
                true => Size {
                    width: thickness,
                    height: f32::INFINITY,
                },
                false => Size {
                    width: f32::INFINITY,
                    height: thickness,
                },
            },
            Some(edge),
            shape,
            theme.accent,
            ui::module::module_foreground(config::Variant::Default, theme.accent, theme),
            None,
        ),
        other => {
            let surface = module_surface(other);
            let extent = Size {
                width: surface.width,
                height: surface.height,
            };
            ui::preview::surface_host(id, other, extent)
        }
    }
}

fn module_surface(representation: Representation) -> PreviewSurface {
    let (_, edge, thickness, _) = drawn_bar();
    match representation {
        Representation::Chip if edge.is_horizontal() => PreviewSurface::new(940.0, thickness),
        Representation::Chip => PreviewSurface::new(thickness, 940.0),
        Representation::Popout => {
            let (width, height) = config::ModuleOverride::default().popout_size();
            PreviewSurface::new(width, height)
        }
        Representation::Widget(size) => {
            let extent = size.extent();
            PreviewSurface::new(extent.width, extent.height)
        }
        Representation::Card | Representation::Panel => PreviewSurface::new(420.0, 600.0),
    }
}

/// Every representation every module declares, which is what makes a module added to the table swept on every edge and shape without a preview written for it.
fn descriptors() -> Vec<Subject> {
    crate::core::modules::MODULES
        .iter()
        .flat_map(|module| {
            module
                .declared()
                .into_iter()
                .map(move |representation| Subject {
                    component_name: module.id,
                    preview_name: format!("{representation:?}"),
                    surface: Some(module_surface(representation)),
                    source: Source::Module(module, representation),
                })
        })
        .collect()
}

fn everything() -> Vec<Subject> {
    let mut subjects = previews();
    subjects.extend(descriptors());
    subjects
}

/// What the subject put on screen, in the coordinates its own draw commands carry, and the input region the same tree claims. Both are read before the tree is dropped, because dropping it withdraws every claim in it.
fn measure(subject: &Subject) -> Result<Measured, LayoutError> {
    let (width, height) = subject
        .surface
        .map(|surface| (surface.width, surface.height))
        .unwrap_or(PAGE);
    let page = || LayoutStyle::new().flex_column().width(width).height(height);

    let built = subject.build()?;
    let root_node = new_container(page(), &[built.layout_node()])?;
    let tree = ComponentList::new(Container::new(page(), vec![built])?);
    compute_layout(
        root_node,
        AvailableSpace::Definite(width),
        AvailableSpace::Definite(height),
    )?;
    Ok((tree.commands().to_vec(), telar::interactive_rects()))
}

/// The **layout box** a command is answerable for, and whether it has any content to put there — an empty `Text` shapes to nothing and is the one zero-area draw that is not a fault. `Line` and `Path` carry artwork rather than a box, and the matrix and layer markers cover nothing of their own.
///
/// Deliberately narrower than [`paints`]: what this returns is measured against [`COLLAPSED`], and only a box the layout produced can be said to have collapsed. An icon's own geometry is the artwork's business — a signal-strength glyph draws its bars as filled slivers a third of a pixel wide, and there is nothing wrong with that.
///
/// The rect is the command's own, in whichever space it was emitted in, and that is sound here because a size is the same in both: a leaf's box is translated into place, never resized. Anything asking *where* a rect is has to put it through [`under_transform`] first.
fn painted_rect(command: &DrawCommand) -> Option<Rect> {
    match command {
        DrawCommand::Rect { rect, .. } | DrawCommand::Image { rect, .. } => Some(*rect),
        DrawCommand::Text { rect, text, .. } => (!text.is_empty()).then_some(*rect),
        // A viewport clipped to nothing is the canonical shape of the bug this file exists for: the content inside keeps its own honest rects and is cut away wholesale, so only the clip itself shows the fault.
        DrawCommand::PushClip { rect, .. } => Some(*rect),
        _ => None,
    }
}

/// A `Rect` with no fill, border or shadow draws nothing at any size, which is all a `display: none` container leaves behind: telar still emits its box at 0x0 and no command names the node it came from.
fn is_invisible_box(command: &DrawCommand) -> bool {
    matches!(
        command,
        DrawCommand::Rect { style, .. }
            if style.fill.is_none() && style.border.is_none() && style.shadow.is_none()
    )
}

/// Whether this command puts ink on the screen at all.
///
/// Wider than [`painted_rect`] by exactly the two commands that carry artwork: every icon in the shell is a path, and an `icon_glyph` chip — `battery`, `volume`, `network`, `mic`, `brightness`, `lockstatus` — draws nothing else. Counting only boxes made those modules invisible to this file rather than measured by it, which is why "did this draw anything" reported them blank on a machine where they render perfectly.
fn paints(command: &DrawCommand) -> bool {
    match command {
        DrawCommand::Path { data, .. } => data.bounds().is_some(),
        DrawCommand::Line { .. } => true,
        other => painted_rect(other).is_some(),
    }
}

/// The world a sweep seeds is process-global — the config, the theme, the default font family, the icon store — so two sweeps running at once measure each other's edge and shape. Every sweep is a `#[test]` of its own, and cargo runs them in parallel: the reading that came back was `activewindow` drawing nothing on ten combinations, roughly every other run, because it had been laid out against a bar some other test had just moved to a different edge.
///
/// The guard lives here rather than in each test so a sweep added later inherits it. Poisoning is ignored on purpose: a panicking test leaves the world half-set, and the next sweep re-seeds it from scratch before it measures anything.
static WORLD: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// What one combination is measured as: what it drew, and what of it the window claims for the pointer.
type Measured = (Vec<DrawCommand>, Vec<Rect>);

type Each<'a> = dyn FnMut(&Subject, Edge, Shape, Result<Measured, LayoutError>) + 'a;

fn sweep(mut each: impl FnMut(&Subject, Edge, Shape, Result<Vec<DrawCommand>, LayoutError>)) {
    sweep_over(everything, None, &mut |subject, edge, mode, measured| {
        each(subject, edge, mode, measured.map(|(commands, _)| commands))
    });
}

/// [`sweep`] with one more module placed at the end of the bar.
fn sweep_with(
    extra: &'static str,
    mut each: impl FnMut(&Subject, Edge, Shape, Result<Vec<DrawCommand>, LayoutError>),
) {
    sweep_over(
        everything,
        Some(extra),
        &mut |subject, edge, mode, measured| {
            each(subject, edge, mode, measured.map(|(commands, _)| commands))
        },
    );
}

/// [`sweep`] handed the claimed region as well as the draw commands.
fn sweep_claimed(mut each: impl FnMut(&Subject, Edge, Shape, Result<Measured, LayoutError>)) {
    sweep_over(everything, None, &mut each);
}

fn sweep_over(subjects: fn() -> Vec<Subject>, extra: Option<&str>, each: &mut Each) {
    let _world = WORLD
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    for edge in Edge::ALL {
        for mode in MODES {
            // Seeded before the list is drawn up, not only before each entry is measured: an entry reads the world to declare its surface — a bar's is its thickness, on the axis it runs along — so a list enumerated first describes whichever combination happened to run before this one.
            reset_layout_runtime();
            seed_world(edge, mode, extra);
            for subject in subjects() {
                reset_layout_runtime();
                seed_world(edge, mode, extra);
                // Scoped, and disposed before the next reset. Replacing the layout runtime starts its node ids over, so an unscoped entry keeps its effects running against ids the next entry now owns — and taffy answers a stale one with "invalid SlotMap key used".
                let scope = telar::owner_scope();
                let owner = scope.id();
                let measured = measure(&subject);
                drop(scope);
                each(&subject, edge, mode, measured);
                telar::dispose_owner(owner);
            }
        }
    }
}

/// Twelve times what `cargo telar test` covers: it renders each preview in whatever shape the config happens to carry, and every combination it does not render is one where a build or a layout can fail unseen.
#[test]
fn every_preview_lays_out_on_every_edge_and_shape() {
    let (mut checked, mut broken) = (0usize, Vec::new());
    sweep(|entry, edge, mode, measured| {
        checked += 1;
        if let Err(e) = measured {
            broken.push(format!(
                "{}::{} on {edge:?}/{mode:?} — {e}",
                entry.component_name, entry.preview_name
            ));
        }
    });
    assert!(checked > 0, "the sweep found no previews to measure");
    assert!(
        broken.is_empty(),
        "{} of {checked} combinations failed to lay out:\n  {}",
        broken.len(),
        broken.join("\n  ")
    );
}

/// The regression net proper. A draw command with no area is content the user cannot see, and it is what every layout bug in this tree's history looked like from underneath — the tree built, the pass succeeded, and the thing measured to nothing.
#[test]
fn nothing_a_preview_draws_collapses_to_nothing() {
    let mut collapsed = Vec::new();
    sweep(|entry, edge, mode, measured| {
        let Ok(commands) = measured else { return };
        for command in commands {
            let Some(rect) = painted_rect(&command) else {
                continue;
            };
            if is_invisible_box(&command) || (rect.width >= COLLAPSED && rect.height >= COLLAPSED) {
                continue;
            }
            collapsed.push(format!(
                "{}::{} on {edge:?}/{mode:?} — {} at {}x{}",
                entry.component_name,
                entry.preview_name,
                kind(&command),
                rect.width,
                rect.height
            ));
        }
    });
    assert!(
        collapsed.is_empty(),
        "{} draw(s) landed with no area, so nothing of them reaches the screen:\n  {}",
        collapsed.len(),
        collapsed.join("\n  ")
    );
}

/// The failure a collapsed rect cannot show: not a draw with no area, but no draw at all. A module that vanishes leaves nothing behind to measure, so the only question that catches it is asked of the whole combination rather than of any one command.
#[test]
fn every_preview_draws_something() {
    let mut blank = Vec::new();
    sweep(|entry, edge, mode, measured| {
        let Ok(commands) = measured else { return };
        if entry.is_room() || commands.iter().any(paints) {
            return;
        }
        blank.push(format!(
            "{}::{} on {edge:?}/{mode:?}",
            entry.component_name, entry.preview_name
        ));
    });
    assert!(
        blank.is_empty(),
        "{} combination(s) put nothing on screen:\n  {}",
        blank.len(),
        blank.join("\n  ")
    );
}

/// The id the unknown-module sweep puts on the starter bar: one letter off a real module, which is how it happens.
const UNKNOWN: &str = "clokc";

/// An id no module answers to is drawn where it was declared, on every edge and in every shape — laid out on the bar, a chip's size, and never collapsed.
///
/// It used to vanish: the bar skipped the entry with a log line, so the only sign a module had been asked for was its absence. The placeholder that replaces it is the one chip on the bar whose whole job is to be seen, which makes "it built" the wrong question twice over — a placeholder squeezed to a sliver, or pushed past the end of its zone, builds as happily as one on screen. So this measures it: the starter bar plus one misspelt id at the end of its last zone, on all four edges in `bar`, `sections` and `chips`.
#[test]
fn an_unknown_module_holds_a_chips_place_on_every_edge_and_shape() {
    let mut wrong = Vec::new();
    sweep_with(UNKNOWN, |entry, edge, mode, measured| {
        if entry.component_name != "bar" {
            return;
        }
        let commands = match measured {
            Ok(commands) => commands,
            Err(e) => {
                wrong.push(format!(
                    "{edge:?}/{mode:?}: the bar failed to lay out — {e}"
                ));
                return;
            }
        };
        let strip = surfaces::preview::bar_strip().expect("the previewed layout has a bar");
        let fill = ui::placeholder::fill(
            config::config()
                .expect("the sweep published a config")
                .resolve_theme(),
        );
        let icon = ui::preview::bar_chip().icon_size();

        let Some(rect) = commands.iter().find_map(|command| match command {
            DrawCommand::Rect { rect, style, .. } if style.fill == Some(Paint::Solid(fill)) => {
                Some(*rect)
            }
            _ => None,
        }) else {
            wrong.push(format!(
                "{edge:?}/{mode:?}: nothing was drawn for '{UNKNOWN}'"
            ));
            return;
        };
        // Only the part on the bar counts: a placeholder pushed past the end of its zone is clipped there, and a box that measures well but is cut away is not on screen. Measured on what is left rather than required to fit exactly, because a text chip at the very end of a zone can overhang it by the pixel its fractional width rounds to.
        let on_bar = |start: f32, length: f32, bar: f32| (start + length).min(bar) - start.max(0.0);
        let wide = on_bar(rect.x - strip.x, rect.width, strip.width);
        let tall = on_bar(rect.y - strip.y, rect.height, strip.height);
        if wide < icon || tall < icon {
            wrong.push(format!(
                    "{edge:?}/{mode:?}: {wide}x{tall}px of the placeholder at {rect:?} is on the {}x{} bar — less than \
                     the {icon}px glyph it holds",
                    strip.width, strip.height
                ));
        }
        let named = commands
            .iter()
            .any(|command| matches!(command, DrawCommand::Text { text, .. } if &**text == UNKNOWN));
        if named != edge.is_horizontal() {
            wrong.push(format!(
                    "{edge:?}/{mode:?}: the id is {} — it belongs along a horizontal bar and only there",
                    if named {
                        "written down a vertical bar"
                    } else {
                        "missing from a horizontal bar"
                    }
                ));
        }
    });
    assert!(
        wrong.is_empty(),
        "an unknown module did not hold a chip's place:\n  {}",
        wrong.join("\n  ")
    );
}

fn open_windows() -> Vec<platform_wayland::ManagedToplevel> {
    [
        (1, "kitty", "nvim — resolve.rs", true, false),
        (2, "firefox", "Firefox", false, false),
        (3, "org.gnome.Nautilus", "Files", false, true),
    ]
    .into_iter()
    .map(
        |(id, app_id, title, activated, minimized)| platform_wayland::ManagedToplevel {
            id: platform_wayland::ManagedToplevelId::from_raw(id),
            title: title.to_string(),
            app_id: app_id.to_string(),
            activated,
            minimized,
            ..platform_wayland::ManagedToplevel::default()
        },
    )
    .collect()
}

#[test]
fn the_windows_strip_lays_each_window_along_its_bar_on_every_edge_and_shape() {
    let mut wrong = Vec::new();
    sweep_with("windows", |entry, edge, mode, measured| {
        if entry.component_name != "bar" {
            return;
        }
        let commands = match measured {
            Ok(commands) => commands,
            Err(e) => {
                wrong.push(format!(
                    "{edge:?}/{mode:?}: the bar failed to lay out — {e}"
                ));
                return;
            }
        };
        let strip = surfaces::preview::bar_strip().expect("the previewed layout has a bar");
        let (rest, active) = modules::windows::fills(
            config::config()
                .expect("the sweep published a config")
                .resolve_theme(),
        );
        let entries: Vec<(Rect, bool)> = commands
            .iter()
            .filter_map(|command| match command {
                DrawCommand::Rect { rect, style } if style.fill == Some(Paint::Solid(rest)) => {
                    Some((*rect, false))
                }
                DrawCommand::Rect { rect, style } if style.fill == Some(Paint::Solid(active)) => {
                    Some((*rect, true))
                }
                _ => None,
            })
            .collect();
        if entries.len() != open_windows().len() {
            wrong.push(format!(
                "{edge:?}/{mode:?}: {} entries for {} windows",
                entries.len(),
                open_windows().len()
            ));
            return;
        }
        if entries.iter().filter(|(_, focused)| *focused).count() != 1 {
            wrong.push(format!(
                "{edge:?}/{mode:?}: the focused window is not the one entry on the accent"
            ));
        }
        let along = |rect: &Rect| match edge.is_vertical() {
            true => (rect.y, rect.height, rect.x, rect.width),
            false => (rect.x, rect.width, rect.y, rect.height),
        };
        for pair in entries.windows(2) {
            let (start, length, across, _) = along(&pair[0].0);
            let (next, _, next_across, _) = along(&pair[1].0);
            if next < start + length - SLACK || (next_across - across).abs() > SLACK {
                wrong.push(format!(
                    "{edge:?}/{mode:?}: {:?} and {:?} are not one after the other along the bar",
                    pair[0].0, pair[1].0
                ));
            }
        }
        for (rect, _) in &entries {
            let inside = rect.x >= strip.x - SLACK
                && rect.y >= strip.y - SLACK
                && rect.x + rect.width <= strip.x + strip.width + SLACK
                && rect.y + rect.height <= strip.y + strip.height + SLACK;
            if !inside {
                wrong.push(format!(
                    "{edge:?}/{mode:?}: an entry at {rect:?} is off the {strip:?} bar"
                ));
            }
        }
        let titled = entries
            .iter()
            .filter(|(rect, _)| {
                under_transform(&commands).iter().any(|(command, at)| {
                    let DrawCommand::Text {
                        rect: text,
                        text: written,
                        ..
                    } = command
                    else {
                        return false;
                    };
                    let drawn = in_surface_space(*text, *at);
                    !written.is_empty()
                        && drawn.x >= rect.x - SLACK
                        && drawn.x <= rect.x + rect.width + SLACK
                        && drawn.y >= rect.y - SLACK
                        && drawn.y <= rect.y + rect.height + SLACK
                })
            })
            .count();
        let expected = if edge.is_vertical() { 0 } else { entries.len() };
        if titled != expected {
            wrong.push(format!(
                "{edge:?}/{mode:?}: {titled} of {} entries carry a title, where {expected} should",
                entries.len()
            ));
        }
    });
    assert!(
        wrong.is_empty(),
        "the windows strip did not hold its windows along the bar:\n  {}",
        wrong.join("\n  ")
    );
}

/// Sub-pixel slack, for the same reason [`COLLAPSED`] has some: layout lands a fraction under a whole pixel routinely, and the question asked is whether a rect is claimed, not whether the arithmetic is exact.
const SLACK: f32 = 0.5;

/// Every command paired with the transform it is drawn under, so a rect can be read in the surface's own coordinates.
///
/// **A command list mixes two spaces, and nothing on a command says which it is in.** A `StyledContainer` paints at its laid-out rect, which is already the surface's. Every leaf is emitted at a zero origin inside a `PushMatrix` carrying its position (`at_layout_position`), and so is whatever a `Canvas` closure draws — `workspaces` emits its active-workspace pill relative to the row it sits in. Reading the second kind as if it were the first is how a check on drawn geometry passes on a rect that is nowhere near where it claims to be, which is what this file did until the fold existed.
///
/// Composed the way the renderer composes it (`renderer-core`'s `DrawState::push_matrix`): a local point goes through the innermost matrix first and out through the chain above it. Handing back the pair rather than a rect keeps it general — a caller asking where a command *is* has everything it needs, including the one this file still cannot ask.
fn under_transform(commands: &[DrawCommand]) -> Vec<(&DrawCommand, Transform)> {
    let mut stack = vec![Transform::IDENTITY];
    let mut placed = Vec::with_capacity(commands.len());
    for command in commands {
        let at = *stack.last().expect("the base transform is never popped");
        match command {
            DrawCommand::PushMatrix { matrix } => {
                stack.push(Transform::from_array(*matrix).then(at))
            }
            DrawCommand::PopMatrix => {
                if stack.len() > 1 {
                    stack.pop();
                }
            }
            _ => placed.push((command, at)),
        }
    }
    placed
}

/// `rect` in the surface's coordinates, as the box containing it once drawn under `at`.
///
/// Measured from the corners, because a transform that turns a rect leaves no rect behind. Nothing in this tree rotates, and a bound that contains the ink is the safe way to be wrong if anything ever does: it over-states what has to be claimed, so the check fails loudly rather than passing quietly.
fn in_surface_space(rect: Rect, at: Transform) -> Rect {
    let (right, bottom) = (rect.x + rect.width, rect.y + rect.height);
    let corners = [
        at.apply(Point::new(rect.x, rect.y)),
        at.apply(Point::new(right, rect.y)),
        at.apply(Point::new(rect.x, bottom)),
        at.apply(Point::new(right, bottom)),
    ];
    let span = |of: fn(&Point) -> f32| {
        let low = corners.iter().map(of).fold(f32::INFINITY, f32::min);
        let high = corners.iter().map(of).fold(f32::NEG_INFINITY, f32::max);
        (low, high - low)
    };
    let ((x, width), (y, height)) = (span(|p| p.x), span(|p| p.y));
    Rect::new(x, y, width, height)
}

/// The rect a command puts visible ink in, in the surface's own coordinates, if it puts any.
///
/// Narrower than [`painted_rect`] by what is not ink: a viewport, and a box drawn in a colour with no alpha — a filler chip holds a place on the bar and paints nothing, and the air it holds belongs to whatever is under the window. Wider by what [`under_transform`] made comparable: a label and a glyph are leaf-local, so until their transform was folded in there was no honest way to measure them against anything.
fn inked_rect(command: &DrawCommand, at: Transform) -> Option<Rect> {
    let local = match command {
        DrawCommand::Rect { rect, style } => {
            let filled = match style.fill {
                Some(Paint::Solid(color)) => color.a > 0.0,
                Some(Paint::Gradient(_)) => true,
                None => false,
            };
            (filled || style.border.is_some()).then_some(*rect)
        }
        DrawCommand::Image { rect, .. } => Some(*rect),
        DrawCommand::Text { rect, text, .. } => (!text.is_empty()).then_some(*rect),
        DrawCommand::Path { data, .. } => data.bounds(),
        _ => None,
    }?;
    Some(in_surface_space(local, at))
}

/// What of `rect` no claim in `region` covers — the way the compositor means it, so a rect spanning two adjoining claims is covered by neither alone and by both together.
fn uncovered(rect: Rect, region: &[Rect]) -> Vec<Rect> {
    let mut left = vec![rect];
    for claim in region {
        left = left
            .into_iter()
            .flat_map(|piece| subtract(piece, *claim))
            .collect();
        if left.is_empty() {
            break;
        }
    }
    left.retain(|piece| piece.width >= SLACK && piece.height >= SLACK);
    left
}

/// `from` with `hole` cut out of it, as the up-to-four rectangles that are left.
fn subtract(from: Rect, hole: Rect) -> Vec<Rect> {
    let Some(cut) = from.intersect(hole) else {
        return vec![from];
    };
    let (right, bottom) = (from.x + from.width, from.y + from.height);
    let (cut_right, cut_bottom) = (cut.x + cut.width, cut.y + cut.height);
    [
        Rect::new(from.x, from.y, from.width, cut.y - from.y),
        Rect::new(from.x, cut_bottom, from.width, bottom - cut_bottom),
        Rect::new(from.x, cut.y, cut.x - from.x, cut.height),
        Rect::new(cut_right, cut.y, right - cut_right, cut.height),
    ]
    .into_iter()
    .filter(|piece| piece.width > 0.0 && piece.height > 0.0)
    .collect()
}

/// **Every pixel a bar paints is a pixel the window claims.**
///
/// A layer window anchored to the whole output hands the compositor the union of the rects its chrome declared opaque, and everything outside that union belongs to the application underneath. A painted rect missing from it is therefore a hole in the bar: a press on the background between two chips, or on a section panel, goes through the shell and lands in whatever is behind it. That is invisible on screen and only shows when somebody clicks.
///
/// The converse is deliberately not asserted. A region is made of rectangles, so a chip with rounded corners claims the corners too, and the bar under a frame claims a strip the ring next to it painted.
///
/// Only the part of a rect that is *on* the bar is asked about, the same allowance [`an_unknown_module_holds_a_chips_place_on_every_edge_and_shape`] makes: a chip at the very end of a zone overhangs it by the pixel its fractional width rounds to, and a sliver past the edge of the surface is a sliver nobody can click.
#[test]
fn every_painted_rect_of_a_bar_is_claimed_for_the_input_region() {
    let mut holes = Vec::new();
    sweep_claimed(|entry, edge, mode, measured| {
        if entry.component_name != "bar" {
            return;
        }
        let Ok((commands, region)) = measured else {
            return;
        };
        let bar = surfaces::preview::bar_strip().expect("the previewed layout has a bar");
        let on_bar = under_transform(&commands)
            .into_iter()
            .filter_map(|(command, at)| inked_rect(command, at))
            .filter_map(|rect| rect.intersect(bar));
        for rect in on_bar {
            holes.extend(uncovered(rect, &region).into_iter().map(|hole| {
                format!(
                    "{edge:?}/{mode:?} — {}x{} at {},{}",
                    hole.width, hole.height, hole.x, hole.y
                )
            }));
        }
    });
    assert!(
        holes.is_empty(),
        "{} painted piece(s) of a bar are outside the region the window claims, so a press on them reaches \
         the application under the shell:\n  {}",
        holes.len(),
        holes.join("\n  ")
    );
}

fn kind(command: &DrawCommand) -> &'static str {
    match command {
        DrawCommand::Rect { .. } => "a rect",
        DrawCommand::Text { .. } => "text",
        DrawCommand::Image { .. } => "an image",
        DrawCommand::PushClip { .. } => "a viewport",
        _ => "a draw",
    }
}

/// The screens every area kind is laid out on: a common laptop, a large landscape monitor and a portrait one, so an area that only fits one aspect ratio shows.
const MONITORS: [(f32, f32); 3] = [(1920.0, 1080.0), (2560.0, 1440.0), (1080.0, 1920.0)];

fn instance(module: &str, representation: layout::Representation) -> layout::ResolvedInstance {
    layout::ResolvedInstance {
        id: layout::InstanceId::new(module),
        module: module.to_string(),
        representation,
        options: toml::Table::new(),
        bindings: std::collections::BTreeMap::new(),
        style: layout::Style::default(),
        placement: None,
        actions: std::collections::BTreeMap::new(),
    }
}

fn group(
    kind: layout::GroupKind,
    children: Vec<layout::ResolvedInstance>,
) -> layout::ResolvedGroup {
    layout::ResolvedGroup {
        id: layout::GroupId::new("swept"),
        kind,
        arrange: None,
        cols: layout::Arrange::TRACKS,
        rows: layout::Arrange::TRACKS,
        gap: None,
        repeat: None,
        komponent: None,
        style: layout::Style::default(),
        children,
    }
}

fn paged(
    kind: layout::GroupKind,
    children: Vec<layout::ResolvedInstance>,
) -> layout::ResolvedGroup {
    layout::ResolvedGroup {
        arrange: Some(layout::Arrange::Pages),
        ..group(kind, children)
    }
}

/// One group of each arrangement that shares its box out, two children each placed the way it takes, on a 2 × 2 of 4 × 4 cells.
fn containers() -> Vec<layout::ResolvedGroup> {
    use layout::{Arrange, ChildCell, GroupKind, Placement, Representation as Placed};
    let placed = |arrange: Arrange, index: u32| match arrange {
        Arrange::Grid => Placement::Cell(ChildCell::at(index, 0)),
        Arrange::Free => Placement::Rect(layout::Rect {
            x: 0.25 * index as f32,
            y: 0.25 * index as f32,
            w: 0.5,
            h: 0.5,
        }),
        _ => Placement::Weight(1.0 + index as f32),
    };
    [Arrange::Row, Arrange::Column, Arrange::Grid, Arrange::Free]
        .into_iter()
        .zip(0u32..)
        .map(|(arrange, at)| {
            let children = [("clock", Placed::WidgetM), ("visualiser", Placed::WidgetL)]
                .into_iter()
                .zip(0u32..)
                .map(
                    |((module, representation), index)| layout::ResolvedInstance {
                        placement: Some(placed(arrange, index)),
                        ..instance(module, representation)
                    },
                )
                .collect();
            let cell = GroupKind::Cell {
                col: at % 2 * 4,
                row: at / 2 * 4,
                col_span: 4,
                row_span: 4,
            };
            layout::ResolvedGroup {
                id: layout::GroupId::new(format!("container-{}", arrange.as_str())),
                arrange: Some(arrange),
                ..group(cell, children)
            }
        })
        .collect()
}

/// A container that shows one child at a time, on the first 4 × 4 cells of its grid, holding two widgets of different sizes.
fn pages_container() -> layout::ResolvedGroup {
    use layout::{Arrange, GroupKind, Representation as Placed};
    layout::ResolvedGroup {
        id: layout::GroupId::new("container-pages"),
        arrange: Some(Arrange::Pages),
        ..group(
            GroupKind::Cell {
                col: 0,
                row: 0,
                col_span: 4,
                row_span: 4,
            },
            vec![
                instance("clock", Placed::WidgetM),
                instance("visualiser", Placed::WidgetL),
            ],
        )
    }
}

fn area_of(
    id: String,
    kind: layout::ResolvedAreaKind,
    groups: Vec<layout::ResolvedGroup>,
) -> layout::ResolvedArea {
    let reserve = matches!(kind, layout::ResolvedAreaKind::Bar { .. });
    layout::ResolvedArea {
        id: layout::AreaId::new(id),
        kind,
        reserve,
        above_fullscreen: false,
        within: layout::Within::Output,
        style: layout::Style::default(),
        visible: None,
        actions: Default::default(),
        groups,
    }
}

fn every_area(mode: Shape) -> Vec<layout::ResolvedArea> {
    use layout::{Anchor, GroupKind, Representation as Placed, ResolvedAreaKind, Zone};
    let chips = || {
        vec![
            group(
                GroupKind::Zone { zone: Zone::Start },
                vec![instance("workspaces", Placed::Chip)],
            ),
            group(
                GroupKind::Zone { zone: Zone::Center },
                vec![instance("clock", Placed::Chip)],
            ),
            paged(
                GroupKind::Zone { zone: Zone::End },
                vec![
                    instance("notes", Placed::Chip),
                    instance("clock", Placed::Chip),
                ],
            ),
        ]
    };
    let widget = || {
        vec![group(
            GroupKind::Cell {
                col: 0,
                row: 0,
                col_span: 1,
                row_span: 1,
            },
            vec![instance("clock", Placed::WidgetM)],
        )]
    };
    let mut areas = Vec::new();
    for edge in Edge::ALL {
        areas.push(area_of(
            format!("bar-{edge:?}"),
            ResolvedAreaKind::Bar {
                edge,
                thickness: 34.0,
                length: layout::Extent::Fill,
                offset: 0.0,
                shape: layout::BarShape {
                    mode: Some(mode),
                    ..layout::BarShape::default()
                },
                autohide: None,
            },
            chips(),
        ));
        areas.push(area_of(
            format!("dock-{edge:?}"),
            ResolvedAreaKind::Dock {
                edge,
                thickness: 60.0,
            },
            chips(),
        ));
    }
    for anchor in Anchor::ALL {
        areas.push(area_of(
            format!("grid-{anchor:?}"),
            ResolvedAreaKind::Grid {
                rect: layout::Rect::default(),
                cell: 80.0,
                gap: 16.0,
                anchor,
            },
            widget(),
        ));
        let outward = |side: surfaces::pinned::Side| match side {
            surfaces::pinned::Side::Start => -4000.0,
            surfaces::pinned::Side::Middle | surfaces::pinned::Side::End => 4000.0,
        };
        let (across, down) = surfaces::pinned::sides(anchor);
        for flow in [layout::StackFlow::Column, layout::StackFlow::Row] {
            let stack = |offset| ResolvedAreaKind::Stack {
                anchor,
                offset,
                width: 380.0,
                flow,
                output_policy: layout::StackOutputPolicy::Here,
                routes: Vec::new(),
                launcher: false,
            };
            areas.push(area_of(
                format!("stack-{flow:?}-{anchor:?}"),
                stack(layout::Offset::ZERO),
                Vec::new(),
            ));
            areas.push(area_of(
                format!("stack-{flow:?}-{anchor:?}-pushed-off"),
                stack(layout::Offset {
                    x: outward(across),
                    y: outward(down),
                }),
                Vec::new(),
            ));
        }
    }
    areas.push(area_of(
        "containers".into(),
        ResolvedAreaKind::Grid {
            rect: layout::Rect::default(),
            cell: 80.0,
            gap: 16.0,
            anchor: Anchor::TopLeft,
        },
        containers(),
    ));
    let mut styled_containers = containers();
    for (at, container) in styled_containers.iter_mut().enumerate() {
        container.style = styled("accent", 1 + at as u8 % 3);
        for child in &mut container.children {
            child.style = styled("red", 3);
        }
    }
    areas.push(area_of(
        "styled-containers".into(),
        ResolvedAreaKind::Grid {
            rect: layout::Rect::default(),
            cell: 80.0,
            gap: 16.0,
            anchor: Anchor::TopLeft,
        },
        styled_containers,
    ));
    let mut styled_pages = pages_container();
    styled_pages.style = styled("accent", 2);
    for child in &mut styled_pages.children {
        child.style = styled("red", 3);
    }
    for (id, container) in [
        ("paged-container", pages_container()),
        ("styled-paged-container", styled_pages),
    ] {
        areas.push(area_of(
            id.into(),
            ResolvedAreaKind::Grid {
                rect: layout::Rect::default(),
                cell: 80.0,
                gap: 16.0,
                anchor: Anchor::TopLeft,
            },
            vec![container],
        ));
    }
    for edge in Edge::ALL {
        let mut groups = chips();
        for group in &mut groups {
            group.style = styled("accent", 2);
            for child in &mut group.children {
                child.style = styled("green", 1);
            }
        }
        areas.push(area_of(
            format!("styled-dock-{edge:?}"),
            ResolvedAreaKind::Dock {
                edge,
                thickness: 60.0,
            },
            groups,
        ));
    }
    areas.push(area_of(
        "free".into(),
        ResolvedAreaKind::Free {
            rect: layout::Rect {
                x: 0.7,
                y: 0.7,
                w: 0.25,
                h: 0.25,
            },
            anchor: layout::Anchor::TopLeft,
        },
        vec![paged(
            GroupKind::Zone { zone: Zone::Start },
            vec![instance("clock", Placed::Card)],
        )],
    ));
    areas.push(area_of(
        "wallpaper".into(),
        ResolvedAreaKind::WallpaperRegion {
            rect: layout::Rect::default(),
            source: String::new(),
            fit: layout::Fit::Cover,
            transition: layout::Transition::Fade,
        },
        Vec::new(),
    ));
    areas.push(area_of(
        "texture".into(),
        ResolvedAreaKind::Texture {
            rect: layout::Rect {
                x: 0.0,
                y: 0.0,
                w: 1.0,
                h: 0.5,
            },
            paint: layout::Paint::Gradient(layout::Gradient {
                angle: 90.0,
                stops: vec![
                    layout::GradientStop {
                        at: 0.0,
                        color: "surface".into(),
                    },
                    layout::GradientStop {
                        at: 1.0,
                        color: "accent".into(),
                    },
                ],
            }),
            tile: layout::Tile::None,
            blend: layout::Blend::Normal,
            opacity: 1.0,
        },
        Vec::new(),
    ));
    areas
}

/// What of each inked rect in `commands` is left once every clip around it has cut it: the ink that actually reaches the screen. A label past the end of a bar and a wallpaper larger than its region are cut by the clip they are drawn in, and are not ink anybody sees.
fn visible_ink(commands: &[DrawCommand]) -> Vec<Rect> {
    inked(commands).into_iter().map(|(_, rect)| rect).collect()
}

/// [`visible_ink`], each rect with the [`kind`] of command that put it there.
fn inked(commands: &[DrawCommand]) -> Vec<(&'static str, Rect)> {
    let mut clips: Vec<Option<Rect>> = Vec::new();
    let mut ink = Vec::new();
    for (command, at) in under_transform(commands) {
        match command {
            DrawCommand::PushClip { rect, .. } => {
                let clip = in_surface_space(*rect, at);
                let within = match clips.last() {
                    Some(Some(outer)) => clip.intersect(*outer),
                    Some(None) => None,
                    None => Some(clip),
                };
                clips.push(within);
            }
            DrawCommand::PopClip => {
                clips.pop();
            }
            _ => {
                let Some(rect) = inked_rect(command, at) else {
                    continue;
                };
                let seen = match clips.last() {
                    Some(Some(clip)) => rect.intersect(*clip),
                    Some(None) => None,
                    None => Some(rect),
                };
                ink.extend(
                    seen.filter(|rect| rect.width >= SLACK && rect.height >= SLACK)
                        .map(|rect| (kind(command), rect)),
                );
            }
        }
    }
    ink
}

fn off_screen(commands: &[DrawCommand], size: (f32, f32)) -> Vec<Rect> {
    let screen = Rect::new(-SLACK, -SLACK, size.0 + 2.0 * SLACK, size.1 + 2.0 * SLACK);
    visible_ink(commands)
        .into_iter()
        .filter(|rect| rect.intersect(screen) != Some(*rect))
        .collect()
}

fn measure_area(
    area: &layout::ResolvedArea,
    size: (f32, f32),
) -> Result<Vec<DrawCommand>, LayoutError> {
    measure_area_beside(area, &[], size)
}

fn measure_area_beside(
    area: &layout::ResolvedArea,
    beside: &[layout::ResolvedArea],
    size: (f32, f32),
) -> Result<Vec<DrawCommand>, LayoutError> {
    let built = built_area_beside(area, beside, size)?;
    let page = || LayoutStyle::new().width(size.0).height(size.1);
    let root_node = new_container(page(), &[built.layout_node()])?;
    let tree = ComponentList::new(Container::new(page(), vec![built])?);
    compute_layout(
        root_node,
        AvailableSpace::Definite(size.0),
        AvailableSpace::Definite(size.1),
    )?;
    Ok(tree.commands().to_vec())
}

/// `area` built on a desktop of `size`, as the shell would build it there.
fn built_area(
    area: &layout::ResolvedArea,
    size: (f32, f32),
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    built_area_beside(area, &[], size)
}

fn built_area_beside(
    area: &layout::ResolvedArea,
    beside: &[layout::ResolvedArea],
    size: (f32, f32),
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let config = config::config().expect("the sweep published a config");
    let resolved = layout::Resolved::of(
        "SWEPT-1",
        [(
            layout::LayerKind::Desktop,
            layout::ResolvedLayer {
                areas: std::iter::once(area.clone())
                    .chain(beside.iter().cloned())
                    .collect(),
            },
        )],
    );
    surfaces::reconcile::publish(&[crate::test_support::measured(
        "SWEPT-1",
        Arc::clone(&config),
        resolved.clone(),
        size,
    )]);
    let surround = surfaces::area::Surround {
        config: &config,
        theme: config.resolve_theme(),
        output: Some("SWEPT-1"),
        layer: layout::LayerKind::Desktop,
        bounds: Rect::new(0.0, 0.0, size.0, size.1),
        reserved: surfaces::layer_window::Reserved::of(&resolved, &config),
        audience: ui::host::Audience::Owner,
    };
    telar::batch(|| surfaces::area::build(area, surround))
        .unwrap_or_else(|| Err(LayoutError::Engine("nothing builds this area".into())))
}

/// **Every area kind works everywhere it can be put** — the standing rule, taken literally: a bar and a dock on all four edges, a grid, a stack and a free area at all nine anchors, a wallpaper and a texture, on a laptop, a large monitor and a portrait one, in `bar`, `sections` and `chips`. Each has to lay out, draw something, and draw all of it on the screen it was placed on.
///
/// The stack is shown an OSD so it has a card to place: an empty column draws nothing, and nothing drawn is nothing measured.
#[test]
fn every_area_kind_lays_out_on_screen_on_every_edge_anchor_and_monitor() {
    let _world = WORLD
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut faults = Vec::new();
    for mode in MODES {
        for size in MONITORS {
            for area in every_area(mode) {
                reset_layout_runtime();
                seed_world(Edge::Top, mode, None);
                surfaces::area::set_stack_builder(modules::stack::area);
                modules::stack::show_osd(modules::osd::OsdKind::Brightness);
                let scope = telar::owner_scope();
                let owner = scope.id();
                let measured = measure_area(&area, size);
                drop(scope);
                let at = format!("{} on {}x{} in {mode:?}", area.id, size.0, size.1);
                match measured {
                    Err(error) => faults.push(format!("{at}: {error}")),
                    Ok(commands) => {
                        if !commands.iter().any(paints) {
                            faults.push(format!("{at}: drew nothing"));
                        }
                        for rect in off_screen(&commands, size) {
                            faults.push(format!(
                                "{at}: {}x{} at {},{} is off the screen",
                                rect.width, rect.height, rect.x, rect.y
                            ));
                        }
                    }
                }
                telar::dispose_owner(owner);
            }
        }
    }
    assert!(
        faults.is_empty(),
        "{} area placement(s) broke the standing rule:\n  {}",
        faults.len(),
        faults.join("\n  ")
    );
}

/// Three clocks sharing a row container `col_span` × `row_span` cells big, at its top left corner: as it shrinks they draw at a smaller widget, then as chips.
fn shrinking_row(col_span: u32, row_span: u32) -> layout::ResolvedGroup {
    use layout::{Arrange, GroupKind, Placement, Representation as Placed};
    let children = (0..3)
        .map(|at| layout::ResolvedInstance {
            id: layout::InstanceId::new(format!("clock-{at}")),
            placement: Some(Placement::Weight(1.0)),
            ..instance("clock", Placed::WidgetM)
        })
        .collect();
    layout::ResolvedGroup {
        id: layout::GroupId::new(format!("row-{col_span}x{row_span}")),
        arrange: Some(Arrange::Row),
        ..group(
            GroupKind::Cell {
                col: 0,
                row: 0,
                col_span,
                row_span,
            },
            children,
        )
    }
}

/// **A container's children never draw outside its box**: every arrangement, and a row shrunk from widgets down to chips, each alone on a grid of 80 px cells 16 px apart, on every monitor. The box is the cells it was written with, which is all a grid at its top left corner with no padding puts it on.
#[test]
fn no_container_draws_past_its_box() {
    let _world = WORLD
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut cases = containers();
    cases.push(pages_container());
    cases.extend(
        [(12, 4), (6, 2), (6, 1), (3, 1), (1, 1)].map(|(cols, rows)| shrinking_row(cols, rows)),
    );
    let mut faults = Vec::new();
    for size in MONITORS {
        for container in &cases {
            reset_layout_runtime();
            seed_world(Edge::Top, Shape::Bar, None);
            let scope = telar::owner_scope();
            let owner = scope.id();
            let area = area_of(
                container.id.to_string(),
                layout::ResolvedAreaKind::Grid {
                    rect: layout::Rect::default(),
                    cell: 80.0,
                    gap: 16.0,
                    anchor: layout::Anchor::TopLeft,
                },
                vec![container.clone()],
            );
            let measured = measure_area(&area, size);
            drop(scope);
            let at = format!("{} on {}x{}", container.id, size.0, size.1);
            let layout::GroupKind::Cell { col, row, .. } = container.kind else {
                unreachable!("every case is on cells");
            };
            let span = surfaces::area::cells_of(container).extent(80.0, 16.0);
            let held = Rect::new(
                col as f32 * 96.0 - SLACK,
                row as f32 * 96.0 - SLACK,
                span.width + 2.0 * SLACK,
                span.height + 2.0 * SLACK,
            );
            match measured {
                Err(error) => faults.push(format!("{at}: {error}")),
                Ok(commands) => {
                    let ink = visible_ink(&commands);
                    if ink.is_empty() {
                        faults.push(format!("{at}: drew nothing"));
                    }
                    for rect in ink
                        .into_iter()
                        .filter(|rect| rect.intersect(held) != Some(*rect))
                    {
                        faults.push(format!(
                            "{at}: {}x{} at {},{} is past its {}x{} box",
                            rect.width, rect.height, rect.x, rect.y, span.width, span.height
                        ));
                    }
                }
            }
            telar::dispose_owner(owner);
        }
    }
    assert!(
        faults.is_empty(),
        "{} container(s) drew past their box:\n  {}",
        faults.len(),
        faults.join("\n  ")
    );
}

/// The side of the container [`resized_child`] sits in: six 80 px cells 16 px apart, which holds a large widget with room to grow it.
const RESIZED_BOX: f32 = 6.0 * 80.0 + 5.0 * 16.0;

/// `module` alone in an unpadded free container six cells square at the top left corner of a grid, at `share` of its box.
fn resized_child(module: &str, share: (f32, f32)) -> layout::ResolvedArea {
    let child = layout::ResolvedInstance {
        placement: Some(layout::Placement::Rect(layout::Rect {
            x: 0.0,
            y: 0.0,
            w: share.0,
            h: share.1,
        })),
        ..instance(module, layout::Representation::WidgetS)
    };
    let container = layout::ResolvedGroup {
        id: layout::GroupId::new("resized"),
        arrange: Some(layout::Arrange::Free),
        style: layout::Style {
            padding: Some(layout::Sides::all(0.0)),
            ..layout::Style::default()
        },
        ..group(
            layout::GroupKind::Cell {
                col: 0,
                row: 0,
                col_span: 6,
                row_span: 6,
            },
            vec![child],
        )
    };
    area_of(
        "resized".into(),
        layout::ResolvedAreaKind::Grid {
            rect: layout::Rect::default(),
            cell: 80.0,
            gap: 16.0,
            anchor: layout::Anchor::TopLeft,
        },
        vec![container],
    )
}

/// `was` laid out in a desktop window of `size`, then edited in place to `now` the way the window keeps a container's children through a change to their shares, and drawn again.
fn resized_in_place(
    was: &layout::ResolvedArea,
    now: &layout::ResolvedArea,
    size: (f32, f32),
) -> Result<Vec<DrawCommand>, LayoutError> {
    telar::set_context(surfaces::layer_window::LayerWindowContext {
        layer: layout::LayerKind::Desktop,
        output: Some("SWEPT-1".into()),
        demands: std::rc::Rc::new(surfaces::layer_window::Demands::new(
            platform_wayland::Layer::Bottom,
        )),
        mapped: telar::signal(true).read_only(),
    });
    let root = Container::new(
        LayoutStyle::new().width(size.0).height(size.1),
        vec![built_area(was, size)?],
    )?;
    let node = root.layout_node();
    let mut tree = ComponentList::new(root);
    tree.on_event(&telar::Event::WindowResized {
        width: size.0 as u32,
        height: size.1 as u32,
    });
    compute_layout(
        node,
        AvailableSpace::Definite(size.0),
        AvailableSpace::Definite(size.1),
    )?;
    let kept = surfaces::area::moves_only(was, now)
        && surfaces::area::move_cells(
            Some("SWEPT-1"),
            layout::LayerKind::Desktop,
            layout::LayerKind::Desktop,
            now,
        );
    if !kept {
        return Err(LayoutError::Engine("the resize was not kept".into()));
    }
    telar::relayout_if_dirty();
    Ok(tree.commands().to_vec())
}

/// What a module drew, by size alone: where it sits moves with whatever is beside it, and a label's width with the reading it shows.
fn drawn_sizes(commands: &[DrawCommand]) -> Vec<(&'static str, f32, f32)> {
    let mut sizes: Vec<(&'static str, f32, f32)> = inked(commands)
        .into_iter()
        .map(|(kind, rect)| match kind {
            "text" => (kind, 0.0, rect.height),
            _ => (kind, rect.width, rect.height),
        })
        .collect();
    sizes.sort_by(|a, b| {
        (a.0, a.1, a.2)
            .partial_cmp(&(b.0, b.1, b.2))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    sizes
}

fn same_sizes(a: &[(&'static str, f32, f32)], b: &[(&'static str, f32, f32)]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(a, b)| {
            a.0 == b.0 && (a.1 - b.1).abs() <= SLACK * 2.0 && (a.2 - b.2).abs() <= SLACK * 2.0
        })
}

/// The focused window and the workspaces the compositor reports, stood in for its own so that two builds compared with each other read the same, whatever another test seeded last.
fn seed_compositor() {
    use services::hyprland::{ActiveWindow, Snapshot, Workspace};
    services::hyprland::seed_active_window(ActiveWindow {
        title: "Resized window".to_string(),
        class: "hogar-shell-preview".to_string(),
        address: "0x1".to_string(),
        handle: None,
    });
    services::hyprland::seed_workspaces(Snapshot {
        workspaces: (1..=3)
            .map(|id| Workspace {
                id,
                name: id.to_string(),
                windows: id as u32 - 1,
                monitor: "SWEPT-1".to_string(),
                clients: Vec::new(),
                handle: None,
            })
            .collect(),
        active: 2,
        focused_monitor: "SWEPT-1".to_string(),
    });
}

/// **A container child resized in place draws what one built at its new size draws**: every module, alone in a container, as a chip, a small, a medium and a large widget, its share grown or shrunk without leaving the representation it fits. The resize keeps the child rather than building it again, so a module that sized what it draws once, at the box it was first told, draws at the old size inside the new one; laid out again, the same drawing has to come out as a fresh build at the new share, with nothing past the container's box.
#[test]
fn a_container_child_resized_in_place_draws_what_a_fresh_one_does() {
    let _world = WORLD
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let resizes: [((f32, f32), (f32, f32)); 4] = [
        ((0.3, 0.06), (0.2, 0.04)),
        ((0.32, 0.32), (0.45, 0.45)),
        ((0.7, 0.32), (0.9, 0.4)),
        ((0.95, 0.95), (0.7, 0.7)),
    ];
    let size = MONITORS[0];
    let held = Rect::new(
        -SLACK,
        -SLACK,
        RESIZED_BOX + 2.0 * SLACK,
        RESIZED_BOX + 2.0 * SLACK,
    );
    let mut faults = Vec::new();
    let mut compared = 0;
    for module in crate::core::modules::MODULES {
        for (from, to) in resizes {
            let fits = |share: (f32, f32)| {
                surfaces::container::fitted(
                    module.id,
                    Size {
                        width: share.0 * RESIZED_BOX,
                        height: share.1 * RESIZED_BOX,
                    },
                    (80.0, 16.0),
                    ui::host::Audience::Owner,
                )
            };
            if fits(from) != fits(to) {
                continue;
            }
            let at = format!("{} as {:?}, {from:?} to {to:?}", module.id, fits(to));
            let (was, now) = (resized_child(module.id, from), resized_child(module.id, to));
            let drawn = |build: &dyn Fn() -> Result<Vec<DrawCommand>, LayoutError>| {
                reset_layout_runtime();
                seed_world(Edge::Top, Shape::Bar, None);
                seed_compositor();
                let mut still = (*config::config().expect("seeded")).clone();
                still.animation.enabled = false;
                config::set_config(Arc::new(still));
                let scope = telar::owner_scope();
                let owner = scope.id();
                let drawn = build();
                drop(scope);
                telar::dispose_owner(owner);
                drawn
            };
            match (
                drawn(&|| resized_in_place(&was, &now, size)),
                drawn(&|| measure_area(&now, size)),
            ) {
                (Err(error), _) | (_, Err(error)) => faults.push(format!("{at}: {error}")),
                (Ok(resized), Ok(fresh)) => {
                    compared += 1;
                    for rect in visible_ink(&resized)
                        .into_iter()
                        .filter(|rect| rect.intersect(held) != Some(*rect))
                    {
                        faults.push(format!(
                            "{at}: {}x{} at {},{} is past its box",
                            rect.width, rect.height, rect.x, rect.y
                        ));
                    }
                    let (resized, fresh) = (drawn_sizes(&resized), drawn_sizes(&fresh));
                    if !same_sizes(&resized, &fresh) {
                        faults.push(format!(
                            "{at}: drew {resized:?} where a fresh build draws {fresh:?}"
                        ));
                    }
                }
            }
        }
    }
    assert!(compared > 0, "every resize left its representation");
    assert!(
        faults.is_empty(),
        "{} resized container child(ren) drew wrong:\n  {}",
        faults.len(),
        faults.join("\n  ")
    );
}

/// **An empty container says so only while its layer is edited**: every arrangement, emptied, on every monitor, draws no text outside the desktop's edit mode and its hint inside it, held inside its box.
#[test]
fn an_empty_container_shows_its_hint_inside_its_box_only_in_edit_mode() {
    let _world = WORLD
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let texts = |commands: &[DrawCommand]| -> Vec<Rect> {
        under_transform(commands)
            .into_iter()
            .filter(|(command, _)| matches!(command, DrawCommand::Text { .. }))
            .filter_map(|(command, at)| inked_rect(command, at))
            .collect()
    };
    let mut faults = Vec::new();
    for size in MONITORS {
        for mut container in containers() {
            container.children.clear();
            let area = area_of(
                container.id.to_string(),
                layout::ResolvedAreaKind::Grid {
                    rect: layout::Rect::default(),
                    cell: 80.0,
                    gap: 16.0,
                    anchor: layout::Anchor::TopLeft,
                },
                vec![container.clone()],
            );
            let layout::GroupKind::Cell { col, row, .. } = container.kind else {
                unreachable!("every case is on cells");
            };
            let span = surfaces::area::cells_of(&container).extent(80.0, 16.0);
            let held = Rect::new(
                col as f32 * 96.0 - SLACK,
                row as f32 * 96.0 - SLACK,
                span.width + 2.0 * SLACK,
                span.height + 2.0 * SLACK,
            );
            for edited in [false, true] {
                reset_layout_runtime();
                seed_world(Edge::Top, Shape::Bar, None);
                surfaces::expressions::set_edited(
                    edited.then(|| (Some("SWEPT-1".to_string()), layout::LayerKind::Desktop)),
                );
                let scope = telar::owner_scope();
                let owner = scope.id();
                let measured = measure_area(&area, size);
                drop(scope);
                let at = format!(
                    "{} on {}x{}{}",
                    container.id,
                    size.0,
                    size.1,
                    if edited { " in edit mode" } else { "" }
                );
                match measured {
                    Err(error) => faults.push(format!("{at}: {error}")),
                    Ok(commands) => {
                        let drawn = texts(&commands);
                        match (edited, drawn.is_empty()) {
                            (true, true) => faults.push(format!("{at}: no hint")),
                            (false, false) => faults.push(format!("{at}: a hint")),
                            _ => {}
                        }
                        for rect in drawn
                            .into_iter()
                            .filter(|rect| rect.intersect(held) != Some(*rect))
                        {
                            faults.push(format!(
                                "{at}: the hint at {},{} {}x{} is past its box",
                                rect.x, rect.y, rect.width, rect.height
                            ));
                        }
                    }
                }
                telar::dispose_owner(owner);
            }
        }
    }
    surfaces::expressions::set_edited(None);
    assert!(
        faults.is_empty(),
        "{} empty container(s) got their hint wrong:\n  {}",
        faults.len(),
        faults.join("\n  ")
    );
}

/// The check above has to be able to fail: ink past any edge of the screen is reported, and ink on it is not.
#[test]
fn ink_past_the_edge_of_the_screen_is_reported() {
    let on = DrawCommand::Rect {
        rect: Rect::new(10.0, 10.0, 100.0, 40.0),
        style: telar::RectStyle::filled(telar::Color::from_rgb_u8(255, 0, 0), 0.0).into(),
    };
    let past = DrawCommand::Rect {
        rect: Rect::new(1900.0, 10.0, 100.0, 40.0),
        style: telar::RectStyle::filled(telar::Color::from_rgb_u8(255, 0, 0), 0.0).into(),
    };
    assert!(off_screen(std::slice::from_ref(&on), (1920.0, 1080.0)).is_empty());
    assert_eq!(off_screen(&[on, past], (1920.0, 1080.0)).len(), 1);
}

fn bar_on(
    edge: Edge,
    autohide: Option<layout::AutoHide>,
    fillet: Option<f32>,
) -> layout::ResolvedArea {
    bar_in(None, edge, autohide, fillet)
}

fn bar_in(
    mode: Option<Shape>,
    edge: Edge,
    autohide: Option<layout::AutoHide>,
    fillet: Option<f32>,
) -> layout::ResolvedArea {
    area_of(
        format!("bar-{edge:?}"),
        layout::ResolvedAreaKind::Bar {
            edge,
            thickness: 34.0,
            length: layout::Extent::Fill,
            offset: 0.0,
            shape: layout::BarShape {
                mode,
                fillet,
                ..layout::BarShape::default()
            },
            autohide,
        },
        Vec::new(),
    )
}

/// A bar that hides itself stops at the reserving bar at its side instead of owning the corner and sliding in under it, on every edge pair, every monitor and in every mode; the bar that stays still owns the corner.
#[test]
fn a_hiding_bar_starts_after_a_reserving_bar_at_its_side_on_every_edge_and_monitor() {
    let _world = WORLD
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let hide = Some(layout::AutoHide {
        peek: 2.0,
        on_hover: true,
    });
    for mode in MODES {
        seed_world(Edge::Top, mode, None);
        let config = config::config().expect("the sweep published a config");
        for size in MONITORS {
            for edge in [Edge::Top, Edge::Bottom] {
                for side in [Edge::Left, Edge::Right] {
                    let across = bar_on(side, None, None);
                    let resolved = layout::Resolved::of(
                        "SWEPT-1",
                        [(
                            layout::LayerKind::Top,
                            layout::ResolvedLayer {
                                areas: vec![across.clone()],
                            },
                        )],
                    );
                    let reserved = surfaces::layer_window::Reserved::of(&resolved, &config);
                    let surround = surfaces::area::Surround {
                        config: &config,
                        theme: config.resolve_theme(),
                        output: Some("SWEPT-1"),
                        layer: layout::LayerKind::Top,
                        bounds: Rect::new(0.0, 0.0, size.0, size.1),
                        reserved,
                        audience: ui::host::Audience::Owner,
                    };
                    let hidden = surfaces::bar::strip_of_area(&bar_on(edge, hide, None), surround);
                    let owner = surfaces::bar::strip_of_area(&bar_on(edge, None, None), surround);
                    let at = format!(
                        "{edge:?} beside {side:?} on {}x{} in {mode:?}",
                        size.0, size.1
                    );
                    match side {
                        Edge::Left => {
                            assert_eq!(
                                hidden.x,
                                reserved.on(Edge::Left),
                                "{at}: starts after the strip"
                            );
                            assert!(
                                owner.x < reserved.on(Edge::Left),
                                "{at}: the owner keeps the corner"
                            );
                        }
                        _ => {
                            assert_eq!(
                                hidden.x + hidden.width,
                                size.0 - reserved.on(Edge::Right),
                                "{at}: ends before the strip"
                            );
                            assert!(
                                owner.x + owner.width > size.0 - reserved.on(Edge::Right),
                                "{at}: the owner keeps the corner"
                            );
                        }
                    }
                }
            }
        }
    }
}

/// Fillets build and draw on every edge and monitor, hiding or not; a bar that is shown keeps all of its ink on the screen, a hiding one is off it by design.
#[test]
fn a_fillet_draws_on_every_edge_and_monitor_without_leaving_the_screen() {
    let _world = WORLD
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let hide = Some(layout::AutoHide {
        peek: 2.0,
        on_hover: true,
    });
    for size in MONITORS {
        for edge in Edge::ALL {
            for autohide in [None, hide] {
                reset_layout_runtime();
                seed_world(Edge::Top, Shape::Bar, None);
                let scope = telar::owner_scope();
                let owner = scope.id();
                let area = bar_on(edge, autohide, Some(12.0));
                let measured = measure_area(&area, size);
                drop(scope);
                telar::dispose_owner(owner);
                let at = format!(
                    "{edge:?} on {}x{}, hiding: {}",
                    size.0,
                    size.1,
                    autohide.is_some()
                );
                let commands = measured.unwrap_or_else(|error| panic!("{at}: {error}"));
                assert!(commands.iter().any(paints), "{at}: drew nothing");
                assert!(
                    autohide.is_some() || off_screen(&commands, size).is_empty(),
                    "{at}: ink off the screen"
                );
            }
        }
    }
}

/// A bar that hides itself yields only to a vertical bar that stays on screen: one that hides as well leaves a peek strip, so the horizontal one runs the edge as if nothing were there. Every corner, monitor and mode.
#[test]
fn a_hiding_bar_ignores_a_hiding_bar_at_its_side_on_every_corner_and_monitor() {
    let _world = WORLD
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let hide = Some(layout::AutoHide {
        peek: 2.0,
        on_hover: true,
    });
    for mode in MODES {
        seed_world(Edge::Top, mode, None);
        let config = config::config().expect("the sweep published a config");
        for size in MONITORS {
            for edge in [Edge::Top, Edge::Bottom] {
                for side in [Edge::Left, Edge::Right] {
                    let strip_beside = |across: layout::ResolvedArea, autohide| {
                        let resolved = layout::Resolved::of(
                            "SWEPT-1",
                            [(
                                layout::LayerKind::Top,
                                layout::ResolvedLayer {
                                    areas: vec![across],
                                },
                            )],
                        );
                        let surround = surfaces::area::Surround {
                            config: &config,
                            theme: config.resolve_theme(),
                            output: Some("SWEPT-1"),
                            layer: layout::LayerKind::Top,
                            bounds: Rect::new(0.0, 0.0, size.0, size.1),
                            reserved: surfaces::layer_window::Reserved::of(&resolved, &config),
                            audience: ui::host::Audience::Owner,
                        };
                        surfaces::bar::strip_of_area(&bar_on(edge, autohide, None), surround)
                    };
                    let at = format!(
                        "{edge:?} beside a hiding {side:?} on {}x{} in {mode:?}",
                        size.0, size.1
                    );
                    let alone = strip_beside(bar_on(side, hide, None), None);
                    let yielding = strip_beside(bar_on(side, hide, None), hide);
                    assert_eq!(
                        yielding, alone,
                        "{at}: the peek strip is nothing to yield to"
                    );
                    let kept = strip_beside(bar_on(side, None, None), hide);
                    assert_ne!(kept, yielding, "{at}: a bar that stays is still yielded to");
                }
            }
        }
    }
}

/// A style that names everything a box can draw: a fill at an opacity, its corners, a line round it and a lift.
fn styled(line: &str, shadow: u8) -> layout::Style {
    layout::Style {
        fill: Some("surface".into()),
        opacity: Some(0.8),
        radius: Some(layout::Corners::all(8.0)),
        border: Some(layout::Border {
            width: Some(2.0),
            color: Some(line.into()),
        }),
        shadow: Some(shadow),
        ..layout::Style::default()
    }
}

/// A bar in `mode` on `edge` whose start zone is a styled group, a plate around its chips, whose centre chip is styled on its own beside a plate with nothing on it yet, and whose end is a styled stack.
fn styled_bar(edge: Edge, mode: Shape) -> layout::ResolvedArea {
    use layout::{GroupKind, Representation as Placed, Zone};
    let mut plated = group(
        GroupKind::Zone { zone: Zone::Start },
        vec![
            instance("workspaces", Placed::Chip),
            instance("clock", Placed::Chip),
        ],
    );
    plated.id = layout::GroupId::new("plated");
    plated.style = styled("accent", 2);
    let mut centre = group(
        GroupKind::Zone { zone: Zone::Center },
        vec![layout::ResolvedInstance {
            style: styled("red", 3),
            ..instance("clock", Placed::Chip)
        }],
    );
    centre.id = layout::GroupId::new("centre");
    let mut empty = group(GroupKind::Zone { zone: Zone::Center }, Vec::new());
    empty.id = layout::GroupId::new("empty");
    empty.style = layout::Style {
        fill: Some(ui::scale::plate::FILL.into()),
        ..layout::Style::default()
    };
    let mut end = paged(
        GroupKind::Zone { zone: Zone::End },
        vec![
            layout::ResolvedInstance {
                style: styled("green", 1),
                ..instance("notes", Placed::Chip)
            },
            instance("clock", Placed::Chip),
        ],
    );
    end.id = layout::GroupId::new("end");
    end.style = layout::Style {
        border: Some(layout::Border::default()),
        ..layout::Style::default()
    };
    let mut bar = area_of(
        format!("styled-bar-{edge:?}-{mode:?}"),
        layout::ResolvedAreaKind::Bar {
            edge,
            thickness: 34.0,
            length: layout::Extent::Fill,
            offset: 0.0,
            shape: layout::BarShape {
                mode: Some(mode),
                ..layout::BarShape::default()
            },
            autohide: None,
        },
        vec![plated, centre, empty, end],
    );
    bar.style = layout::Style {
        border: Some(layout::Border::default()),
        shadow: Some(2),
        ..layout::Style::default()
    };
    bar
}

/// **A style paints inside what it styles**: a bar whose groups draw plates and whose chips draw their own boxes, with borders and shadows on all of them, lays out on every edge, in every mode and on every monitor, draws, and puts none of that ink outside its strip. A shadow is no ink of a box: it is the one thing meant to fall past it.
#[test]
fn a_styled_bar_draws_its_plates_and_chips_inside_its_strip_on_every_edge_and_shape() {
    let _world = WORLD
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut faults = Vec::new();
    for mode in MODES {
        for size in MONITORS {
            for edge in Edge::ALL {
                reset_layout_runtime();
                seed_world(Edge::Top, mode, None);
                let config = config::config().expect("the sweep published a config");
                let scope = telar::owner_scope();
                let owner = scope.id();
                let area = styled_bar(edge, mode);
                let measured = measure_area(&area, size);
                drop(scope);
                let surround = surfaces::area::Surround {
                    config: &config,
                    theme: config.resolve_theme(),
                    output: Some("SWEPT-1"),
                    layer: layout::LayerKind::Desktop,
                    bounds: Rect::new(0.0, 0.0, size.0, size.1),
                    reserved: surfaces::layer_window::Reserved::default(),
                    audience: ui::host::Audience::Owner,
                };
                let strip = surfaces::bar::strip_of_area(&area, surround);
                let held = Rect::new(
                    strip.x - SLACK,
                    strip.y - SLACK,
                    strip.width + 2.0 * SLACK,
                    strip.height + 2.0 * SLACK,
                );
                let at = format!("{edge:?} on {}x{} in {mode:?}", size.0, size.1);
                match measured {
                    Err(error) => faults.push(format!("{at}: {error}")),
                    Ok(commands) => {
                        if !commands.iter().any(paints) {
                            faults.push(format!("{at}: drew nothing"));
                        }
                        let edged = commands
                            .iter()
                            .filter(|command| {
                                matches!(command, DrawCommand::Rect { style, .. } if style.border.is_some())
                            })
                            .count();
                        let expected = match mode {
                            Shape::Bar => 5,
                            _ => 4,
                        };
                        if edged != expected {
                            faults
                                .push(format!("{at}: {edged} boxes drew a border, not {expected}"));
                        }
                        let cut = commands
                            .iter()
                            .position(|command| matches!(command, DrawCommand::PushClip { .. }));
                        let lifted_outside = commands
                            .iter()
                            .enumerate()
                            .filter(|(at, command)| {
                                matches!(command, DrawCommand::Rect { style, .. }
                                    if style.shadow.is_some()
                                        && (style.fill.is_some() || style.border.is_some()))
                                    && cut.is_none_or(|cut| *at < cut)
                            })
                            .count();
                        if lifted_outside != 0 {
                            faults.push(format!(
                                "{at}: {lifted_outside} lifted boxes cast outside the strip's cut"
                            ));
                        }
                        for rect in visible_ink(&commands)
                            .into_iter()
                            .filter(|rect| rect.intersect(held) != Some(*rect))
                        {
                            faults.push(format!(
                                "{at}: {}x{} at {},{} is outside the {}x{} strip at {},{}",
                                rect.width,
                                rect.height,
                                rect.x,
                                rect.y,
                                strip.width,
                                strip.height,
                                strip.x,
                                strip.y
                            ));
                        }
                    }
                }
                telar::dispose_owner(owner);
            }
        }
    }
    assert!(
        faults.is_empty(),
        "{} styled bar(s) drew outside their strip:\n  {}",
        faults.len(),
        faults.join("\n  ")
    );
}

/// The concave pieces a fillet of `radius` drew: each a square of that side painted as a radial ramp from clear to a colour, with that colour.
fn notch_pieces(commands: &[DrawCommand], radius: f32) -> Vec<(Rect, telar::Color)> {
    under_transform(commands)
        .into_iter()
        .filter_map(|(command, at)| {
            let DrawCommand::Rect { rect, style } = command else {
                return None;
            };
            let Some(Paint::Gradient(gradient)) = style.fill else {
                return None;
            };
            match gradient.kind {
                telar::GradientKind::Radial { radius: r, .. } if r == radius => Some((
                    in_surface_space(*rect, at),
                    gradient.stops.active().last()?.color,
                )),
                _ => None,
            }
        })
        .collect()
}

/// **A fillet is drawn where a bar owns a corner, and only in `bar`**: on every edge, in every mode and on every monitor, a bar that hides itself carries a piece at each inner end, a steady horizontal bar carries one at each corner a reserving vertical bar leaves it, a vertical bar or a bar alone carries none, and `sections` and `chips` carry none at all. Each piece is on the screen and in the colour of the strip it grows out of.
#[test]
fn a_fillet_is_drawn_where_a_bar_owns_a_corner_and_in_bar_mode_only() {
    const RADIUS: f32 = 12.0;
    let _world = WORLD
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let hide = Some(layout::AutoHide {
        peek: 2.0,
        on_hover: true,
    });
    let mut faults = Vec::new();
    for mode in MODES {
        for size in MONITORS {
            for edge in Edge::ALL {
                let across: Vec<Edge> = match edge.is_horizontal() {
                    true => vec![Edge::Left, Edge::Right],
                    false => vec![Edge::Top],
                };
                let neighbours: Vec<_> = across
                    .iter()
                    .map(|side| bar_on(*side, None, None))
                    .collect();
                let cases = [
                    ("hiding", hide, &neighbours[..], 2),
                    (
                        "steady beside reserving bars",
                        None,
                        &neighbours[..],
                        if edge.is_horizontal() { 2 } else { 0 },
                    ),
                    ("steady alone", None, &[][..], 0),
                ];
                for (name, autohide, beside, in_bar_mode) in cases {
                    let expected = if mode == Shape::Bar { in_bar_mode } else { 0 };
                    let at = format!("{edge:?} {name} on {}x{} in {mode:?}", size.0, size.1);
                    let drawn = |fillet| {
                        reset_layout_runtime();
                        seed_world(Edge::Top, mode, None);
                        let scope = telar::owner_scope();
                        let owner = scope.id();
                        let measured = measure_area_beside(
                            &bar_in(Some(mode), edge, autohide, fillet),
                            beside,
                            size,
                        );
                        drop(scope);
                        telar::dispose_owner(owner);
                        measured
                    };
                    let (plain, rounded) = match (drawn(None), drawn(Some(RADIUS))) {
                        (Ok(plain), Ok(rounded)) => (plain, rounded),
                        (Err(error), _) | (_, Err(error)) => {
                            faults.push(format!("{at}: {error}"));
                            continue;
                        }
                    };
                    if !notch_pieces(&plain, RADIUS).is_empty() {
                        faults.push(format!("{at}: pieces without a fillet"));
                    }
                    let pieces = notch_pieces(&rounded, RADIUS);
                    if pieces.len() != expected {
                        faults.push(format!("{at}: {} pieces, not {expected}", pieces.len()));
                    }
                    let screen =
                        Rect::new(-SLACK, -SLACK, size.0 + 2.0 * SLACK, size.1 + 2.0 * SLACK);
                    for (rect, colour) in pieces {
                        if autohide.is_none() && rect.intersect(screen) != Some(rect) {
                            faults.push(format!("{at}: a piece at {rect:?} is off the screen"));
                        }
                        if colour.a <= 0.0 {
                            faults.push(format!("{at}: a piece with no colour"));
                        }
                        let strip_solid = rounded.iter().any(|command| {
                            matches!(command, DrawCommand::Rect { style, .. }
                                if style.fill == Some(Paint::Solid(colour)))
                        });
                        if !strip_solid {
                            faults.push(format!("{at}: a piece in a colour the strip is not"));
                        }
                    }
                }
            }
        }
    }
    assert!(
        faults.is_empty(),
        "{} fillet(s) were drawn wrong:\n  {}",
        faults.len(),
        faults.join("\n  ")
    );
}

/// Every box in `commands` that draws a line round itself: where it is on the surface, the line, and the lift under it.
fn edged(commands: &[DrawCommand]) -> Vec<(Rect, telar::Border, Option<telar::Shadow>)> {
    under_transform(commands)
        .into_iter()
        .filter_map(|(command, at)| match command {
            DrawCommand::Rect { rect, style } => {
                Some((in_surface_space(*rect, at), style.border?, style.shadow))
            }
            _ => None,
        })
        .collect()
}

/// **A container's style is the plate behind it and a child's is its own box**: every arrangement, with a lifted, lined plate and children lined and lifted in another colour, draws one plate exactly over the container's cells and one box per child on show inside it, each in the line and the lift its style names, on every monitor.
#[test]
fn a_styled_container_draws_its_plate_over_its_cells_and_a_box_round_each_child_on_show() {
    let _world = WORLD
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut faults = Vec::new();
    for size in MONITORS {
        for mut container in containers().into_iter().chain([pages_container()]) {
            let shown = match container.arrange {
                Some(layout::Arrange::Pages) => 1,
                _ => container.children.len(),
            };
            container.style = styled("accent", 2);
            for child in &mut container.children {
                child.style = styled("red", 3);
            }
            reset_layout_runtime();
            seed_world(Edge::Top, Shape::Bar, None);
            let theme = config::config()
                .expect("the sweep published a config")
                .resolve_theme();
            let line = |token: &str| telar::Border::uniform(layout::color_of(token, &theme), 2.0);
            let lift = |step| ui::scale::elevation::shadow(step);
            let scope = telar::owner_scope();
            let owner = scope.id();
            let area = area_of(
                container.id.to_string(),
                layout::ResolvedAreaKind::Grid {
                    rect: layout::Rect::default(),
                    cell: 80.0,
                    gap: 16.0,
                    anchor: layout::Anchor::TopLeft,
                },
                vec![container.clone()],
            );
            let measured = measure_area(&area, size);
            drop(scope);
            telar::dispose_owner(owner);
            let at = format!("{} on {}x{}", container.id, size.0, size.1);
            let layout::GroupKind::Cell { col, row, .. } = container.kind else {
                unreachable!("every case is on cells");
            };
            let span = surfaces::area::cells_of(&container).extent(80.0, 16.0);
            let cells = Rect::new(
                col as f32 * 96.0,
                row as f32 * 96.0,
                span.width,
                span.height,
            );
            let commands = match measured {
                Ok(commands) => commands,
                Err(error) => {
                    faults.push(format!("{at}: {error}"));
                    continue;
                }
            };
            let boxes = edged(&commands);
            let plates: Vec<_> = boxes
                .iter()
                .filter(|(_, border, shadow)| *border == line("accent") && *shadow == lift(2))
                .collect();
            let children: Vec<_> = boxes
                .iter()
                .filter(|(_, border, shadow)| *border == line("red") && *shadow == lift(3))
                .collect();
            if plates.len() != 1 {
                faults.push(format!("{at}: {} plates, not one", plates.len()));
            }
            for (rect, ..) in &plates {
                let off = (rect.x - cells.x)
                    .abs()
                    .max((rect.y - cells.y).abs())
                    .max((rect.width - cells.width).abs())
                    .max((rect.height - cells.height).abs());
                if off > SLACK {
                    faults.push(format!(
                        "{at}: the plate {rect:?} is not its cells {cells:?}"
                    ));
                }
            }
            if children.len() != shown {
                faults.push(format!("{at}: {} child boxes, not {shown}", children.len()));
            }
            let held = Rect::new(
                cells.x - SLACK,
                cells.y - SLACK,
                cells.width + 2.0 * SLACK,
                cells.height + 2.0 * SLACK,
            );
            for (rect, ..) in &children {
                if rect.intersect(held) != Some(*rect) {
                    faults.push(format!("{at}: a child box {rect:?} is past its plate"));
                }
            }
        }
    }
    assert!(
        faults.is_empty(),
        "{} styled container(s) drew wrong:\n  {}",
        faults.len(),
        faults.join("\n  ")
    );
}

/// **A dock's groups are plates and its chips boxes, whatever it is drawn as**: a dock on every edge, in `bar`, `sections` and `chips`, on every monitor, draws one plate per styled group and one box per chip on show, in their own lines and lifts, all on the screen.
#[test]
fn a_styled_dock_draws_a_plate_per_group_and_a_box_per_chip_on_every_edge_and_shape() {
    let _world = WORLD
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut faults = Vec::new();
    for mode in MODES {
        for size in MONITORS {
            for area in every_area(mode)
                .into_iter()
                .filter(|area| area.id.to_string().starts_with("styled-dock-"))
            {
                reset_layout_runtime();
                seed_world(Edge::Top, mode, None);
                let theme = config::config()
                    .expect("the sweep published a config")
                    .resolve_theme();
                let scope = telar::owner_scope();
                let owner = scope.id();
                let measured = measure_area(&area, size);
                drop(scope);
                telar::dispose_owner(owner);
                let at = format!("{} on {}x{} in {mode:?}", area.id, size.0, size.1);
                let commands = match measured {
                    Ok(commands) => commands,
                    Err(error) => {
                        faults.push(format!("{at}: {error}"));
                        continue;
                    }
                };
                let line =
                    |token: &str| telar::Border::uniform(layout::color_of(token, &theme), 2.0);
                let boxes = edged(&commands);
                let plates = boxes
                    .iter()
                    .filter(|(_, border, shadow)| {
                        *border == line("accent") && *shadow == ui::scale::elevation::shadow(2)
                    })
                    .count();
                let chips = boxes
                    .iter()
                    .filter(|(_, border, shadow)| {
                        *border == line("green") && *shadow == ui::scale::elevation::shadow(1)
                    })
                    .count();
                if plates != 3 {
                    faults.push(format!("{at}: {plates} plates, not 3"));
                }
                if chips != 3 {
                    faults.push(format!("{at}: {chips} chip boxes, not 3"));
                }
            }
        }
    }
    assert!(
        faults.is_empty(),
        "{} styled dock(s) drew wrong:\n  {}",
        faults.len(),
        faults.join("\n  ")
    );
}
