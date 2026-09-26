//! `hogar-shell layout` — today's `config.toml` read once and written back out as a layout.
//!
//! Every placement key the config has ever carried has somewhere to go in the model: a `[bars.<edge>]` table is a bar area with three zones, `[corners]` are the chips today routes into the owning bar's outer ends, `[widgets]` is the desktop layer, `[stack]` is a column on the overlay, and `[lock] show_*` are readings laid around the prompt. What the config could *not* say, and the layout must, is identity — today a module id **is** an instance, so the notifications chip in the top bar's start zone and the one in its end zone are one chip seen twice. Here each placement is its own instance with an id of its own, handed out by [`Ids`].
//!
//! This is a migration rather than a translation layer: nothing else reads it, it runs once on the user's own say-so, and it goes away with the rest of the old keys in Sprint 8 (DEC-15). So it is allowed to be literal about today's behaviour — a bar that hides itself reserves nothing, because that is what `Config::edge_reserved` does — and where the model has no way to say something, it says so on its way out instead of inventing a key to say it in.

use std::collections::BTreeMap;
use std::path::Path;

use config::{Config, Edge};
use layout::{
    Anchor, Area, AreaId, AreaKind, AreaStyle, AutoHide, BarShape, Extent, Group, GroupId,
    GroupKind, Instance, InstanceId, Layer, Layers, Layout, LayoutId, LayoutStore, OutputMatch,
    OutputRule, PromptStyle, Rect, Representation, StackOutputPolicy, Zone,
};
use util::{paths, writer};

use super::{Command, Target};

pub(crate) const LAYOUT: Target = Target {
    name: "layout",
    commands: &[
        Command {
            name: "import-config",
            args: "[--name <name>]",
            help: "turn today's bars, widgets, stack and lock screen into a layout file",
            run: import,
        },
        Command {
            name: "list",
            args: "",
            help: "every layout this shell can use",
            run: |_| Ok(list()),
        },
        Command {
            name: "show",
            args: "[name]",
            help: "print a layout as it is stored",
            run: |args| show(args.first().copied()),
        },
        Command {
            name: "check",
            args: "[name]",
            help: "report what is wrong with a layout, without applying it",
            run: |args| check(args.first().copied()),
        },
        Command {
            name: "use",
            args: "<name>",
            help: "draw this layout from now on",
            run: |args| use_layout(args.first().copied()),
        },
    ],
};

/// Makes `name` the layout the shell draws, and asks whatever is running to pick it up.
///
/// Which layout is active is machine state, not a config key (TA-2): it is a choice about this installation rather than a description of one. So this writes `state.json` and reloads, and the running shell reads the name back on the next pass — one direction, no second copy of the answer in the shell's memory to fall out of step.
fn use_layout(name: Option<&str>) -> Result<String, String> {
    let name = name.filter(|name| !name.trim().is_empty()).ok_or_else(|| {
        format!(
            "say which layout to use — `hogar-shell layout list` names them\n{}",
            list()
        )
    })?;
    let (store, _) = LayoutStore::load(layouts_dir());
    let id = LayoutId::new(name);
    if store.get(&id).is_none() {
        return Err(format!("there is no layout called `{name}`\n{}", list()));
    }
    services::state::update(|state| state.layout = Some(name.to_string()));
    config::request_reload();
    Ok(format!("drawing `{name}`"))
}

/// The layouts on disk, with the built-in one marked.
///
/// These three read files and answer from them, so they run in the CLI process the way `config check` does: a layout that stops the shell from starting is exactly the one a user needs to be able to look at.
fn list() -> String {
    let (store, report) = LayoutStore::load(layouts_dir());
    let mut lines: Vec<String> = store
        .names()
        .map(|id| match id.as_str() == layout::BUILT_IN {
            true => format!("{id}\tbuilt-in, read-only"),
            false => id.to_string(),
        })
        .collect();
    if !report.is_clean() {
        lines.push(report.render());
    }
    lines.join("\n")
}

fn show(name: Option<&str>) -> Result<String, String> {
    let (store, _) = LayoutStore::load(layouts_dir());
    let id = LayoutId::new(name.unwrap_or(layout::BUILT_IN));
    let found = store
        .get(&id)
        .ok_or_else(|| format!("there is no layout called '{id}'"))?;
    toml::to_string_pretty(found).map_err(|why| why.to_string())
}

/// Parses, validates and resolves one layout, and says what is wrong with it.
///
/// Resolution is per output, and which outputs exist is a question only a running compositor answers, so this resolves against one nominal output. That catches everything that does not depend on a monitor's name — a missing prompt, an area with no kind, an instance with no module — and leaves the per-monitor half to the running shell's own notice.
fn check(name: Option<&str>) -> Result<String, String> {
    let (store, mut report) = LayoutStore::load(layouts_dir());
    let id = LayoutId::new(name.unwrap_or(layout::BUILT_IN));
    let found = store
        .get(&id)
        .ok_or_else(|| format!("there is no layout called '{id}'"))?;

    let path = store.path_of(&id);
    if let Ok(text) = std::fs::read_to_string(&path) {
        report.merge(layout::check_unknown_keys(&text, &id));
    }
    report.merge(layout::validate(
        found,
        &Descriptors(crate::core::modules::MODULES),
    ));

    let (resolved, resolving) = layout::resolve(found, store.all(), NOMINAL_OUTPUT, None);
    report.merge(resolving);
    report.merge(layout::validate_resolved(
        &resolved,
        &path.display().to_string(),
    ));

    match report.is_clean() {
        true => Ok(report.summary()),
        false => Err(report.render()),
    }
}

/// The output a layout is resolved against when no compositor is there to name one. A glob rule matches it and a rule naming a real monitor does not, which is the right way round: a check with no compositor should not claim a monitor's rule is wrong.
const NOMINAL_OUTPUT: &str = "";

fn layouts_dir() -> std::path::PathBuf {
    paths::config_dir().join("layouts")
}

/// What the layout model has to ask the module table.
///
/// It lives here rather than in `crates/layout` because that crate deliberately knows nothing about modules or the IPC table — the whole point of `Catalogue` is that the model can be validated by a test that states its own three modules. This is the real answer, wired to the descriptors this binary ships.
///
/// **It carries the table rather than reading the installed one.** `ui::descriptor::install` runs in `setup_shell`, and these verbs answer in the CLI process where nothing has run it, so an installed-table lookup answers `None` for every module the binary has — which came out as `layout check` calling `clock` an unknown module. `config check` already takes the table as an argument for the same reason (`check::command`).
struct Descriptors(&'static [ui::descriptor::ModuleDescriptor]);

impl Descriptors {
    fn find(&self, module: &str) -> Option<&'static ui::descriptor::ModuleDescriptor> {
        ui::descriptor::lookup(self.0, module)
    }
}

impl layout::Catalogue for Descriptors {
    fn knows_module(&self, module: &str) -> bool {
        self.find(module).is_some()
    }

    fn has_representation(&self, module: &str, representation: Representation) -> bool {
        self.find(module)
            .is_some_and(|found| found.input(drawn_as(representation)).is_some())
    }

    fn is_read_only(&self, module: &str, representation: Representation) -> bool {
        self.find(module)
            .and_then(|found| found.input(drawn_as(representation)))
            .is_some_and(|input| input == ui::descriptor::Input::ReadOnly)
    }

    fn command_resolves(&self, line: &str) -> bool {
        super::resolves(line)
    }
}

/// The layout model's five placeable sizes as the descriptor table names them. The table has two more, `Panel` and `Popout`, which are opened rather than placed and so have no way to appear in a layout.
fn drawn_as(representation: Representation) -> ui::host::Representation {
    use ui::host::{Representation as Drawn, WidgetSize};
    match representation {
        Representation::Chip => Drawn::Chip,
        Representation::WidgetS => Drawn::Widget(WidgetSize::S),
        Representation::WidgetM => Drawn::Widget(WidgetSize::M),
        Representation::WidgetL => Drawn::Widget(WidgetSize::L),
        Representation::Card => Drawn::Card,
    }
}

/// The name an import writes under when it is not given one.
const IMPORTED: &str = "imported";

/// Where the two lock grids sit: above and below the prompt's own default rectangle, so neither covers the one area a lock screen cannot do without.
const ABOVE_THE_PROMPT: Rect = Rect {
    x: 0.25,
    y: 0.06,
    w: 0.5,
    h: 0.27,
};
const BELOW_THE_PROMPT: Rect = Rect {
    x: 0.25,
    y: 0.67,
    w: 0.5,
    h: 0.27,
};

/// What one import produced, before anything is written.
struct Imported {
    layout: Layout,
    notes: Notes,
}

fn import(args: &[&str]) -> Result<String, String> {
    let name = wanted_name(args)?;
    let source = Config::default_path();
    let config = Config::load(&source).map_err(|why| format!("{}: {why}", source.display()))?;
    let monitors = monitor_configs(&source)?;
    let imported = import_of(&name, &config, &monitors);
    let text = toml::to_string_pretty(&imported.layout).map_err(|why| why.to_string())?;
    let written = paths::config_dir()
        .join("layouts")
        .join(format!("{name}.toml"));
    writer::write(&written, text.into_bytes())
        .map_err(|why| format!("could not write {}: {why}", written.display()))?;
    Ok(reply(&written, &imported.notes))
}

/// The name `--name` asked for, or [`IMPORTED`].
///
/// Refused rather than sanitised when it could not be a file name of its own, and refused for the built-in layout, which is read-only: a file called `default.toml` would take its name and there would be nothing left to fall back to.
fn wanted_name(args: &[&str]) -> Result<String, String> {
    let mut words = args.iter();
    let Some(flag) = words.next() else {
        return Ok(IMPORTED.to_string());
    };
    if *flag != "--name" {
        return Err(format!(
            "unexpected argument '{flag}', expected --name <name>"
        ));
    }
    let name = words.next().ok_or("missing argument <name>")?.trim();
    if name.is_empty() || name.starts_with('.') || name.contains(['/', '\\']) {
        return Err(format!(
            "'{name}' cannot be a layout name: it becomes layouts/<name>.toml"
        ));
    }
    if name == layout::BUILT_IN {
        return Err(format!(
            "'{name}' is the built-in layout, which is read-only; pick another name"
        ));
    }
    Ok(name.to_string())
}

/// Every `monitors/<output>/config.toml` beside the config, as the whole config that output sees.
///
/// A malformed override stops the import rather than being skipped: this runs once, and a migration that quietly leaves one screen behind is one the user finds out about months later.
fn monitor_configs(source: &Path) -> Result<Vec<(String, Config)>, String> {
    let mut configs = Vec::new();
    for file in Config::monitor_overrides(source) {
        let Some(output) = file
            .parent()
            .and_then(|dir| dir.file_name())
            .and_then(|name| name.to_str())
        else {
            continue;
        };
        let config = Config::for_output(source, Some(output))
            .map_err(|why| format!("{}: {why}", file.display()))?;
        configs.push((output.to_string(), config));
    }
    Ok(configs)
}

/// Where the layout landed, and then what the user has to know about it: the keys that did not come with it, and the keys that did but that nothing reads yet. Two headings rather than one, because "gone" and "waiting on a module" are different things to do something about.
fn reply(written: &Path, notes: &Notes) -> String {
    let mut out = written.display().to_string();
    listed(&mut out, "not carried over:", &notes.unmapped);
    listed(
        &mut out,
        "carried into the layout but not yet acted on:",
        &notes.pending,
    );
    out
}

fn listed(out: &mut String, heading: &str, lines: &[String]) {
    if lines.is_empty() {
        return;
    }
    out.push('\n');
    out.push_str(heading);
    for line in lines {
        out.push_str("\n  ");
        out.push_str(line);
    }
}

/// The whole layout: what every output shows, the screens `[bars] excluded_screens` keeps bars off, and one rule per monitor override saying only what that screen does differently.
fn import_of(name: &str, config: &Config, monitors: &[(String, Config)]) -> Imported {
    let mut notes = Notes::default();
    let everywhere = layers_of(config, "config.toml", &mut notes);

    let mut outputs = vec![OutputRule {
        matches: OutputMatch("*".to_string()),
        layers: everywhere.clone(),
        workspaces: Vec::new(),
    }];
    outputs.extend(excluded_screens(config, &everywhere));
    for (output, theirs) in monitors {
        let file = format!("monitors/{output}/config.toml");
        let mine = layers_of(theirs, &file, &mut notes);
        outputs.extend(output_rule(output, &everywhere, &mine));
    }

    Imported {
        layout: Layout {
            id: LayoutId::new(name),
            name: name.to_string(),
            extends: None,
            outputs,
        },
        notes,
    }
}

/// One config as the four layers it describes.
///
/// The bars are built first, and that order is load-bearing: ids are handed out as placements are met, and `panel toggle <module>` resolves to a module's *first* instance — which should be the chip the user can see and press, not a reading on a lock screen they are not looking at.
fn layers_of(config: &Config, file: &str, notes: &mut Notes) -> Layers {
    let mut ids = Ids::default();
    let top = top_layer(config, &mut ids);
    let desktop = desktop_layer(config, &mut ids);
    let overlay = overlay_layer(config);
    let lock = lock_layer(config, file, &mut ids, notes);
    Layers {
        top,
        desktop,
        overlay,
        lock,
        ..Layers::default()
    }
}

fn top_layer(config: &Config, ids: &mut Ids) -> Layer {
    Layer {
        areas: Edge::ALL
            .into_iter()
            .filter_map(|edge| bar_area(config, edge, ids))
            .collect(),
        remove: Vec::new(),
    }
}

/// One edge's bar, or nothing when that edge has none.
///
/// `reserve` follows today's answer rather than the model's: `Config::edge_reserved` gives a bar that hides itself nothing at all, deliberately, so that a strip the user asked to be able to ignore does not tile every window short of it. The model would let such a bar reserve its peek strip instead, and that is a choice for whoever wants it, not something an import should make on the user's behalf.
fn bar_area(config: &Config, edge: Edge, ids: &mut Ids) -> Option<Area> {
    let bar = config.bars.get(edge);
    if bar.is_empty() || !config.edge_present(edge) {
        return None;
    }

    let (lead, trail) = config.corner_modules_for(edge);
    let start: Vec<config::ModuleEntry> = lead
        .map(config::ModuleEntry::bare)
        .into_iter()
        .chain(bar.start.iter().cloned())
        .collect();
    let end: Vec<config::ModuleEntry> = bar
        .end
        .iter()
        .cloned()
        .chain(trail.map(config::ModuleEntry::bare))
        .collect();

    let autohide = (!bar.persistent).then(|| AutoHide {
        peek: config.bar_peek(edge) as f32,
        on_hover: bar.show_on_hover,
    });

    Some(Area {
        id: AreaId::new(format!("bar-{}", edge.as_str())),
        kind: Some(AreaKind::Bar {
            edge: Some(edge),
            thickness: Some(bar.size as f32),
            length: Some(Extent::Fill),
            offset: Some(0.0),
            shape: bar_shape(&bar.shape),
            autohide,
        }),
        reserve: Some(bar.persistent),
        above_fullscreen: config.general.show_over_fullscreen.then_some(true),
        groups: vec![
            zone("start", Zone::Start, &start, ids),
            zone("center", Zone::Center, &bar.center, ids),
            zone("end", Zone::End, &end, ids),
        ],
        ..Area::default()
    })
}

/// A bar's own shape, and only its own. The global `[shape]` stays in `config.toml`, where the model reads it from as the answer for every bar that does not override it, so baking the resolved values into each area here would freeze a global setting into four copies that no longer follow it.
fn bar_shape(shape: &config::BarShape) -> BarShape {
    BarShape {
        mode: shape.mode,
        gap: shape.gap.map(|gap| gap as f32),
        spacing: shape.spacing.map(|spacing| spacing as f32),
        radius: shape.radius.map(|radius| radius as f32),
    }
}

/// One run of a bar, with an instance per entry placed in it.
fn zone(id: &str, zone: Zone, entries: &[config::ModuleEntry], ids: &mut Ids) -> Group {
    Group {
        id: GroupId::new(id),
        kind: Some(GroupKind::Zone { zone }),
        children: entries.iter().map(|entry| chip(entry, ids)).collect(),
        remove: Vec::new(),
    }
}

/// One bar entry as an instance of its own. `variant` and `accent` were all an entry could say about itself, and they become that instance's options — the same two keys, now on the thing they describe.
fn chip(entry: &config::ModuleEntry, ids: &mut Ids) -> Instance {
    let mut options = toml::Table::new();
    if let Some(variant) = entry.variant
        && let Ok(value) = toml::Value::try_from(variant)
    {
        options.insert("variant".to_string(), value);
    }
    if let Some(accent) = &entry.accent {
        options.insert("accent".to_string(), toml::Value::String(accent.clone()));
    }
    Instance {
        id: ids.next(&entry.id),
        module: Some(entry.id.clone()),
        representation: Some(Representation::Chip),
        options,
        ..Instance::default()
    }
}

fn desktop_layer(config: &Config, ids: &mut Ids) -> Layer {
    let mut areas = Vec::new();
    if config.widgets.clock.enabled {
        areas.push(clock_grid(&config.widgets.clock, ids));
    }
    if config.widgets.visualiser.enabled {
        areas.push(visualiser_dock(&config.widgets.visualiser, ids));
    }
    Layer {
        areas,
        remove: Vec::new(),
    }
}

/// The desktop clock, on a grid over the whole output, anchored where its nine-way placement put it.
///
/// The placement is the grid's `anchor` rather than a cell of a three-by-three grid, because a cell index says which slot of a grid a widget is in and not where on the screen that lands: `cell` is a size in pixels, so cell `(1, 1)` of an eighty-pixel grid is near the top left corner whatever `center` was meant to mean. One widget anchored in the region is what the surface drew, said in the model's own words.
///
/// `margin` is the area's `style.padding` and not an option, because it is how far the face is held off the edges of the region rather than anything about the clock: content-sized, anchored and inset is what the widgets surface does today, and all three now belong to the area that holds it. The rest of `[widgets.clock]` rides along as the instance's options, less the two keys the layout itself answers — it is here because it was switched on, and it sits where the anchor puts it.
fn clock_grid(clock: &config::DesktopClockConfig, ids: &mut Ids) -> Area {
    Area {
        id: AreaId::new("widgets"),
        kind: Some(AreaKind::Grid {
            rect: None,
            cell: None,
            gap: None,
            anchor: Some(anchored(clock.position)),
        }),
        style: inset(clock.margin),
        groups: vec![Group {
            id: GroupId::new("clock"),
            kind: Some(GroupKind::Cell {
                col: 0,
                row: 0,
                col_span: 1,
                row_span: 1,
            }),
            children: vec![Instance {
                id: ids.next("clock"),
                module: Some("clock".to_string()),
                representation: Some(Representation::WidgetM),
                options: section_options(
                    toml::Value::try_from(clock).ok(),
                    &["enabled", "position", "margin"],
                ),
                ..Instance::default()
            }],
            remove: Vec::new(),
        }],
        ..Area::default()
    }
}

/// How far an area holds its contents off its own edges. Nothing at all when the margin is zero, so an area that insets nothing says nothing.
fn inset(margin: u32) -> AreaStyle {
    AreaStyle {
        padding: (margin > 0).then_some(margin as f32),
        ..AreaStyle::default()
    }
}

/// The audio visualiser, as the strip along one edge that it already is. `reach` becomes the dock's thickness, read through `reach_px` so the imported strip is exactly as deep as the one on screen, and `margin` is the area's `style.padding` for the same reason the clock's is.
///
/// Its run is a `Zone`, deliberately and not for want of a better fit: a dock is content along an edge, and start, centre and end along an edge is exactly what a zone says — the same vocabulary a bar uses for the same question. `Center` is where the row of bars spans its edge today.
fn visualiser_dock(visualiser: &config::DesktopVisualiserConfig, ids: &mut Ids) -> Area {
    Area {
        id: AreaId::new("visualiser"),
        kind: Some(AreaKind::Dock {
            edge: Some(visualiser.edge),
            thickness: Some(visualiser.reach_px()),
        }),
        style: inset(visualiser.margin),
        groups: vec![Group {
            id: GroupId::new("row"),
            kind: Some(GroupKind::Zone { zone: Zone::Center }),
            children: vec![Instance {
                id: ids.next("visualiser"),
                module: Some("visualiser".to_string()),
                representation: Some(Representation::WidgetL),
                options: section_options(
                    toml::Value::try_from(visualiser).ok(),
                    &["enabled", "edge", "reach", "margin"],
                ),
                ..Instance::default()
            }],
            remove: Vec::new(),
        }],
        ..Area::default()
    }
}

/// The anchor a nine-way widget placement names. The two vocabularies have the same nine positions, so this is a rename and nothing else.
fn anchored(placement: config::ClockPlacement) -> Anchor {
    use config::ClockPlacement as Placed;
    match placement {
        Placed::TopLeft => Anchor::TopLeft,
        Placed::TopCenter => Anchor::Top,
        Placed::TopRight => Anchor::TopRight,
        Placed::CenterLeft => Anchor::Left,
        Placed::Center => Anchor::Center,
        Placed::CenterRight => Anchor::Right,
        Placed::BottomLeft => Anchor::BottomLeft,
        Placed::BottomCenter => Anchor::Bottom,
        Placed::BottomRight => Anchor::BottomRight,
    }
}

/// A `[toml]` section as one instance's options, less the keys the layout itself now answers.
fn section_options(section: Option<toml::Value>, answered: &[&str]) -> toml::Table {
    let Some(toml::Value::Table(mut options)) = section else {
        return toml::Table::new();
    };
    for key in answered {
        options.remove(*key);
    }
    options
}

/// The column notifications, toasts and OSDs are routed into.
///
/// One stack, with no routes and following the focused output, because that is the whole of what `[stack]` could say. Several stacks with rules between them is what the model makes possible (TA-5), not something a config ever described.
fn overlay_layer(config: &Config) -> Layer {
    Layer {
        areas: vec![Area {
            id: AreaId::new("stack"),
            kind: Some(AreaKind::Stack {
                anchor: Some(anchor_of(config.stack.edge, config.stack.align)),
                width: Some(config.stack.width),
                output_policy: Some(StackOutputPolicy::Focused),
                routes: Vec::new(),
            }),
            ..Area::default()
        }],
        remove: Vec::new(),
    }
}

/// The corner an edge and an alignment pin a stack to. They say the same thing one anchor does: along a horizontal edge the alignment runs left to right, along a vertical one it runs top to bottom.
fn anchor_of(edge: Edge, align: config::Align) -> Anchor {
    match (edge, align) {
        (Edge::Top, config::Align::Start) => Anchor::TopLeft,
        (Edge::Top, config::Align::Center) => Anchor::Top,
        (Edge::Top, config::Align::End) => Anchor::TopRight,
        (Edge::Bottom, config::Align::Start) => Anchor::BottomLeft,
        (Edge::Bottom, config::Align::Center) => Anchor::Bottom,
        (Edge::Bottom, config::Align::End) => Anchor::BottomRight,
        (Edge::Left, config::Align::Start) => Anchor::TopLeft,
        (Edge::Left, config::Align::Center) => Anchor::Left,
        (Edge::Left, config::Align::End) => Anchor::BottomLeft,
        (Edge::Right, config::Align::Start) => Anchor::TopRight,
        (Edge::Right, config::Align::Center) => Anchor::Right,
        (Edge::Right, config::Align::End) => Anchor::BottomRight,
    }
}

/// The lock screen's content set, in the order the card shows it: the clock and the user above the field, and what is playing, the weather, the machine and what is waiting below it.
///
/// The prompt is written last because anything after it in z-order would cover it, which validation refuses outright — so the file's order is also the reason it is safe.
fn lock_layer(config: &Config, file: &str, ids: &mut Ids, notes: &mut Notes) -> Layer {
    let lock = &config.lock;

    let mut user = toml::Table::new();
    if !lock.show_avatar {
        user.insert("show_avatar".to_string(), toml::Value::Boolean(false));
        notes.pending(format!(
            "{file} [lock] show_avatar = false — it is on the `user` instance now, and the reading will honour it once the module declares the option"
        ));
    }

    let greeting = vec![
        row("clock", 0, vec![reading("clock", toml::Table::new(), ids)]),
        row("user", 1, vec![reading("user", user, ids)]),
    ];

    let mut readings: Vec<Group> = Vec::new();
    if lock.show_media {
        let at = readings.len() as u32;
        readings.push(row(
            "media",
            at,
            vec![reading("media", toml::Table::new(), ids)],
        ));
    }
    if lock.show_weather {
        let at = readings.len() as u32;
        readings.push(row(
            "weather",
            at,
            vec![reading("weather", toml::Table::new(), ids)],
        ));
    }
    if lock.show_resources {
        let at = readings.len() as u32;
        readings.push(row(
            "resources",
            at,
            vec![
                reading("cpu", toml::Table::new(), ids),
                reading("memory", toml::Table::new(), ids),
            ],
        ));
    }
    if lock.show_notifications {
        let at = readings.len() as u32;
        let mut options = toml::Table::new();
        options.insert(
            "notification_detail".to_string(),
            toml::Value::String(detail(lock.hide_notifs).to_string()),
        );
        readings.push(row(
            "notifications",
            at,
            vec![reading("notifications", options, ids)],
        ));
    }

    let mut areas = vec![grid("greeting", ABOVE_THE_PROMPT, Anchor::Bottom, greeting)];
    if !readings.is_empty() {
        areas.push(grid("readings", BELOW_THE_PROMPT, Anchor::Top, readings));
    }
    areas.push(prompt_area());
    Layer {
        areas,
        remove: Vec::new(),
    }
}

/// What a notification reading says about itself on a screen anyone walking past can read: how many are waiting, or which applications they came from. Never a body, at either setting.
fn detail(hidden: bool) -> &'static str {
    if hidden { "count" } else { "apps" }
}

/// One row of a lock grid, holding the readings that share it.
fn row(id: &str, at: u32, children: Vec<Instance>) -> Group {
    Group {
        id: GroupId::new(id),
        kind: Some(GroupKind::Cell {
            col: 0,
            row: at,
            col_span: 1,
            row_span: 1,
        }),
        children,
        remove: Vec::new(),
    }
}

/// One reading, at the size a lock screen shows it — the largest every one of them can be drawn at.
fn reading(module: &str, options: toml::Table, ids: &mut Ids) -> Instance {
    Instance {
        id: ids.next(module),
        module: Some(module.to_string()),
        representation: Some(Representation::WidgetM),
        options,
        ..Instance::default()
    }
}

/// One of the lock layer's two grids, anchored towards the prompt: the greeting sits at the bottom of the space above it and the readings at the top of the space below it, which is the contiguous centred card the lock screen draws today.
fn grid(id: &str, rect: Rect, anchor: Anchor, groups: Vec<Group>) -> Area {
    Area {
        id: AreaId::new(id),
        kind: Some(AreaKind::Grid {
            rect: Some(rect),
            cell: None,
            gap: None,
            anchor: Some(anchor),
        }),
        groups,
        ..Area::default()
    }
}

/// The password field, at the rectangle the model itself defaults to, so that the one area a lock screen cannot do without is not pinned here to a number this file made up.
fn prompt_area() -> Area {
    Area {
        id: AreaId::new("prompt"),
        kind: Some(AreaKind::Prompt {
            rect: None,
            style: PromptStyle::default(),
        }),
        ..Area::default()
    }
}

/// The outputs `[bars] excluded_screens` keeps bars off, as rules that take the bar areas away again.
fn excluded_screens(config: &Config, everywhere: &Layers) -> Vec<OutputRule> {
    let bars: Vec<AreaId> = everywhere
        .top
        .areas
        .iter()
        .map(|area| area.id.clone())
        .collect();
    if bars.is_empty() {
        return Vec::new();
    }
    config
        .bars
        .excluded_screens
        .iter()
        .map(|pattern| OutputRule {
            matches: OutputMatch(pattern.clone()),
            layers: Layers {
                top: Layer {
                    areas: Vec::new(),
                    remove: bars.clone(),
                },
                ..Layers::default()
            },
            workspaces: Vec::new(),
        })
        .collect()
}

/// One monitor's rule, or nothing when its file said nothing about the layout.
fn output_rule(output: &str, everywhere: &Layers, theirs: &Layers) -> Option<OutputRule> {
    let layers = differences(everywhere, theirs);
    let empty = [
        &layers.background,
        &layers.desktop,
        &layers.top,
        &layers.overlay,
        &layers.lock,
    ]
    .into_iter()
    .all(|layer| layer.areas.is_empty() && layer.remove.is_empty());
    (!empty).then(|| OutputRule {
        matches: OutputMatch(output.to_string()),
        layers,
        workspaces: Vec::new(),
    })
}

/// What one output says that every output does not.
///
/// Merging is by id, so a rule that repeats an area's id and writes one field leaves the rest of that area — and everything inside it — following the bar every screen has. That is the whole of why per-monitor layout keys stop being a wholesale array replacement (D-21): a file that used to be a copy of the global bar with one number changed, and drifted from it a key at a time, becomes the one number.
fn differences(everywhere: &Layers, theirs: &Layers) -> Layers {
    Layers {
        background: layer_differences(&everywhere.background, &theirs.background),
        desktop: layer_differences(&everywhere.desktop, &theirs.desktop),
        top: layer_differences(&everywhere.top, &theirs.top),
        overlay: layer_differences(&everywhere.overlay, &theirs.overlay),
        lock: layer_differences(&everywhere.lock, &theirs.lock),
    }
}

fn layer_differences(everywhere: &Layer, theirs: &Layer) -> Layer {
    let mut areas = Vec::new();
    for area in &theirs.areas {
        match everywhere.areas.iter().find(|held| held.id == area.id) {
            Some(held) => areas.extend(area_differences(held, area)),
            None => areas.push(area.clone()),
        }
    }
    Layer {
        areas,
        remove: everywhere
            .areas
            .iter()
            .filter(|area| !theirs.areas.iter().any(|mine| mine.id == area.id))
            .map(|area| area.id.clone())
            .collect(),
    }
}

fn area_differences(everywhere: &Area, theirs: &Area) -> Option<Area> {
    let mut groups = Vec::new();
    for group in &theirs.groups {
        match everywhere.groups.iter().find(|held| held.id == group.id) {
            Some(held) => groups.extend(group_differences(held, group)),
            None => groups.push(group.clone()),
        }
    }

    let area = Area {
        id: theirs.id.clone(),
        kind: kind_differences(&everywhere.kind, &theirs.kind),
        reserve: changed(&everywhere.reserve, &theirs.reserve),
        above_fullscreen: changed(&everywhere.above_fullscreen, &theirs.above_fullscreen),
        style: style_differences(&everywhere.style, &theirs.style),
        groups,
        remove: everywhere
            .groups
            .iter()
            .filter(|group| !theirs.groups.iter().any(|mine| mine.id == group.id))
            .map(|group| group.id.clone())
            .collect(),
        ..Area::default()
    };
    (!says_nothing(&area)).then_some(area)
}

/// The appearance fields the two levels disagree about. Only `padding` is ever set by an import, but a rule that carried one field of a style and dropped the rest would be a worse bug than one that never carried any.
fn style_differences(everywhere: &AreaStyle, theirs: &AreaStyle) -> AreaStyle {
    AreaStyle {
        fill: changed(&everywhere.fill, &theirs.fill),
        radius: changed(&everywhere.radius, &theirs.radius),
        opacity: changed(&everywhere.opacity, &theirs.opacity),
        padding: changed(&everywhere.padding, &theirs.padding),
        backdrop: changed(&everywhere.backdrop, &theirs.backdrop),
    }
}

/// The geometry fields the two levels disagree about, under the same variant tag so the area is still unambiguously the kind of region it was.
fn kind_differences(everywhere: &Option<AreaKind>, theirs: &Option<AreaKind>) -> Option<AreaKind> {
    let theirs = theirs.as_ref()?;
    let Some(everywhere) = everywhere else {
        return Some(theirs.clone());
    };
    if !everywhere.is_same_kind(theirs) {
        return Some(theirs.clone());
    }
    Some(match (everywhere, theirs) {
        (
            AreaKind::Bar {
                edge,
                thickness,
                length,
                offset,
                shape,
                autohide,
            },
            AreaKind::Bar {
                edge: their_edge,
                thickness: their_thickness,
                length: their_length,
                offset: their_offset,
                shape: their_shape,
                autohide: their_autohide,
            },
        ) => AreaKind::Bar {
            edge: changed(edge, their_edge),
            thickness: changed(thickness, their_thickness),
            length: changed(length, their_length),
            offset: changed(offset, their_offset),
            shape: BarShape {
                mode: changed(&shape.mode, &their_shape.mode),
                gap: changed(&shape.gap, &their_shape.gap),
                spacing: changed(&shape.spacing, &their_shape.spacing),
                radius: changed(&shape.radius, &their_shape.radius),
            },
            autohide: changed(autohide, their_autohide),
        },
        (
            AreaKind::Grid {
                rect,
                cell,
                gap,
                anchor,
            },
            AreaKind::Grid {
                rect: their_rect,
                cell: their_cell,
                gap: their_gap,
                anchor: their_anchor,
            },
        ) => AreaKind::Grid {
            rect: changed(rect, their_rect),
            cell: changed(cell, their_cell),
            gap: changed(gap, their_gap),
            anchor: changed(anchor, their_anchor),
        },
        (
            AreaKind::Dock { edge, thickness },
            AreaKind::Dock {
                edge: their_edge,
                thickness: their_thickness,
            },
        ) => AreaKind::Dock {
            edge: changed(edge, their_edge),
            thickness: changed(thickness, their_thickness),
        },
        (
            AreaKind::Stack {
                anchor,
                width,
                output_policy,
                routes,
            },
            AreaKind::Stack {
                anchor: their_anchor,
                width: their_width,
                output_policy: their_policy,
                routes: their_routes,
            },
        ) => AreaKind::Stack {
            anchor: changed(anchor, their_anchor),
            width: changed(width, their_width),
            output_policy: changed(output_policy, their_policy),
            routes: match routes == their_routes {
                true => Vec::new(),
                false => their_routes.clone(),
            },
        },
        (
            AreaKind::Prompt { rect, style },
            AreaKind::Prompt {
                rect: their_rect,
                style: their_style,
            },
        ) => AreaKind::Prompt {
            rect: changed(rect, their_rect),
            style: PromptStyle {
                fill: changed(&style.fill, &their_style.fill),
                radius: changed(&style.radius, &their_style.radius),
                opacity: changed(&style.opacity, &their_style.opacity),
            },
        },
        _ => theirs.clone(),
    })
}

fn group_differences(everywhere: &Group, theirs: &Group) -> Option<Group> {
    let kind = (everywhere.kind != theirs.kind)
        .then_some(theirs.kind)
        .flatten();
    let group = if merges_in_order(everywhere, theirs) {
        Group {
            id: theirs.id.clone(),
            kind,
            children: theirs
                .children
                .iter()
                .filter(|child| {
                    !everywhere
                        .children
                        .iter()
                        .any(|held| held.id == child.id && same_instance(held, child))
                })
                .cloned()
                .collect(),
            remove: everywhere
                .children
                .iter()
                .filter(|child| !theirs.children.iter().any(|mine| mine.id == child.id))
                .map(|child| child.id.clone())
                .collect(),
        }
    } else {
        Group {
            id: theirs.id.clone(),
            kind,
            children: theirs.children.clone(),
            remove: everywhere
                .children
                .iter()
                .map(|child| child.id.clone())
                .collect(),
        }
    };
    (group.kind.is_some() || !group.children.is_empty() || !group.remove.is_empty())
        .then_some(group)
}

/// Whether laying `theirs` over what every output has leaves the run in the order `theirs` wrote it.
///
/// A merge keeps the order the earlier level put its children in and appends the ones it had not seen, so a screen that only adds or drops a chip says so in a line. A screen that *reorders* them cannot be said that way at all, and the rule takes the whole run away by id and places it again — removals are applied before additions, so what it writes is what it gets.
fn merges_in_order(everywhere: &Group, theirs: &Group) -> bool {
    let kept = everywhere
        .children
        .iter()
        .map(|child| &child.id)
        .filter(|id| theirs.children.iter().any(|mine| &mine.id == *id));
    let added = theirs
        .children
        .iter()
        .map(|child| &child.id)
        .filter(|id| !everywhere.children.iter().any(|held| &held.id == *id));
    let merged: Vec<&InstanceId> = kept.chain(added).collect();
    let wanted: Vec<&InstanceId> = theirs.children.iter().map(|child| &child.id).collect();
    merged == wanted
}

/// Whether the two levels place the same instance. `actions` is left out of the comparison because an import never writes one — nothing in today's config binds a gesture.
fn same_instance(everywhere: &Instance, theirs: &Instance) -> bool {
    everywhere.module == theirs.module
        && everywhere.representation == theirs.representation
        && everywhere.options == theirs.options
        && everywhere.bindings == theirs.bindings
}

/// Whether this rule's area would say nothing at all: an id and a variant tag, and no field under either.
fn says_nothing(area: &Area) -> bool {
    area.reserve.is_none()
        && area.above_fullscreen.is_none()
        && area.style.is_empty()
        && area.groups.is_empty()
        && area.remove.is_empty()
        && match &area.kind {
            None => true,
            Some(AreaKind::Bar {
                edge,
                thickness,
                length,
                offset,
                shape,
                autohide,
            }) => {
                edge.is_none()
                    && thickness.is_none()
                    && length.is_none()
                    && offset.is_none()
                    && shape.is_empty()
                    && autohide.is_none()
            }
            Some(AreaKind::Grid {
                rect,
                cell,
                gap,
                anchor,
            }) => rect.is_none() && cell.is_none() && gap.is_none() && anchor.is_none(),
            Some(AreaKind::Dock { edge, thickness }) => edge.is_none() && thickness.is_none(),
            Some(AreaKind::Stack {
                anchor,
                width,
                output_policy,
                routes,
            }) => {
                anchor.is_none() && width.is_none() && output_policy.is_none() && routes.is_empty()
            }
            Some(AreaKind::Prompt { rect, style }) => rect.is_none() && style.is_empty(),
            Some(_) => false,
        }
}

/// The field when the two levels disagree about it, and nothing when they agree. A level can only ever *set* a field, so an output that has nothing where every output has something says nothing here either.
fn changed<T: Clone + PartialEq>(everywhere: &Option<T>, theirs: &Option<T>) -> Option<T> {
    (everywhere != theirs).then(|| theirs.clone()).flatten()
}

/// Instance ids that read like the module they show: `clock`, then `clock-2`, `clock-3` for every later placement of it.
///
/// Today one module id is one instance, so the same module placed on two bars is one chip drawn twice and addressed once. Each placement becoming its own instance is the point of the model, and each needs an id — readable rather than a ULID because this file is one a user opens, and because `panel toggle <module>` resolves to a module's first instance, which is the one that keeps the bare name.
#[derive(Default)]
struct Ids {
    placed: BTreeMap<String, usize>,
}

impl Ids {
    fn next(&mut self, module: &str) -> InstanceId {
        let placed = self.placed.entry(module.to_string()).or_default();
        *placed += 1;
        match *placed {
            1 => InstanceId::new(module),
            nth => InstanceId::new(format!("{module}-{nth}")),
        }
    }
}

/// What the import has to say for itself, in the two ways a key can fail to arrive whole.
///
/// `unmapped` is empty as the model stands: every placement key today's config has now has somewhere to go, the last of them once `AutoHide` grew `on_hover`. It stays because it is the half of the report the user cannot work out for themselves — a key that is simply gone leaves nothing in the layout to notice — and the next key the model cannot say has a line waiting for it instead of being dropped in silence.
#[derive(Default)]
struct Notes {
    /// Keys the model has no way to say, which the layout therefore does not carry at all.
    unmapped: Vec<String>,
    /// Keys the layout carries but that nothing reads yet, so they will start acting when the module that owns them declares them.
    pending: Vec<String>,
}

impl Notes {
    fn pending(&mut self, line: String) {
        Self::once(&mut self.pending, line);
    }

    /// Records a note once, however many of the configs it was met in — a per-monitor file repeating what the global one said is one thing to fix, not two.
    fn once(lines: &mut Vec<String>, line: String) {
        if !lines.contains(&line) {
            lines.push(line);
        }
    }
}

#[cfg(test)]
mod tests {
    use config::{BarConfig, ModuleEntry, Shape};
    use layout::{LayerKind, Resolved, ResolvedAreaKind, resolve};

    use super::*;

    fn parsed(text: &str) -> Config {
        toml::from_str(text).expect("the config parses")
    }

    fn importing(config: &Config) -> Imported {
        import_of(IMPORTED, config, &[])
    }

    /// The arrangement one output ends up with, with nothing left to decide — and nothing wrong with it, asserted here so that no test below has to remember to.
    fn arrangement(imported: &Imported, output: &str) -> Resolved {
        let (resolved, report) = resolve(&imported.layout, &BTreeMap::new(), output, None);
        assert!(report.is_clean(), "{}", report.render());
        resolved
    }

    fn bar(resolved: &Resolved, id: &str) -> ResolvedAreaKind {
        resolved
            .layer(LayerKind::Top)
            .expect("the top layer is there")
            .areas
            .iter()
            .find(|area| area.id.as_str() == id)
            .unwrap_or_else(|| panic!("'{id}' is on the top layer"))
            .kind
            .clone()
    }

    /// Every chip on every bar, as `(instance, module)` in the order the bars draw them. The lock layer places readings of its own, so a question about the bars asks only the bars.
    fn chips(resolved: &Resolved) -> Vec<(String, String)> {
        resolved
            .layer(LayerKind::Top)
            .map(|layer| {
                layer
                    .areas
                    .iter()
                    .flat_map(|area| area.groups.iter())
                    .flat_map(|group| group.children.iter())
                    .map(|instance| (instance.id.to_string(), instance.module.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn modules(resolved: &Resolved) -> Vec<String> {
        chips(resolved)
            .into_iter()
            .map(|(_, module)| module)
            .collect()
    }

    /// The import had nothing to say beyond the file it wrote: nothing left behind, and nothing waiting on a module to honour it.
    fn nothing_to_report(imported: &Imported) {
        assert!(
            imported.notes.unmapped.is_empty() && imported.notes.pending.is_empty(),
            "left behind {:?}, waiting on {:?}",
            imported.notes.unmapped,
            imported.notes.pending
        );
    }

    #[test]
    fn the_starter_config_imports_to_the_bar_it_draws_today() {
        let imported = importing(&Config::starter());
        nothing_to_report(&imported);

        let resolved = arrangement(&imported, "DP-1");
        let ResolvedAreaKind::Bar {
            edge,
            thickness,
            length,
            offset,
            shape,
            autohide,
        } = bar(&resolved, "bar-top")
        else {
            panic!("the starter config's one bar is a bar");
        };
        assert_eq!(edge, Edge::Top);
        assert_eq!(thickness, 34.0);
        assert_eq!(length, Extent::Fill);
        assert_eq!(offset, 0.0);
        assert!(
            shape.is_empty(),
            "it overrides nothing the global shape says"
        );
        assert!(autohide.is_none(), "and it stays on screen");
        assert_eq!(
            resolved.reserved(Edge::Top),
            34.0,
            "reserving exactly what it reserved before"
        );

        let zones: Vec<(String, GroupKind)> =
            resolved.layer(LayerKind::Top).expect("the top layer").areas[0]
                .groups
                .iter()
                .map(|group| (group.id.to_string(), group.kind))
                .collect();
        assert_eq!(
            zones,
            vec![
                ("start".to_string(), GroupKind::Zone { zone: Zone::Start }),
                ("center".to_string(), GroupKind::Zone { zone: Zone::Center }),
                ("end".to_string(), GroupKind::Zone { zone: Zone::End }),
            ],
            "three zones, in the order a bar reads"
        );
        assert_eq!(
            chips(&resolved),
            vec![
                ("workspaces".to_string(), "workspaces".to_string()),
                ("clock".to_string(), "clock".to_string()),
                ("notes".to_string(), "notes".to_string()),
            ],
            "holding what the bar holds, in the order it draws it"
        );
    }

    #[test]
    fn a_module_placed_twice_becomes_two_instances_that_can_be_told_apart() {
        let config = parsed(
            r#"
            [bars.top]
            start = ["notifications"]
            end = ["notifications", "battery"]
            [bars.bottom]
            start = ["battery"]
            "#,
        );
        assert_eq!(
            chips(&arrangement(&importing(&config), "DP-1")),
            vec![
                ("notifications".to_string(), "notifications".to_string()),
                ("notifications-2".to_string(), "notifications".to_string()),
                ("battery".to_string(), "battery".to_string()),
                ("battery-2".to_string(), "battery".to_string()),
            ],
            "each placement is its own instance, and the first of each keeps the bare name"
        );
    }

    #[test]
    fn an_entrys_variant_and_accent_become_that_instances_own_options() {
        let config = parsed(
            r#"
            [bars.top]
            start = [{ id = "clock", variant = "filled", accent = "red" }, "notes"]
            "#,
        );
        let resolved = arrangement(&importing(&config), "DP-1");
        let placed =
            &resolved.layer(LayerKind::Top).expect("the top layer").areas[0].groups[0].children;
        assert_eq!(
            placed[0].options.get("variant").and_then(|it| it.as_str()),
            Some("filled")
        );
        assert_eq!(
            placed[0].options.get("accent").and_then(|it| it.as_str()),
            Some("red")
        );
        assert!(
            placed[1].options.is_empty(),
            "an entry that said nothing about itself carries nothing"
        );
    }

    /// One fixture per edge and shape mode: the two together are what a bar *is* to whoever set it up, and the mode was read from a different table than the edge.
    #[test]
    fn every_edge_and_shape_mode_imports_to_the_bar_it_names() {
        for edge in Edge::ALL {
            for (mode, wanted) in [
                ("bar", Shape::Bar),
                ("sections", Shape::Sections),
                ("chips", Shape::Chips),
            ] {
                let fixture = format!(
                    r#"
                    [bars.{edge}]
                    size = 40
                    start = ["clock"]
                    [bars.{edge}.shape]
                    mode = "{mode}"
                    radius = 12
                    "#,
                    edge = edge.as_str(),
                );
                let imported = importing(&parsed(&fixture));
                nothing_to_report(&imported);

                let resolved = arrangement(&imported, "DP-1");
                let ResolvedAreaKind::Bar {
                    edge: on,
                    thickness,
                    shape,
                    ..
                } = bar(&resolved, &format!("bar-{}", edge.as_str()))
                else {
                    panic!("a {mode} bar on {} is a bar", edge.as_str());
                };
                assert_eq!(on, edge);
                assert_eq!(thickness, 40.0);
                assert_eq!(
                    shape.mode,
                    Some(wanted),
                    "the per-bar mode rides with the bar"
                );
                assert_eq!(shape.radius, Some(12.0));
                assert_eq!(
                    resolved
                        .layer(LayerKind::Top)
                        .expect("the top layer")
                        .areas
                        .len(),
                    1,
                    "and an edge with nothing on it has no area at all"
                );
            }
        }
    }

    #[test]
    fn a_global_shape_stays_where_it_is_rather_than_being_copied_onto_every_bar() {
        let config = parsed(
            r#"
            [shape]
            mode = "chips"
            radius = 20
            [bars.top]
            start = ["clock"]
            "#,
        );
        let imported = importing(&config);
        nothing_to_report(&imported);
        let ResolvedAreaKind::Bar { shape, .. } = bar(&arrangement(&imported, "DP-1"), "bar-top")
        else {
            panic!("still a bar");
        };
        assert!(
            shape.is_empty(),
            "the bar overrides nothing, so it keeps following `[shape]`"
        );
    }

    #[test]
    fn a_bar_that_hides_itself_carries_its_peek_strip_and_reserves_nothing() {
        let config = parsed(
            r#"
            [bars.bottom]
            start = ["clock"]
            persistent = false
            peek = 4
            "#,
        );
        let imported = importing(&config);
        nothing_to_report(&imported);

        let resolved = arrangement(&imported, "DP-1");
        let ResolvedAreaKind::Bar { autohide, .. } = bar(&resolved, "bar-bottom") else {
            panic!("still a bar");
        };
        assert_eq!(
            autohide,
            Some(AutoHide {
                peek: 4.0,
                on_hover: true
            })
        );
        assert_eq!(
            resolved.reserved(Edge::Bottom),
            0.0,
            "a bar that is not there most of the time tiles no window short of it, as before"
        );
    }

    #[test]
    fn a_bar_that_only_a_drag_brings_back_keeps_saying_so() {
        let config = parsed(
            r#"
            [bars.top]
            start = ["clock"]
            persistent = false
            show_on_hover = false
            "#,
        );
        let imported = importing(&config);
        nothing_to_report(&imported);
        let ResolvedAreaKind::Bar { autohide, .. } =
            bar(&arrangement(&imported, "DP-1"), "bar-top")
        else {
            panic!("still a bar");
        };
        assert_eq!(
            autohide.map(|hide| hide.on_hover),
            Some(false),
            "a pointer crossing the edge must not bring it back, as before"
        );
    }

    #[test]
    fn showing_the_bars_over_a_fullscreen_window_becomes_a_flag_on_each_of_them() {
        let config = parsed(
            r#"
            [general]
            show_over_fullscreen = true
            [bars.top]
            start = ["clock"]
            [bars.bottom]
            start = ["notes"]
            "#,
        );
        let resolved = arrangement(&importing(&config), "DP-1");
        assert!(
            resolved
                .layer(LayerKind::Top)
                .expect("the top layer")
                .areas
                .iter()
                .all(|area| area.above_fullscreen),
            "every bar, since the key was never one bar's to set"
        );
    }

    #[test]
    fn the_corner_modules_land_at_the_ends_of_the_bar_that_owns_them() {
        let config = parsed(
            r#"
            [bars.top]
            start = ["workspaces"]
            end = ["clock"]
            [corners]
            top_left = "logo"
            top_right = "tray"
            "#,
        );
        assert_eq!(
            modules(&arrangement(&importing(&config), "DP-1")),
            vec!["logo", "workspaces", "clock", "tray"],
            "the leading corner opens the start zone and the trailing one closes the end zone"
        );
    }

    #[test]
    fn the_desktop_widgets_become_a_grid_cell_and_a_dock() {
        let config = parsed(
            r#"
            [widgets.clock]
            enabled = true
            position = "bottom_right"
            scale = 4.0
            margin = 64
            [widgets.visualiser]
            enabled = true
            edge = "left"
            reach = 200
            margin = 0
            "#,
        );
        let imported = importing(&config);
        nothing_to_report(&imported);

        let resolved = arrangement(&imported, "DP-1");
        let desktop = resolved.layer(LayerKind::Desktop).expect("a desktop layer");
        assert_eq!(
            desktop
                .areas
                .iter()
                .map(|area| area.id.to_string())
                .collect::<Vec<_>>(),
            vec!["widgets", "visualiser"]
        );
        let ResolvedAreaKind::Grid { anchor, .. } = &desktop.areas[0].kind else {
            panic!("the clock sits on a grid");
        };
        assert_eq!(
            *anchor,
            Anchor::BottomRight,
            "the nine-way placement is where the grid's cells sit in the region"
        );
        assert_eq!(
            desktop.areas[0].groups[0].kind,
            GroupKind::Cell {
                col: 0,
                row: 0,
                col_span: 1,
                row_span: 1
            },
            "and one widget is one cell, not a cell chosen out of nine"
        );

        assert_eq!(
            desktop.areas[0].style.padding,
            Some(64.0),
            "how far the face is held off the edges is the area's, not the clock's"
        );

        let clock = &desktop.areas[0].groups[0].children[0];
        assert_eq!(
            clock.options.get("scale").and_then(|it| it.as_float()),
            Some(4.0)
        );
        assert!(
            ["enabled", "position", "margin"]
                .iter()
                .all(|key| clock.options.get(*key).is_none()),
            "and what the layout itself now says is not said twice: {:?}",
            clock.options
        );

        assert!(
            desktop.areas[1].style.is_empty(),
            "a visualiser held off nothing insets nothing"
        );

        let ResolvedAreaKind::Dock { edge, thickness } = &desktop.areas[1].kind else {
            panic!("the visualiser is a dock");
        };
        assert_eq!(*edge, Edge::Left);
        assert_eq!(*thickness, 200.0);
    }

    #[test]
    fn the_notification_column_becomes_one_stack_anchored_where_it_sat() {
        let config = parsed(
            r#"
            [stack]
            edge = "bottom"
            align = "start"
            width = 420.0
            "#,
        );
        let resolved = arrangement(&importing(&config), "DP-1");
        let ResolvedAreaKind::Stack {
            anchor,
            width,
            output_policy,
            routes,
        } = &resolved
            .layer(LayerKind::Overlay)
            .expect("an overlay layer")
            .areas[0]
            .kind
        else {
            panic!("the column is a stack");
        };
        assert_eq!(*anchor, Anchor::BottomLeft);
        assert_eq!(*width, 420.0);
        assert_eq!(*output_policy, StackOutputPolicy::Focused);
        assert!(routes.is_empty(), "everything still goes to the one column");
    }

    #[test]
    fn the_lock_layer_keeps_todays_content_set_around_the_prompt() {
        let resolved = arrangement(&importing(&Config::default()), "DP-1");
        let lock = resolved.layer(LayerKind::Lock).expect("a lock layer");
        assert_eq!(
            lock.areas
                .iter()
                .map(|area| area.id.to_string())
                .collect::<Vec<_>>(),
            vec!["greeting", "readings", "prompt"],
            "the prompt is last, because anything after it would be covering it"
        );

        let readings: Vec<&str> = lock
            .areas
            .iter()
            .flat_map(|area| area.groups.iter())
            .flat_map(|group| group.children.iter())
            .map(|instance| instance.module.as_str())
            .collect();
        assert_eq!(
            readings,
            vec!["clock", "user", "media", "notifications"],
            "the rows `[lock]` has on by default, in the order the card shows them"
        );

        let notifications = lock.areas[1]
            .groups
            .iter()
            .flat_map(|group| group.children.iter())
            .find(|instance| instance.module == "notifications")
            .expect("the notification reading is placed");
        assert_eq!(
            notifications
                .options
                .get("notification_detail")
                .and_then(|it| it.as_str()),
            Some("count"),
            "hidden bodies become the count, which is what the lock screen showed"
        );
    }

    #[test]
    fn a_lock_screen_told_to_show_everything_keeps_the_machine_in_one_row() {
        let config = parsed(
            r#"
            [lock]
            show_weather = true
            show_resources = true
            hide_notifs = false
            "#,
        );
        let imported = importing(&config);
        nothing_to_report(&imported);

        let resolved = arrangement(&imported, "DP-1");
        let rows: Vec<(String, Vec<&str>)> =
            resolved.layer(LayerKind::Lock).expect("a lock layer").areas[1]
                .groups
                .iter()
                .map(|group| {
                    (
                        group.id.to_string(),
                        group
                            .children
                            .iter()
                            .map(|instance| instance.module.as_str())
                            .collect(),
                    )
                })
                .collect();
        assert_eq!(
            rows,
            vec![
                ("media".to_string(), vec!["media"]),
                ("weather".to_string(), vec!["weather"]),
                ("resources".to_string(), vec!["cpu", "memory"]),
                ("notifications".to_string(), vec!["notifications"]),
            ],
            "the machine's two readings share the row they share today"
        );

        let detail = resolved
            .instances()
            .find(|instance| instance.module == "notifications")
            .and_then(|instance| instance.options.get("notification_detail").cloned());
        assert_eq!(detail.as_ref().and_then(|it| it.as_str()), Some("apps"));
    }

    #[test]
    fn a_lock_screen_with_no_picture_carries_the_key_and_says_nothing_reads_it_yet() {
        let config = parsed(
            r#"
            [lock]
            show_avatar = false
            "#,
        );
        let imported = importing(&config);
        assert!(
            imported.notes.unmapped.is_empty(),
            "it is carried, not lost: {:?}",
            imported.notes.unmapped
        );
        assert_eq!(
            imported.notes.pending.len(),
            1,
            "{:?}",
            imported.notes.pending
        );
        assert!(
            imported.notes.pending[0].contains("show_avatar"),
            "{}",
            imported.notes.pending[0]
        );

        let user = arrangement(&imported, "DP-1")
            .instances()
            .find(|instance| instance.module == "user")
            .expect("the user reading is still placed")
            .clone();
        assert_eq!(
            user.options.get("show_avatar").and_then(|it| it.as_bool()),
            Some(false),
            "the key moved onto the instance it describes"
        );
    }

    #[test]
    fn an_excluded_screen_loses_the_bars_and_keeps_the_rest() {
        let config = parsed(
            r#"
            [bars]
            excluded_screens = ["HDMI-*"]
            [bars.top]
            start = ["clock"]
            "#,
        );
        let imported = importing(&config);
        let excluded = arrangement(&imported, "HDMI-A-1");
        assert!(
            excluded.layer(LayerKind::Top).is_none(),
            "the excluded screen has no bar"
        );
        assert!(
            excluded.layer(LayerKind::Overlay).is_some(),
            "but still takes the notifications"
        );
        assert_eq!(
            modules(&arrangement(&imported, "DP-1")),
            vec!["clock"],
            "and every other screen keeps its bar"
        );
    }

    #[test]
    fn a_monitor_override_says_only_what_that_screen_does_differently() {
        let config = parsed(
            r#"
            [bars.top]
            size = 34
            start = ["workspaces"]
            end = ["clock"]
            "#,
        );
        let laptop = parsed(
            r#"
            [bars.top]
            size = 48
            start = ["workspaces"]
            end = ["clock", "battery"]
            "#,
        );
        let imported = import_of(IMPORTED, &config, &[("eDP-1".to_string(), laptop)]);
        nothing_to_report(&imported);

        let rule = imported
            .layout
            .outputs
            .iter()
            .find(|rule| rule.matches.0 == "eDP-1")
            .expect("the laptop has a rule of its own");
        let area = &rule.layers.top.areas[0];
        assert_eq!(area.id.as_str(), "bar-top");
        assert_eq!(
            area.kind,
            Some(AreaKind::Bar {
                edge: None,
                thickness: Some(48.0),
                length: None,
                offset: None,
                shape: BarShape::default(),
                autohide: None,
            }),
            "the one number that differs, under the tag that says which kind of area it is"
        );
        assert!(
            area.reserve.is_none(),
            "and nothing it agrees with the global bar about"
        );
        assert_eq!(
            area.groups.len(),
            1,
            "only the zone that gained something is named again"
        );
        assert_eq!(area.groups[0].children.len(), 1);

        let laptop = arrangement(&imported, "eDP-1");
        let ResolvedAreaKind::Bar { thickness, .. } = bar(&laptop, "bar-top") else {
            panic!("still a bar");
        };
        assert_eq!(thickness, 48.0);
        assert_eq!(
            modules(&laptop),
            vec!["workspaces", "clock", "battery"],
            "the added chip follows the ones that screen already had"
        );

        let ResolvedAreaKind::Bar { thickness, .. } =
            bar(&arrangement(&imported, "DP-1"), "bar-top")
        else {
            panic!("still a bar");
        };
        assert_eq!(thickness, 34.0, "and every other screen is untouched");
    }

    #[test]
    fn a_monitor_that_reorders_its_chips_places_the_whole_run_again() {
        let config = parsed(
            r#"
            [bars.top]
            start = ["workspaces", "clock"]
            "#,
        );
        let other = parsed(
            r#"
            [bars.top]
            start = ["clock", "workspaces"]
            "#,
        );
        let imported = import_of(IMPORTED, &config, &[("DP-2".to_string(), other)]);
        assert_eq!(
            modules(&arrangement(&imported, "DP-2")),
            vec!["clock", "workspaces"],
            "a merge by id cannot reorder, so the rule takes the run away and places it again"
        );
        assert_eq!(
            modules(&arrangement(&imported, "DP-1")),
            vec!["workspaces", "clock"]
        );
    }

    #[test]
    fn a_monitor_that_only_insets_its_widgets_differently_says_that_and_nothing_else() {
        let config = parsed(
            r#"
            [widgets.clock]
            enabled = true
            margin = 48
            "#,
        );
        let roomier = parsed(
            r#"
            [widgets.clock]
            enabled = true
            margin = 120
            "#,
        );
        let imported = import_of(IMPORTED, &config, &[("DP-2".to_string(), roomier)]);
        let rule = imported
            .layout
            .outputs
            .iter()
            .find(|rule| rule.matches.0 == "DP-2")
            .expect("the screen has a rule of its own");
        assert_eq!(rule.layers.desktop.areas[0].style.padding, Some(120.0));
        assert!(
            rule.layers.desktop.areas[0].groups.is_empty(),
            "the clock inside it is untouched"
        );

        let resolved = arrangement(&imported, "DP-2");
        let ResolvedAreaKind::Grid { anchor, .. } = &resolved
            .layer(LayerKind::Desktop)
            .expect("a desktop layer")
            .areas[0]
            .kind
        else {
            panic!("still a grid");
        };
        assert_eq!(
            *anchor,
            Anchor::Center,
            "and it still follows the placement every screen has"
        );
    }

    #[test]
    fn a_monitor_that_says_nothing_about_the_layout_gets_no_rule() {
        let config = parsed(
            r#"
            [bars.top]
            start = ["clock"]
            "#,
        );
        let same = parsed(
            r#"
            [bars.top]
            start = ["clock"]
            [theme]
            accent = "red"
            "#,
        );
        let imported = import_of(IMPORTED, &config, &[("DP-2".to_string(), same)]);
        assert_eq!(
            imported.layout.outputs.len(),
            1,
            "a per-monitor theme is not a layout key"
        );
    }

    /// The shape of a config that has been lived in: a bar on every edge, each with a look of its own, one of them hiding itself, corner modules, desktop widgets, and a lock screen with everything switched on.
    #[test]
    fn a_config_with_every_placement_key_set_imports_with_nothing_left_behind() {
        let config = parsed(
            r#"
            [general]
            show_over_fullscreen = true

            [bars.top]
            size = 34
            start = ["workspaces", "notifications"]
            center = ["clock"]
            end = ["notifications", "battery", "tray"]
            [bars.top.shape]
            mode = "chips"
            radius = 14

            [bars.bottom]
            size = 28
            start = ["media"]
            end = ["battery"]
            persistent = false
            peek = 3
            [bars.bottom.shape]
            mode = "sections"

            [bars.left]
            size = 44
            start = ["logo"]
            end = ["session"]
            [bars.left.shape]
            gap = 6

            [bars.right]
            size = 44
            center = ["battery"]
            [bars.right.shape]
            spacing = 4

            [corners]
            top_left = "logo"
            bottom_right = "utilities"

            [widgets.clock]
            enabled = true
            position = "top_center"
            [widgets.visualiser]
            enabled = true
            edge = "bottom"

            [stack]
            edge = "top"
            align = "end"

            [lock]
            show_weather = true
            show_resources = true
            "#,
        );
        let imported = importing(&config);
        nothing_to_report(&imported);

        let resolved = arrangement(&imported, "DP-1");
        assert_eq!(
            resolved
                .layer(LayerKind::Top)
                .expect("four bars")
                .areas
                .iter()
                .map(|area| area.id.to_string())
                .collect::<Vec<_>>(),
            vec!["bar-top", "bar-bottom", "bar-left", "bar-right"],
            "one area per edge that carries a bar"
        );

        let ids: Vec<String> = resolved
            .instances()
            .map(|instance| instance.id.to_string())
            .collect();
        let each_once: std::collections::BTreeSet<&String> = ids.iter().collect();
        assert_eq!(
            ids.len(),
            each_once.len(),
            "every instance in the layout has an id of its own: {ids:?}"
        );
        assert!(
            ids.contains(&"notifications".to_string())
                && ids.contains(&"notifications-2".to_string()),
            "the module placed twice is two instances: {ids:?}"
        );

        assert!(resolved.layer(LayerKind::Desktop).is_some());
        assert!(resolved.layer(LayerKind::Overlay).is_some());
        assert_eq!(
            resolved
                .layer(LayerKind::Lock)
                .expect("a lock layer")
                .areas
                .last()
                .map(|area| area.id.to_string()),
            Some("prompt".to_string()),
            "with the prompt still last"
        );
    }

    /// The re-parse on the second line is load-bearing and has to stay above the key check: `check_unknown_keys` reports nothing at all about text it cannot parse, so asserting it is clean on a file that never parsed would pass for the wrong reason. Proved by seeding a mistyped key into the written text and watching the last assertion fire.
    #[test]
    fn the_layout_it_writes_is_one_it_can_read_back() {
        let imported = importing(&Config::starter());
        let text = toml::to_string_pretty(&imported.layout).expect("it serializes");
        let again: Layout = toml::from_str(&text).expect("and parses back");
        let (resolved, report) = resolve(&again, &BTreeMap::new(), "DP-1", None);
        assert!(report.is_clean(), "{}", report.render());
        assert_eq!(chips(&resolved).len(), 3);
        assert!(
            layout::check_unknown_keys(&text, &LayoutId::new(IMPORTED)).is_clean(),
            "and it writes no key the model does not have"
        );
    }

    #[test]
    fn a_name_that_could_not_be_a_file_of_its_own_is_refused() {
        assert_eq!(wanted_name(&[]), Ok(IMPORTED.to_string()));
        assert_eq!(wanted_name(&["--name", "mine"]), Ok("mine".to_string()));
        assert!(wanted_name(&["--name"]).is_err());
        assert!(wanted_name(&["--name", "a/b"]).is_err());
        assert!(wanted_name(&["--name", ""]).is_err());
        assert!(
            wanted_name(&["--name", layout::BUILT_IN]).is_err(),
            "the built-in layout is read-only"
        );
        assert!(
            wanted_name(&["mine"]).is_err(),
            "and the flag is not optional"
        );
    }

    #[test]
    fn the_reply_names_the_file_and_then_keeps_the_two_kinds_of_note_apart() {
        let written = Path::new("/somewhere/layouts/imported.toml");
        assert_eq!(
            reply(written, &Notes::default()),
            "/somewhere/layouts/imported.toml",
            "an import with nothing to say says only where it put the layout"
        );

        let notes = Notes {
            unmapped: vec!["a key with nowhere to go".to_string()],
            pending: vec![
                "a key nothing reads yet".to_string(),
                "and another".to_string(),
            ],
        };
        assert_eq!(
            reply(written, &notes)
                .lines()
                .skip(1)
                .collect::<Vec<&str>>(),
            vec![
                "not carried over:",
                "  a key with nowhere to go",
                "carried into the layout but not yet acted on:",
                "  a key nothing reads yet",
                "  and another",
            ],
            "one line each under the heading that says what to do about it"
        );
    }

    #[test]
    fn an_edge_with_no_thickness_carries_no_bar_at_all() {
        let mut config = Config::default();
        config.bars.top = BarConfig {
            size: 0,
            start: vec![ModuleEntry::bare("clock")],
            ..BarConfig::default()
        };
        assert!(
            importing(&config).layout.outputs[0]
                .layers
                .top
                .areas
                .is_empty(),
            "a bar with no thickness is not a bar"
        );
    }

    #[test]
    fn the_built_in_layout_is_listed_and_marked_read_only() {
        let listed = list();
        assert!(
            listed.lines().any(|line| line.starts_with("default\t")),
            "{listed}"
        );
        assert!(listed.contains("read-only"), "{listed}");
    }

    #[test]
    fn showing_a_layout_that_is_not_there_says_so_rather_than_printing_nothing() {
        let refused = show(Some("no-such-layout")).expect_err("it refuses");
        assert!(refused.contains("no-such-layout"), "{refused}");
    }

    #[test]
    fn showing_the_built_in_layout_prints_something_that_parses_back() {
        let printed = show(None).expect("it prints");
        let again: Layout = toml::from_str(&printed).expect("and it parses back");
        assert_eq!(again.outputs.len(), 1);
    }

    /// The check has to pass on the layout the shell falls back to, or the fallback is not one.
    ///
    /// **Deliberately without installing the module table**, which is the process this verb actually runs in: `ui::descriptor::install` happens in `setup_shell`, and a check answers in the CLI. Installing it here is what hid the bug — every module in a real user's layout came back "there is no module called `clock`", because the catalogue looked in an empty table while the binary's own table sat one argument away.
    #[test]
    fn the_built_in_layout_checks_clean_without_the_module_table_installed() {
        telar::set_locale("en");
        assert!(
            ui::descriptor::installed().is_empty(),
            "this test is only worth anything while nothing installed the table on this thread"
        );
        let verdict = check(None);
        assert!(verdict.is_ok(), "{}", verdict.unwrap_err());
    }

    /// `check` answers from the files rather than from the shell, so it must refuse a name that is not there instead of reporting a clean layout it never read.
    #[test]
    fn checking_a_layout_that_is_not_there_is_an_error_not_a_clean_report() {
        telar::set_locale("en");
        ui::descriptor::install(crate::core::modules::MODULES);
        let refused = check(Some("no-such-layout")).expect_err("it refuses");
        assert!(refused.contains("no-such-layout"), "{refused}");
    }
}
