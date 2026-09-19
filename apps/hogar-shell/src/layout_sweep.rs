//! Every preview, laid out on all four edges in all three shape modes, measured rather than merely built.
//!
//! `cargo telar test` renders each preview once and fails on a panic or a layout error, which is `assert!(build().is_ok())` — and that passes for every layout bug this tree has actually had: a result list laid out 612×**0** because a `max_height` is not a height, a wallpaper row measuring shorter than the tiles inside it, an avatar drawn outside its scrolling viewport. Each was found by looking at a picture, and each is a fact about *geometry*, which is why this measures the draw commands instead of comparing stored images: a baseline would also have to be told apart from a font that shaped differently on the machine running it, and a suite that cries wolf is one people delete.
//!
//! The twelve combinations are the shell's own hard rule — every surface and every module works on all four edges and in `bar`, `sections` and `chips` — so a module only ever looked at on a top bar is exactly where a shape bug survives.
//!
//! **"Did the preview draw anything at all?" took a correction to become askable.** It reported 79 of 456 combinations blank, and the reading taken from that was that `battery`, `brightness`, `mic`, `network`, `volume` and `lockstatus` have no reading on a machine with no battery and no PipeWire — a hardware-dependent answer, and so a flaky assertion. That was wrong twice over: those chips draw their glyph at a fallback level and are on screen, and what was blank was the *accounting*. Every icon is an SVG, an SVG is a `Path`, and only boxes were counted — so a chip whose whole content is an icon was invisible to this file rather than measured by it. [`paints`] counts artwork too, which makes the question machine-independent rather than merely askable.
//!
//! **What this cannot ask: *"did anything draw outside its surface?"* — the avatar bug.** Not for the reason it looks like. Draw rects being pre-transform is twelve lines of matrix stack, which telar already owns in `DrawState` and merely does not re-export; and content legitimately below the fold has a clean discriminator, since a scroll area emits a viewport and being outside one is what scrolling means, so a draw that nothing is clipping has no such reading. What blocks it is that **a preview has no bounds to be outside of**: `PreviewSurface` is a sizing hint rather than a viewport — `surfaces::preview` gives the bar 940 × its thickness on purpose — and entries stack their variants well past it. Measured, the check calls every such gallery a fault. Asking it needs real surfaces, which a sweep over previews does not have.

#![cfg(test)]

use std::sync::Arc;

use telar::{
    AvailableSpace, ComponentList, Container, DrawCommand, LayoutError, LayoutItem, LayoutStyle,
    Paint, PreviewSurface, Rect, compute_layout, new_container, reset_layout_runtime, set_theme,
};

use config::{BarConfig, Config, Edge, ModuleEntry, Shape};
use ui::descriptor::{ChipFrame, ModuleDescriptor};
use ui::host::{Host, InstanceId, Representation, Size};

/// The page a preview is measured on when it is a tree rather than a surface. Wide enough that a bar-width module is not the thing under test.
const PAGE: (f32, f32) = (1000.0, 760.0);

/// Below this a rect is not something a user can see. Text shaping and fractional layout both land a fraction under a whole pixel routinely, so the question asked is "did this collapse", not "is this exact".
const COLLAPSED: f32 = 0.5;

const MODES: [Shape; 3] = [Shape::Bar, Shape::Sections, Shape::Chips];

/// The world one combination builds against. Deliberately [`Config::starter`] rather than the user's file: a sweep that read `~/.config/hogar-shell/config.toml` would measure a different shell on every machine.
///
/// `edit` changes the starter before its bar is moved to the edge under test, so an entry added to the top bar is measured on whichever edge is being swept.
fn seed_world(edge: Edge, mode: Shape, edit: &dyn Fn(&mut Config)) {
    let mut config = Config::starter();
    edit(&mut config);
    // `starter` puts its modules on the top bar and `drawn_edge` reports the first non-empty one, so moving them wholesale is what makes a chip believe it is on the edge under test.
    let bar = std::mem::take(&mut config.bars.top);
    *match edge {
        Edge::Top => &mut config.bars.top,
        Edge::Bottom => &mut config.bars.bottom,
        Edge::Left => &mut config.bars.left,
        Edge::Right => &mut config.bars.right,
    } = bar;
    if edge != Edge::Top {
        config.bars.top = BarConfig::default();
    }
    config.shape.mode = mode;

    let config = Arc::new(config);
    services::locale::init(config.language());
    seed_home();
    ui::icon::init_store(&config.icons);
    set_theme(config.resolve_theme());
    config::set_config(config);
    crate::install_hooks();
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

/// The bar the seeded world draws, and its thickness.
fn drawn_bar() -> (Arc<Config>, Edge, f32) {
    let config = config::config().expect("the sweep published a config");
    let edge = ui::panel::drawn_edge(&config);
    let thickness = config.bars.get(edge).size as f32;
    (config, edge, thickness)
}

/// A chip on the bar the seeded world draws; any other representation on a surface of its own the size [`module_surface`] declares.
fn module_host(id: &str, representation: Representation) -> Host {
    let (config, edge, _) = drawn_bar();
    let theme = config.resolve_theme();
    match representation {
        Representation::Chip => Host::chip(
            InstanceId::of_module(id),
            config,
            edge,
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
    let (config, edge, thickness) = drawn_bar();
    match representation {
        Representation::Chip if edge.is_horizontal() => PreviewSurface::new(940.0, thickness),
        Representation::Chip => PreviewSurface::new(thickness, 940.0),
        Representation::Popout => {
            PreviewSurface::new(config.popouts.card_width(), config.popouts.card_height())
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

/// What the subject put on screen, in the coordinates its own draw commands carry.
fn measure(subject: &Subject) -> Result<Vec<DrawCommand>, LayoutError> {
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
    Ok(tree.commands().to_vec())
}

/// The **layout box** a command is answerable for, and whether it has any content to put there — an empty `Text` shapes to nothing and is the one zero-area draw that is not a fault. `Line` and `Path` carry artwork rather than a box, and the matrix and layer markers cover nothing of their own.
///
/// Deliberately narrower than [`paints`]: what this returns is measured against [`COLLAPSED`], and only a box the layout produced can be said to have collapsed. An icon's own geometry is the artwork's business — a signal-strength glyph draws its bars as filled slivers a third of a pixel wide, and there is nothing wrong with that.
fn painted_rect(command: &DrawCommand) -> Option<Rect> {
    match command {
        DrawCommand::Rect { rect, .. } | DrawCommand::Image { rect, .. } => Some(*rect),
        DrawCommand::Text { rect, text, .. } => (!text.is_empty()).then_some(*rect),
        // A viewport clipped to nothing is the canonical shape of the bug this file exists for: the content inside keeps its own honest rects and is cut away wholesale, so only the clip itself shows the fault.
        DrawCommand::PushClip { rect, .. } => Some(*rect),
        _ => None,
    }
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

type Each<'a> = dyn FnMut(&Subject, Edge, Shape, Result<Vec<DrawCommand>, LayoutError>) + 'a;

fn sweep(mut each: impl FnMut(&Subject, Edge, Shape, Result<Vec<DrawCommand>, LayoutError>)) {
    sweep_over(everything, &|_| {}, &mut each);
}

/// [`sweep`] over a starter config that `edit` has changed first.
fn sweep_with(
    edit: &dyn Fn(&mut Config),
    mut each: impl FnMut(&Subject, Edge, Shape, Result<Vec<DrawCommand>, LayoutError>),
) {
    sweep_over(everything, edit, &mut each);
}

fn sweep_over(subjects: fn() -> Vec<Subject>, edit: &dyn Fn(&mut Config), each: &mut Each) {
    let _world = WORLD
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    for edge in Edge::ALL {
        for mode in MODES {
            // Seeded before the list is drawn up, not only before each entry is measured: an entry reads the world to declare its surface — a bar's is its thickness, on the axis it runs along — so a list enumerated first describes whichever combination happened to run before this one.
            reset_layout_runtime();
            seed_world(edge, mode, edit);
            for subject in subjects() {
                reset_layout_runtime();
                seed_world(edge, mode, edit);
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
            if rect.width >= COLLAPSED && rect.height >= COLLAPSED {
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
    sweep_with(
        &|config| config.bars.top.end.push(ModuleEntry::bare(UNKNOWN)),
        |entry, edge, mode, measured| {
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
            let surface = entry.surface.expect("the bar preview declares its surface");
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
            let on_bar =
                |start: f32, length: f32, bar: f32| (start + length).min(bar) - start.max(0.0);
            let wide = on_bar(rect.x, rect.width, surface.width);
            let tall = on_bar(rect.y, rect.height, surface.height);
            if wide < icon || tall < icon {
                wrong.push(format!(
                    "{edge:?}/{mode:?}: {wide}x{tall}px of the placeholder at {rect:?} is on the {}x{} bar — less than \
                     the {icon}px glyph it holds",
                    surface.width, surface.height
                ));
            }
            let named = commands.iter().any(
                |command| matches!(command, DrawCommand::Text { text, .. } if &**text == UNKNOWN),
            );
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
        },
    );
    assert!(
        wrong.is_empty(),
        "an unknown module did not hold a chip's place:\n  {}",
        wrong.join("\n  ")
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
