//! What a layout is not allowed to say, and why.
//!
//! Resolution ([`crate::resolve`]) answers fields and reports the ones nobody filled. Validation is the other half: rules that a complete, well-typed layout can still break. They are all here rather than spread through the code that draws, because a rule enforced where it is drawn is a rule that is enforced differently in each of the five places something is drawn.
//!
//! Two of them are load-bearing:
//!
//! - **A workspace rule may not touch reservation.** Reservation is what keeps windows out of a bar's strip, and the compositor re-tiles every window when it changes. If switching workspaces could change it, every workspace switch would shuffle the user's windows. So a workspace rule that adds, removes or resizes a reserving area is rejected with its path, and the output-level arrangement stands.
//! - **The lock layer holds readings, never controls.** Anything placed there is on a screen that anyone walking past can see and touch, so only representations a module declares `ReadOnly` may go there, actions are refused outright, and a command source has to opt in. A layout that breaks any of it is not silently fixed: the lock falls back to the built-in minimal lock, because a half-corrected lock screen is worse than a plain one.
//!
//! What this crate cannot know on its own — whether a module exists, whether it has a representation, whether that representation is read-only, whether a command line resolves, and what an expression's names read — is asked of a [`Catalogue`]. That keeps the layout model free of the module registry, the IPC table and the shell's readings, and lets a test state exactly which modules and names it is talking about.
//!
//! **Expressions are checked where they are written.** Every `visible` and every binding is compiled against the names it may read on its layer and typed against what it drives: a `visible` gives a bool, a binding what its option takes. A finding is located at the expression's key, with the span inside the expression it is about; [`locate_expressions`] moves that span into the file for a caller that has the file's text. A binding that fails is left out where it is drawn and its instance keeps its written options, so one bad expression costs only itself.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;

use config::scheme;
use config::theme::NordTheme;
use telar_expression::{Compiled, ErrorCode, ErrorKind, Errors, HostError, Type, is_identifier};
use util::report::{Finding, Message, Report, Span};

use crate::library::{Library, komponent_path};
use crate::merge::{KeyAt, merge_layers, merge_session_layers};
use crate::model::*;
use crate::resolve::{Resolved, ResolvedArea, ResolvedAreaKind};

/// What validation has to ask someone else.
pub trait Catalogue {
    fn knows_module(&self, module: &str) -> bool;
    /// Whether the module can be drawn at this size at all.
    fn has_representation(&self, module: &str, representation: Representation) -> bool;
    /// Whether that representation registers no press, drag, scroll or hover target.
    fn is_read_only(&self, module: &str, representation: Representation) -> bool;
    /// Whether the line names a real IPC command, checked without running it.
    fn command_resolves(&self, line: &str) -> bool;
    /// What is wrong with an instance of `module` setting `options`, as `(key, why)`: a key the module does not declare, or a value of the wrong kind.
    fn option_problems(&self, module: &str, options: &toml::Table) -> Vec<(String, Message)>;
    /// Whether a module already gives a reading called `name`, which a layout's own source would shadow.
    fn is_service_source(&self, name: &str) -> bool;
    /// Parses and type-checks an expression against every name it may read. `on_lock` reads as whoever is in front of the lock screen: a private field is its type's empty value, and a source that runs a command is refused unless it says `lock_safe = true` (TA-8).
    fn compile(&self, source: &str, on_lock: bool) -> Result<Compiled, Errors> {
        self.compile_with(source, on_lock, &Locals::default())
    }
    /// [`Catalogue::compile`] for an expression that also reads `locals`: `$item` and `$index` in a copy of a repeated group's children. A local is read before any name of the shell's, inside the copy alone.
    fn compile_with(
        &self,
        source: &str,
        on_lock: bool,
        locals: &Locals,
    ) -> Result<Compiled, Errors>;
    /// Whether `error`, from compiling `source`, is only that a `$name` it reads names nothing yet: a variable, which the shell sets while it runs, so the expression is warned of rather than refused and comes alive once the variable is set. A catalogue that knows no variables waits for none.
    fn awaits_variable(&self, _source: &str, _error: &telar_expression::Error) -> bool {
        false
    }
    /// The type a binding at `path` on an instance of `module` has to give — an option `module` declares, or `accent` — or why nothing can be bound there.
    fn binding_type(&self, module: &str, path: &str) -> Result<Type, Message>;
    /// What an expression's failure says. A failure of a name this catalogue answers for is in the words of whoever answers for it; a catalogue that knows no such words says the expression language's own.
    fn describe(&self, code: &ErrorCode) -> Message {
        Message::expression(code)
    }
}

/// One thing wrong with an expression: which stage refused it, the bytes of it that are wrong, and what is.
#[derive(Clone, Debug, PartialEq)]
pub struct Mistake {
    pub kind: ErrorKind,
    pub span: telar_expression::Span,
    pub message: Message,
}

impl Mistake {
    /// What `error` is, in the words `catalogue` has for it.
    pub fn of(catalogue: &dyn Catalogue, error: &telar_expression::Error) -> Self {
        Self {
            kind: error.kind,
            span: error.span,
            message: catalogue.describe(&error.code),
        }
    }

    /// A mistake with `expr` as a whole rather than with a part of it.
    fn whole(expr: &Expr, message: Message) -> Self {
        Self {
            kind: ErrorKind::Type,
            span: telar_expression::Span::new(0, expr.0.len()),
            message,
        }
    }

    /// As the command line shows it: in English, over `source` with a caret under the part that is wrong.
    pub fn render(&self, source: &str) -> String {
        let said = ErrorCode::Host(HostError::new("", self.message.english()));
        telar_expression::Error::new(self.kind, self.span, said).render(source)
    }
}

/// Everything wrong with `layout` that does not depend on which output it is shown on.
pub fn validate(layout: &Layout, catalogue: &dyn Catalogue) -> Report {
    let mut report = Report::default();
    let file = format!("layouts/{}.toml", layout.id);
    crate::names::unreadable_in_layout(layout, &file, &mut report);

    let mut instances = BTreeMap::new();
    for rule in &layout.outputs {
        let at = format!("outputs.{}", rule.matches.0);
        check_layer_ids(&rule.layers, &at, &file, &mut instances, &mut report);
        check_lock_layer(&rule.layers.lock, &at, &file, catalogue, &mut report);
        check_modules(&rule.layers, &at, &file, catalogue, &mut report);
        check_actions(&rule.layers, &at, &file, catalogue, &mut report);
        let scope = output_level(layout, &rule.matches);
        let level = Level {
            layers: &rule.layers,
            scope: &scope,
            inherits: layout.extends.is_some(),
            at: &at,
            file: &file,
        };
        check_expressions(level, catalogue, Severity::Error, &mut report);
        check_unsets(level, catalogue, Severity::Error, &mut report);
        check_containers(level, &mut report);
        check_panels(level, &mut report);
        check_panel_reserve(&rule.layers, &scope, &at, &file, &mut report);
        check_gradients(&rule.layers, &at, &file, &mut report);
        check_wallpaper_regions(&rule.layers, &at, &file, &mut report);
        check_bar_corners(&rule.layers, &scope, &at, &file, &mut report);
        check_cell_areas(&rule.layers, &at, &file, &mut report);
        check_styles(&rule.layers, &at, &file, &mut report);
        check_stripless_bars(&rule.layers, &scope, &at, &file, &mut report);
        check_workspace_rules(layout, rule, &at, &file, catalogue, &mut report);
    }
    check_sources(layout, &file, catalogue, &mut report);
    report
}

/// What a level calls its sources. What each one says is checked once every level of the chain is laid over the others, where the shell reads them to run (`automation::sources::UserSources::of`): a key a level leaves out may come from the layout it extends.
fn check_sources(layout: &Layout, file: &str, catalogue: &dyn Catalogue, report: &mut Report) {
    for name in layout.sources.keys() {
        let at = format!("sources.{name}");
        if !is_identifier(name) {
            report.error(Finding::new(
                file,
                at.clone(),
                util::message!("finding.source_name", name = name),
            ));
        }
        if catalogue.is_service_source(name) {
            report.error(Finding::new(
                file,
                at,
                util::message!("finding.source_shadows", name = name),
            ));
        }
    }
}

/// Everything wrong with a layout's **lock layer alone**.
///
/// For the one caller that must not hear about a bar: the session opener decides between the layout's lock screen and the built-in minimal one, and a mistyped desktop widget is no reason to take away a lock screen the user configured. Every check it runs is one [`validate`] runs too — this is the same set narrowed to one layer, not a second opinion about it. One thing is said differently: an expression that does not check is a warning here, since it is left out where it is drawn, and a binding the lock cannot read is no reason to take the lock screen away.
pub fn validate_lock(layout: &Layout, catalogue: &dyn Catalogue) -> Report {
    let mut report = Report::default();
    let file = format!("layouts/{}.toml", layout.id);
    for rule in &layout.outputs {
        let at = format!("outputs.{}", rule.matches.0);
        let alone = Layers {
            lock: rule.layers.lock.clone(),
            ..Layers::default()
        };
        check_lock_layer(&rule.layers.lock, &at, &file, catalogue, &mut report);
        check_modules(&alone, &at, &file, catalogue, &mut report);
        check_actions(&alone, &at, &file, catalogue, &mut report);
        let scope = output_level(layout, &rule.matches);
        let level = Level {
            layers: &alone,
            scope: &scope,
            inherits: layout.extends.is_some(),
            at: &at,
            file: &file,
        };
        check_expressions(level, catalogue, Severity::Warning, &mut report);
        check_unsets(level, catalogue, Severity::Warning, &mut report);
        check_containers(level, &mut report);
        check_gradients(&alone, &at, &file, &mut report);
        check_styles(&alone, &at, &file, &mut report);
    }
    report
}

/// Everything wrong with one output's resolved arrangement. The lock layer's prompt lives here rather than in [`validate`] because "exactly one per output" is only answerable once an output is known.
///
/// `theme` is the one the lock would draw with, since whether the prompt's text can be read on its card depends on what its tokens are.
pub fn validate_resolved(resolved: &Resolved, file: &str, theme: &NordTheme) -> Report {
    let mut report = Report::default();
    let Some(lock) = resolved.layer(LayerKind::Lock) else {
        return report;
    };

    let prompts: Vec<(&ResolvedArea, &Rect)> = lock
        .areas
        .iter()
        .filter_map(|area| match &area.kind {
            ResolvedAreaKind::Prompt { rect } => Some((area, rect)),
            _ => None,
        })
        .collect();

    let at = format!("layers.lock on {}", resolved.output);
    let (prompt, rect) = match prompts.as_slice() {
        [prompt] => *prompt,
        [] => {
            report.error(Finding::new(
                file,
                at.clone(),
                util::message!("finding.no_prompt"),
            ));
            return report;
        }
        several => {
            report.error(Finding::new(
                file,
                at.clone(),
                util::message!("finding.prompts", count = several.len()),
            ));
            return report;
        }
    };

    if prompt.visible.is_some() {
        report.error(Finding::new(
            file,
            format!("{at}.areas.{}.visible", prompt.id),
            util::message!("finding.prompt_visible"),
        ));
    }
    if !rect.is_on_output() {
        report.error(Finding::new(
            file,
            format!("{at}.areas.{}.rect", prompt.id),
            util::message!("finding.prompt_off_output"),
        ));
    }
    if rect.w < SMALLEST_PROMPT || rect.h < SMALLEST_PROMPT {
        report.error(Finding::new(
            file,
            format!("{at}.areas.{}.rect", prompt.id),
            util::message!("finding.prompt_too_small"),
        ));
    }
    let held = format!("{at}.areas.{}", prompt.id);
    check_prompt_style(&prompt.style, &held, file, theme, &mut report);
    for group in &prompt.groups {
        let held = format!("{held}.groups.{}", group.id);
        check_prompt_style(&group.style, &held, file, theme, &mut report);
        for child in &group.children {
            let held = format!("{held}.children.{}", child.id);
            check_prompt_style(&child.style, &held, file, theme, &mut report);
            for path in child
                .bindings
                .keys()
                .filter(|path| StyleBinding::from_path(path).is_some())
            {
                report.error(Finding::new(
                    file,
                    format!("{held}.bindings.{path}"),
                    util::message!("finding.prompt_style_bound"),
                ));
            }
        }
    }
    report
}

/// The prompt, and every group and instance drawn inside it, stays opaque enough to find and its text readable on whatever card it paints.
fn check_prompt_style(style: &Style, at: &str, file: &str, theme: &NordTheme, report: &mut Report) {
    if let Some(opacity) = style.opacity
        && below(opacity, FAINTEST_PROMPT)
    {
        report.error(Finding::new(
            file,
            format!("{at}.style.opacity"),
            util::message!(
                "finding.prompt_faint",
                opacity = opacity,
                least = FAINTEST_PROMPT
            ),
        ));
    }
    // Only a fill the layout chose is judged: the theme's own surface is what the minimal lock draws too, so refusing it would fall back to the same card.
    if style.fill.is_some() {
        let ratio = prompt_contrast(style, theme);
        if !scheme::readable_ratio(ratio) {
            report.error(Finding::new(
                file,
                format!("{at}.style.fill"),
                util::message!(
                    "finding.prompt_contrast",
                    ratio = format!("{ratio:.1}"),
                    needed = scheme::MIN_TEXT_CONTRAST
                ),
            ));
        }
    }
}

/// What a table of `item` may hold: every key `item` has and the tag of `owner`, the enum the table names a variant of — an area's `kind`, a group's `place`, a source's `kind` — and beside them the keys of the variant it names, whose name as written comes third. Read off the model's own tables ([`crate::schema::keys_of`]), so what the reference lists and what this accepts are one list.
fn keys_in<'a>(
    table: &'a dyn toml_edit::TableLike,
    item: &str,
    owner: &str,
) -> (Vec<&'static str>, Vec<&'static str>, &'a str) {
    let tag = crate::schema::tag_of(owner);
    let named = tag
        .and_then(|tag| table.get(tag))
        .and_then(|it| it.as_str())
        .unwrap_or("");
    let mut common = crate::schema::keys_of(item);
    common.extend(tag);
    let variant = crate::schema::keys_of(&crate::schema::variant_of(owner, named));
    (common, variant, named)
}

/// Reports keys an area or a group does not have.
///
/// This reads the file's own text rather than the parsed model, because an area's geometry is flattened into its table and serde cannot both flatten and refuse unknown keys. Reading the text also means each finding carries a real line and column, so a mistyped `thikness` points at itself instead of at the area it belongs to.
pub fn check_unknown_keys(text: &str, id: &LayoutId) -> Report {
    let mut report = Report::default();
    let file = format!("layouts/{id}.toml");
    let Ok(parsed) = toml_edit::Document::parse(text) else {
        return report;
    };

    if let Some(sources) = parsed.get("sources").and_then(|it| it.as_table_like()) {
        for (name, source) in sources.iter() {
            let Some(source) = source.as_table_like() else {
                continue;
            };
            let (common, allowed, kind) = keys_in(source, "Source", "Source");
            report_strays(
                source,
                &common,
                &allowed,
                &format!("sources.{name}"),
                Holding::Source,
                kind,
                text,
                &file,
                &mut report,
            );
        }
    }

    let Some(outputs) = parsed.get("outputs").and_then(|it| it.as_array_of_tables()) else {
        return report;
    };

    for rule in outputs {
        let at = rule
            .get("match")
            .and_then(|it| it.as_str())
            .unwrap_or("*")
            .to_string();
        check_rule_keys(rule, &at, text, &file, &mut report);
        if let Some(workspaces) = rule
            .get("workspaces")
            .and_then(|it| it.as_array_of_tables())
        {
            for workspace in workspaces {
                let at = format!(
                    "{at}.workspaces.{}",
                    workspace
                        .get("match")
                        .and_then(|it| it.as_str())
                        .unwrap_or("")
                );
                check_rule_keys(workspace, &at, text, &file, &mut report);
            }
        }
    }
    report
}

fn check_rule_keys(rule: &toml_edit::Table, at: &str, text: &str, file: &str, report: &mut Report) {
    let Some(layers) = rule.get("layers").and_then(|it| it.as_table()) else {
        return;
    };
    for (layer, value) in layers.iter() {
        let Some(areas) = value
            .as_table()
            .and_then(|it| it.get("areas"))
            .and_then(|it| it.as_array_of_tables())
        else {
            continue;
        };
        for area in areas {
            let area_id = area.get("id").and_then(|it| it.as_str()).unwrap_or("");
            let (common, allowed, kind) = keys_in(area, "Area", "AreaKind");
            let at = format!("outputs.{at}.layers.{layer}.areas.{area_id}");
            report_strays(
                area,
                &common,
                &allowed,
                &at,
                Holding::Area,
                kind,
                text,
                file,
                report,
            );
            report_invalid_kind_values(area, &at, text, file, report);

            let Some(groups) = area.get("groups").and_then(|it| it.as_array_of_tables()) else {
                continue;
            };
            for group in groups {
                let group_id = group.get("id").and_then(|it| it.as_str()).unwrap_or("");
                let (common, allowed, place) = keys_in(group, "Group", "GroupKind");
                let at = format!("{at}.groups.{group_id}");
                report_strays(
                    group,
                    &common,
                    &allowed,
                    &at,
                    Holding::Group,
                    place,
                    text,
                    file,
                    report,
                );
            }
        }
    }
}

/// Reports each key of an area's kind whose value the kind refuses, and the `kind` itself where no kind has that name. The model reads an area's kind through a flattened, optional field, which turns any such refusal into "no kind", so the key and the value are found again here, one key at a time, against the kind's own deserializer.
fn report_invalid_kind_values(
    area: &toml_edit::Table,
    at: &str,
    text: &str,
    file: &str,
    report: &mut Report,
) {
    let Some(name) = area.get("kind").and_then(|it| it.as_str()) else {
        return;
    };
    let (_, allowed, _) = keys_in(area, "Area", "AreaKind");
    let mut refuse = |key: &str, item: &toml_edit::Item, why: &str| {
        let value = item
            .clone()
            .into_value()
            .map(|value| value.to_string())
            .unwrap_or_default();
        let mut finding = Finding::new(
            file,
            format!("{at}.{key}"),
            util::message!(
                "finding.invalid_value",
                key = key,
                value = value.trim(),
                holder = Holding::Area.named(name),
                why = why
            ),
        );
        finding.span = item
            .span()
            .map(|bytes| util::report::Span::locate(text, bytes));
        report.error(finding);
    };
    if let Err(error) = toml::from_str::<AreaKind>(&format!("kind = {name:?}")) {
        refuse("kind", &area["kind"], error.message());
        return;
    }
    for key in allowed {
        let Some(item) = area.get(key) else {
            continue;
        };
        let Ok(value) = item.clone().into_value() else {
            continue;
        };
        let alone = format!("kind = {name:?}\n{key} = {value}");
        if let Err(error) = toml::from_str::<AreaKind>(&alone) {
            refuse(key, item, error.message());
        }
    }
}

/// What a table of keys belongs to.
#[derive(Clone, Copy)]
enum Holding {
    Area,
    Group,
    Source,
}

impl Holding {
    /// What a sentence calls one, of `kind` where it says one.
    fn named(self, kind: &str) -> Message {
        match (self, kind.is_empty()) {
            (Holding::Area, true) => util::message!("finding.holder.area"),
            (Holding::Group, true) => util::message!("finding.holder.group"),
            (Holding::Source, true) => util::message!("finding.holder.source"),
            (Holding::Area, false) => util::message!("finding.holder.area_of_kind", kind = kind),
            (Holding::Group, false) => util::message!("finding.holder.group_of_kind", kind = kind),
            (Holding::Source, false) => {
                util::message!("finding.holder.source_of_kind", kind = kind)
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn report_strays(
    table: &dyn toml_edit::TableLike,
    common: &[&str],
    allowed: &[&str],
    at: &str,
    holding: Holding,
    kind: &str,
    text: &str,
    file: &str,
    report: &mut Report,
) {
    for (key, value) in table.iter() {
        if common.contains(&key) || allowed.contains(&key) {
            continue;
        }
        let retired = retired(key).filter(|_| matches!(holding, Holding::Group));
        let message = match retired {
            Some(now) => util::message!("finding.retired_key", key = key, now = now),
            None => util::message!(
                "finding.unknown_key",
                key = key,
                holder = holding.named(kind)
            ),
        };
        let mut finding = Finding::new(file, format!("{at}.{key}"), message);
        let written = match retired {
            Some(_) => table.key(key).and_then(toml_edit::Key::span),
            None => None,
        };
        finding.span = written
            .or_else(|| value.span())
            .map(|bytes| util::report::Span::locate(text, bytes));
        report.error(finding);
    }
}

/// Keys a group or a komponent held before 0.1.0, each with what is written in its place now: a file still writing one is told where it went rather than that nothing has heard of it.
const RETIRED: &[(&str, &str)] = &[("stacked", "arrange = \"pages\"")];

/// What is written in place of `key`, where it is a key a group or a komponent no longer holds.
fn retired(key: &str) -> Option<&'static str> {
    RETIRED
        .iter()
        .find(|(old, _)| *old == key)
        .map(|(_, now)| *now)
}

/// The key a komponent's file `text` writes that it no longer holds, located and naming what is written in its place, for a caller whose read of the file failed: the parser's own refusal says only that the key is unknown.
pub fn retired_in_komponent(text: &str, file: impl Into<std::path::PathBuf>) -> Option<Finding> {
    let parsed = toml_edit::Document::parse(text).ok()?;
    let (key, now) = parsed
        .iter()
        .find_map(|(key, _)| Some((key, retired(key)?)))?;
    let mut finding = Finding::new(
        file,
        key,
        util::message!("finding.retired_key", key = key, now = now),
    );
    finding.span = parsed
        .key(key)
        .and_then(toml_edit::Key::span)
        .map(|bytes| util::report::Span::locate(text, bytes));
    Some(finding)
}

/// How many colour stops a gradient may have. The renderer holds them in a fixed array of this length, so a ninth is not a subtlety that gets lost — it is a stop the user wrote and will never see.
const MOST_STOPS: usize = 8;

fn check_gradients(layers: &Layers, at: &str, file: &str, report: &mut Report) {
    for (kind, layer) in layers.each() {
        for area in &layer.areas {
            let Some(AreaKind::Texture {
                gradient: Some(gradient),
                ..
            }) = &area.kind
            else {
                continue;
            };
            if gradient.stops.len() > MOST_STOPS {
                report.error(Finding::new(
                    file,
                    format!("{at}.layers.{kind}.areas.{}.gradient.stops", area.id),
                    util::message!(
                        "finding.gradient_stops",
                        most = MOST_STOPS,
                        count = gradient.stops.len(),
                        extra = gradient.stops.len() - MOST_STOPS
                    ),
                ));
            }
        }
    }
}

/// A wallpaper region's focus, dim, blur and parallax past what they run to, each reported with what is drawn instead.
fn check_wallpaper_regions(layers: &Layers, at: &str, file: &str, report: &mut Report) {
    for (kind, layer) in layers.each() {
        for area in &layer.areas {
            let Some(AreaKind::WallpaperRegion {
                focus,
                dim,
                blur,
                parallax,
                ..
            }) = &area.kind
            else {
                continue;
            };
            let written = [
                ("focus.x", focus.map(|focus| focus.x), 1.0, Focus::MIDDLE.x),
                ("focus.y", focus.map(|focus| focus.y), 1.0, Focus::MIDDLE.y),
                ("dim", *dim, 1.0, 0.0),
                ("blur", *blur, AreaKind::MOST_BLUR, 0.0),
                ("parallax", *parallax, AreaKind::MOST_PARALLAX, 0.0),
            ];
            for (key, value, most, unset) in written {
                let Some(value) = value.filter(|value| !(0.0..=most).contains(value)) else {
                    continue;
                };
                let drawn = match value.is_nan() {
                    true => unset,
                    false => value.clamp(0.0, most),
                };
                report.warn(Finding::new(
                    file,
                    format!("{at}.layers.{kind}.areas.{}.{key}", area.id),
                    util::message!(
                        "finding.region_range",
                        key = key,
                        most = most,
                        value = value,
                        drawn = drawn
                    ),
                ));
            }
        }
    }
}

/// A bar is rounded by its `shape.radius`, which its chips nest their own corners inside; a `style.radius` beside it would be a second answer for the same pixels, so it is reported rather than drawn. Its `shape.fillet` curves the usable area out of the bar's one strip, so it is reported on a bar in `sections` or `chips` mode, judged on the bar as this level and the broader ones of the same file merge it, and where it is negative.
fn check_bar_corners(layers: &Layers, scope: &Layers, at: &str, file: &str, report: &mut Report) {
    for (kind, layer) in layers.each() {
        let merged = scope.get(kind);
        for area in &layer.areas {
            let path = format!("{at}.layers.{kind}.areas.{}", area.id);
            if matches!(area.kind, Some(AreaKind::Bar { .. })) && area.style.radius.is_some() {
                report.warn(Finding::new(
                    file,
                    format!("{path}.style.radius"),
                    util::message!("finding.bar_radius"),
                ));
            }
            let Some(AreaKind::Bar { shape: written, .. }) = &area.kind else {
                continue;
            };
            if let Some(fillet) = written.fillet
                && negative(fillet)
            {
                report.error(Finding::new(
                    file,
                    format!("{path}.fillet"),
                    util::message!("finding.fillet_negative", fillet = fillet),
                ));
            }
            let Some(AreaKind::Bar { shape: held, .. }) = merged
                .areas
                .iter()
                .find(|held| held.id == area.id)
                .and_then(|held| held.kind.as_ref())
            else {
                continue;
            };
            let stripless = matches!(
                held.mode,
                Some(config::Shape::Sections | config::Shape::Chips)
            );
            if held.fillet.is_some()
                && stripless
                && (written.fillet.is_some() || written.mode.is_some())
            {
                report.warn(Finding::new(
                    file,
                    format!("{path}.fillet"),
                    util::message!("finding.fillet_mode"),
                ));
            }
        }
    }
}

/// What a grid or a panel says about its cells that cannot be drawn: a cell not above 0, a negative gap, and for a panel no columns or rows.
fn check_cell_areas(layers: &Layers, at: &str, file: &str, report: &mut Report) {
    for (kind, layer) in layers.each() {
        for area in &layer.areas {
            let (tracks, cell) = match &area.kind {
                Some(AreaKind::Grid { cell, gap, .. }) => ((None, None, *gap), *cell),
                Some(AreaKind::Panel {
                    cols,
                    rows,
                    gap,
                    cell,
                    ..
                }) => ((*cols, *rows, *gap), *cell),
                _ => continue,
            };
            let under = |key: &str| format!("{at}.layers.{kind}.areas.{}.{key}", area.id);
            check_tracks(tracks, under, file, report);
            check_cell(cell, under, file, report);
        }
    }
}

/// Whether `x` is below `least`, or no number at all: a NaN compares false against everything, so a plain `<` would let it through.
fn below(x: f32, least: f32) -> bool {
    x.is_nan() || x < least
}

/// Whether `x` is below 0 or NaN, which no width, gap or fillet can be.
fn negative(x: f32) -> bool {
    below(x, 0.0)
}

/// Whether `x` is 0, below it or NaN, which no cell or weight can be.
fn not_positive(x: f32) -> bool {
    x.is_nan() || x <= 0.0
}

/// What holds a [`Style`], which decides the keys that mean something on it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Styled {
    Area,
    Group,
    Instance,
}

fn check_styles(layers: &Layers, at: &str, file: &str, report: &mut Report) {
    for (kind, layer) in layers.each() {
        for area in &layer.areas {
            let path = format!("{at}.layers.{kind}.areas.{}", area.id);
            check_style(&area.style, Styled::Area, &path, file, report);
            for group in &area.groups {
                let path = format!("{path}.groups.{}", group.id);
                check_style(&group.style, Styled::Group, &path, file, report);
                for instance in &group.children {
                    let path = format!("{path}.children.{}", instance.id);
                    check_style(&instance.style, Styled::Instance, &path, file, report);
                }
            }
        }
    }
}

/// What a style written at `at` says that its holder cannot draw: a backdrop off an area, padding on an instance, a shadow past the deepest step, a negative width, or an opacity outside 0 to 1.
fn check_style(style: &Style, holder: Styled, at: &str, file: &str, report: &mut Report) {
    if style.backdrop.is_some() && holder != Styled::Area {
        report.error(Finding::new(
            file,
            format!("{at}.style.backdrop"),
            util::message!("finding.style_backdrop"),
        ));
    }
    if let Some(padding) = style.padding {
        let sides = [
            padding.top(),
            padding.right(),
            padding.bottom(),
            padding.left(),
        ];
        if holder == Styled::Instance {
            report.error(Finding::new(
                file,
                format!("{at}.style.padding"),
                util::message!("finding.style_padding"),
            ));
        } else if let Some(width) = sides.into_iter().find(|side| negative(*side)) {
            report.error(Finding::new(
                file,
                format!("{at}.style.padding"),
                util::message!("finding.style_negative", width = width),
            ));
        }
    }
    if let Some(opacity) = style.opacity
        && !(0.0..=1.0).contains(&opacity)
    {
        report.error(Finding::new(
            file,
            format!("{at}.style.opacity"),
            util::message!("finding.style_opacity", opacity = opacity),
        ));
    }
    if let Some(width) = style.border.as_ref().and_then(|border| border.width)
        && negative(width)
    {
        report.error(Finding::new(
            file,
            format!("{at}.style.border.width"),
            util::message!("finding.style_negative", width = width),
        ));
    }
    if let Some(shadow) = style.shadow
        && shadow > Style::DEEPEST_SHADOW
    {
        report.error(Finding::new(
            file,
            format!("{at}.style.shadow"),
            util::message!(
                "finding.style_shadow",
                shadow = shadow,
                deepest = Style::DEEPEST_SHADOW
            ),
        ));
    }
}

/// A bar in `sections` or `chips` mode paints no strip, so a border or a shadow on the bar would have no box to go around: each section or chip carries its own. Judged on the bar as this level and the broader ones of the same file merge it, and reported at the level that writes either half. A mode the bar leaves to `[shape]` in the config is not seen here and is judged where the bar is drawn.
fn check_stripless_bars(
    layers: &Layers,
    scope: &Layers,
    at: &str,
    file: &str,
    report: &mut Report,
) {
    for (kind, layer) in layers.each() {
        let merged = scope.get(kind);
        for area in &layer.areas {
            let Some(held) = merged.areas.iter().find(|held| held.id == area.id) else {
                continue;
            };
            let stripless = matches!(
                &held.kind,
                Some(AreaKind::Bar { shape, .. })
                    if matches!(shape.mode, Some(config::Shape::Sections | config::Shape::Chips))
            );
            if !stripless {
                continue;
            }
            let writes_mode = matches!(
                &area.kind,
                Some(AreaKind::Bar { shape, .. }) if shape.mode.is_some()
            );
            let path = format!("{at}.layers.{kind}.areas.{}.style", area.id);
            for (key, merged_has, written) in [
                (
                    "border",
                    held.style.border.is_some(),
                    area.style.border.is_some(),
                ),
                (
                    "shadow",
                    held.style.shadow.is_some(),
                    area.style.shadow.is_some(),
                ),
            ] {
                if merged_has && (written || writes_mode) {
                    report.warn(Finding::new(
                        file,
                        format!("{path}.{key}"),
                        util::message!("finding.stripless_edge", key = key),
                    ));
                }
            }
        }
    }
}

/// What a group's `arrange` and its children's places say that cannot be drawn: an arrangement a zone does not take, children repeated where each has a place of its own, an inner grid with no tracks, a negative gap, `cols`, `rows` or `gap` its arrangement does not read, and a child's `weight`, `cell` or `rect` where its group does not arrange children that way. Judged on the group as this level and the broader ones of the same file merge it, and reported at the level that writes either half. A group drawing a komponent is the komponent's to judge, and one this file never places, in a layout that extends another, may be arranged there.
fn check_containers(level: Level<'_>, report: &mut Report) {
    let Level {
        layers,
        scope,
        inherits,
        at,
        file,
    } = level;
    for (kind, layer) in layers.each() {
        for area in &layer.areas {
            for group in &area.groups {
                let at = format!("{at}.layers.{kind}.areas.{}.groups.{}", area.id, group.id);
                let merged = merged_group(scope, kind, &area.id, &group.id).unwrap_or(group);
                if merged.komponent.is_some() {
                    continue;
                }
                let takes_back = group.unset.contains(&Unset::Arrange);
                let unknown =
                    merged.arrange.is_none() && merged.kind.is_none() && inherits && !takes_back;
                let writes_arrange = group.arrange.is_some() || takes_back;
                let key = |written: bool, key: &str, otherwise: &str| match written {
                    true => format!("{at}.{key}"),
                    false => format!("{at}.{otherwise}"),
                };
                if let (Some(arrange), Some(GroupKind::Zone { .. })) = (merged.arrange, merged.kind)
                    && arrange != Arrange::Pages
                    && (writes_arrange || group.kind.is_some())
                {
                    report.error(Finding::new(
                        file,
                        key(writes_arrange, "arrange", "place"),
                        util::message!("finding.zone_arrange", arrange = arrange.as_str()),
                    ));
                }
                let on_cell = matches!(merged.kind, Some(GroupKind::Cell { .. }));
                if let Some(arrange) = merged.arrange.filter(|arrange| places_each(*arrange))
                    && merged.repeat.is_some()
                    && !on_cell
                    && (writes_arrange || group.repeat.is_some())
                {
                    report.error(Finding::new(
                        file,
                        key(group.repeat.is_some(), "repeat", "arrange"),
                        util::message!("finding.arranged_repeats", arrange = arrange.as_str()),
                    ));
                }
                let under = |key: &str| format!("{at}.{key}");
                check_tracks((group.cols, group.rows, group.gap), under, file, report);
                let inherited = inherits && merged.arrange.is_none();
                if !takes_back && !inherited {
                    check_unarranged(
                        [
                            (merged.cols.is_some(), group.cols.is_some()),
                            (merged.rows.is_some(), group.rows.is_some()),
                            (merged.gap.is_some(), group.gap.is_some()),
                        ],
                        (merged.arrange, writes_arrange),
                        under,
                        file,
                        report,
                    );
                }
                for child in merged.children.iter().filter(|_| !unknown) {
                    let written = group.children.iter().find(|held| held.id == child.id);
                    if written.is_none() && !writes_arrange {
                        continue;
                    }
                    let at = format!("{at}.children.{}", child.id);
                    check_child_place(
                        (child, written),
                        (merged.arrange, writes_arrange),
                        &at,
                        file,
                        report,
                    );
                }
            }
        }
    }
}

/// What a panel says about its owner that cannot be drawn: an owner that is not an instance on the panel's layer, one that sits in a panel itself, `along = true` for an owner outside a bar, and a second panel for one owner. Judged on the panel and its owner as this level and the broader ones of the same file merge them, and reported where this level writes the panel's geometry. An owner this file never places, in a layout that extends another, may be placed there. No komponent file is read here, so an owner under a komponent's use is taken as written and [`validate_komponents`] checks it against the komponent. The lock layer has no panels at all ([`check_lock_layer`]).
fn check_panels(level: Level<'_>, report: &mut Report) {
    let Level {
        layers,
        scope,
        inherits,
        at,
        file,
    } = level;
    for (kind, layer) in layers.each() {
        if kind == LayerKind::Lock {
            continue;
        }
        let merged = scope.get(kind);
        for area in &layer.areas {
            let Some(AreaKind::Panel {
                owner: writes_owner,
                along: writes_along,
                ..
            }) = &area.kind
            else {
                continue;
            };
            let held = merged
                .areas
                .iter()
                .find(|held| held.id == area.id)
                .unwrap_or(area);
            let Some(AreaKind::Panel {
                owner: Some(owner),
                along,
                ..
            }) = &held.kind
            else {
                continue;
            };
            let path = format!("{at}.layers.{kind}.areas.{}", area.id);
            let key = |written: bool, key: &str| match written {
                true => format!("{path}.{key}"),
                false => format!("{path}.kind"),
            };
            let owner_key = key(writes_owner.is_some(), "owner");
            match merged.areas.iter().find(|holder| holder.places(owner)) {
                None if inherits => {}
                None => report.error(Finding::new(
                    file,
                    owner_key.clone(),
                    util::message!("finding.panel_owner_missing", owner = owner, layer = kind),
                )),
                Some(holder) if matches!(holder.kind, Some(AreaKind::Panel { .. })) => report
                    .error(Finding::new(
                        file,
                        owner_key.clone(),
                        util::message!(
                            "finding.panel_owner_in_panel",
                            owner = owner,
                            panel = &holder.id
                        ),
                    )),
                Some(holder) => {
                    if *along == Some(true) && !matches!(holder.kind, Some(AreaKind::Bar { .. })) {
                        report.error(Finding::new(
                            file,
                            key(writes_along.is_some(), "along"),
                            util::message!("finding.panel_along", owner = owner),
                        ));
                    }
                }
            }
            let first = merged
                .areas
                .iter()
                .find(|other| other.kind.as_ref().and_then(AreaKind::owner) == Some(owner));
            if let Some(first) = first.filter(|first| first.id != area.id) {
                report.error(Finding::new(
                    file,
                    owner_key,
                    util::message!(
                        "finding.panel_owner_taken",
                        owner = owner,
                        panel = &first.id
                    ),
                ));
            }
        }
    }
}

/// A panel opens over what is there, so one that reserves its edge is reported where this level writes its `reserve`, or makes a reserving area a panel.
fn check_panel_reserve(layers: &Layers, scope: &Layers, at: &str, file: &str, report: &mut Report) {
    for (kind, layer) in layers.each() {
        for area in &layer.areas {
            let Some(held) = scope.get(kind).areas.iter().find(|held| held.id == area.id) else {
                continue;
            };
            let writes =
                area.reserve == Some(true) || matches!(area.kind, Some(AreaKind::Panel { .. }));
            if matches!(held.kind, Some(AreaKind::Panel { .. }))
                && held.reserve == Some(true)
                && writes
            {
                let key = match area.reserve {
                    Some(_) => "reserve",
                    None => "kind",
                };
                report.error(Finding::new(
                    file,
                    format!("{at}.layers.{kind}.areas.{}.{key}", area.id),
                    util::message!("finding.panel_reserve"),
                ));
            }
        }
    }
}

/// Whether each child of a group arranged as `arrange` has a place of its own, which a copy of a repeated child could not.
fn places_each(arrange: Arrange) -> bool {
    matches!(arrange, Arrange::Grid | Arrange::Free)
}

/// What a group, a komponent or a panel says about its cells and their gap that cannot be drawn: no columns or rows, or a negative gap. `at` names each key by where it is written: under the group or the area, or alone at the top of a komponent's file.
fn check_tracks(
    (cols, rows, gap): (Option<u32>, Option<u32>, Option<f32>),
    at: impl Fn(&str) -> String,
    file: &str,
    report: &mut Report,
) {
    for (key, tracks) in [("cols", cols), ("rows", rows)] {
        if tracks == Some(0) {
            report.error(Finding::new(
                file,
                at(key),
                util::message!("finding.no_tracks", key = key),
            ));
        }
    }
    check_gap(gap, at, file, report);
}

fn check_cell(cell: Option<f32>, at: impl Fn(&str) -> String, file: &str, report: &mut Report) {
    if let Some(cell) = cell
        && not_positive(cell)
    {
        report.error(Finding::new(
            file,
            at("cell"),
            util::message!("finding.cell_not_positive", cell = cell),
        ));
    }
}

fn check_gap(gap: Option<f32>, at: impl Fn(&str) -> String, file: &str, report: &mut Report) {
    if let Some(gap) = gap
        && negative(gap)
    {
        report.error(Finding::new(
            file,
            at("gap"),
            util::message!("finding.gap_negative", gap = gap),
        ));
    }
}

/// What a group or a komponent writes of an arrangement it does not have: `cols` or `rows` off a `grid`, and `gap` where nothing is spaced, in a loose run or one page at a time. Each key is `(held, written)`: whether the merged group has it and whether the level being checked writes it, in the order `cols`, `rows`, `gap`. As in [`check_child_place`], a key its arrangement ignores is reported at a level that writes either half.
fn check_unarranged(
    [cols, rows, gap]: [(bool, bool); 3],
    (arrange, writes_arrange): (Option<Arrange>, bool),
    at: impl Fn(&str) -> String,
    file: &str,
    report: &mut Report,
) {
    let grid = arrange == Some(Arrange::Grid);
    let spaced = arrange.is_some_and(|arrange| arrange != Arrange::Pages);
    for (key, (held, written), read, message) in [
        (
            "cols",
            cols,
            grid,
            util::message!("finding.tracks_off_grid", key = "cols"),
        ),
        (
            "rows",
            rows,
            grid,
            util::message!("finding.tracks_off_grid", key = "rows"),
        ),
        ("gap", gap, spaced, util::message!("finding.gap_unspaced")),
    ] {
        if held && !read && (writes_arrange || written) {
            report.error(Finding::new(file, at(key), message));
        }
    }
}

/// What `child`, in a group arranged as `arrange`, says about its place that cannot be drawn: a weight not above 0, or a `weight`, `cell` or `rect` its group does not place children by. `written` is the child as the level being checked writes it, if it does, and `writes_arrange` whether that level writes the group's `arrange`: a place its group does not take is reported at a level that writes either half.
fn check_child_place(
    (child, written): (&Instance, Option<&Instance>),
    (arrange, writes_arrange): (Option<Arrange>, bool),
    at: &str,
    file: &str,
    report: &mut Report,
) {
    if let Some(weight) = written.and_then(|written| written.weight)
        && not_positive(weight)
    {
        report.error(Finding::new(
            file,
            format!("{at}.weight"),
            util::message!("finding.weight_not_positive", weight = weight),
        ));
    } else if let Some(weight) = written.and_then(|written| written.weight)
        && !(Instance::WEIGHTS.0..=Instance::WEIGHTS.1).contains(&weight)
    {
        report.warn(Finding::new(
            file,
            format!("{at}.weight"),
            util::message!(
                "finding.weight_out_of_range",
                weight = weight,
                least = Instance::WEIGHTS.0,
                most = Instance::WEIGHTS.1
            ),
        ));
    }
    let flows = matches!(arrange, Some(Arrange::Row | Arrange::Column));
    let misplaced = |has: fn(&Instance) -> bool, takes: bool| {
        has(child) && !takes && (writes_arrange || written.is_some_and(has))
    };
    for (key, misplaced, message) in [
        (
            "weight",
            misplaced(|it| it.weight.is_some(), flows),
            util::message!("finding.child_weight"),
        ),
        (
            "cell",
            misplaced(|it| it.cell.is_some(), arrange == Some(Arrange::Grid)),
            util::message!("finding.child_cell"),
        ),
        (
            "rect",
            misplaced(|it| it.rect.is_some(), arrange == Some(Arrange::Free)),
            util::message!("finding.child_rect"),
        ),
    ] {
        if misplaced {
            report.error(Finding::new(file, format!("{at}.{key}"), message));
        }
    }
}

fn check_layer_ids(
    layers: &Layers,
    at: &str,
    file: &str,
    instances: &mut BTreeMap<InstanceId, (LayerKind, AreaId, GroupId)>,
    report: &mut Report,
) {
    let mut named = BTreeSet::new();
    for (kind, layer) in layers.each() {
        let mut areas = BTreeSet::new();
        for area in &layer.areas {
            if !areas.insert(area.id.clone()) {
                report.error(Finding::new(
                    file,
                    format!("{at}.layers.{kind}.areas.{}", area.id),
                    util::message!("finding.shared_area_id"),
                ));
            }
            let mut groups = BTreeSet::new();
            for group in &area.groups {
                if !groups.insert(group.id.clone()) {
                    report.error(Finding::new(
                        file,
                        format!("{at}.layers.{kind}.areas.{}.groups.{}", area.id, group.id),
                        util::message!("finding.shared_group_id"),
                    ));
                }
                if group.komponent.is_some() {
                    let address =
                        InstanceId::new(InstanceId::komponent_prefix(&area.id, &group.id));
                    let place = (kind, area.id.clone(), group.id.clone());
                    if *instances.entry(address).or_insert_with(|| place.clone()) != place {
                        report.error(Finding::new(
                            file,
                            format!(
                                "{at}.layers.{kind}.areas.{}.groups.{}.komponent",
                                area.id, group.id
                            ),
                            util::message!(
                                "finding.shared_komponent_address",
                                address = format!("{}.{}", area.id, group.id)
                            ),
                        ));
                    }
                }
                for instance in &group.children {
                    let place = (kind, area.id.clone(), group.id.clone());
                    // A later rule naming the instance where an earlier one placed it refines it; anywhere else it is a second instance under the same id.
                    let elsewhere = *instances
                        .entry(instance.id.clone())
                        .or_insert_with(|| place.clone())
                        != place;
                    if !named.insert(instance.id.clone()) || elsewhere {
                        report.error(Finding::new(
                            file,
                            format!(
                                "{at}.layers.{kind}.areas.{}.groups.{}.children.{}",
                                area.id, group.id, instance.id
                            ),
                            util::message!("finding.shared_instance_id"),
                        ));
                    }
                }
            }
        }
    }
}

fn check_modules(
    layers: &Layers,
    at: &str,
    file: &str,
    catalogue: &dyn Catalogue,
    report: &mut Report,
) {
    for (kind, layer) in layers.each() {
        for (area, group, instance) in instances_of(layer) {
            let Some(module) = &instance.module else {
                continue;
            };
            let path = format!(
                "{at}.layers.{kind}.areas.{}.groups.{}.children.{}",
                area.id, group.id, instance.id
            );
            if !catalogue.knows_module(module) {
                report.error(Finding::new(
                    file,
                    format!("{path}.module"),
                    util::message!("finding.unknown_module", module = module),
                ));
                continue;
            }
            let representation = instance.representation.unwrap_or(Representation::Chip);
            if !catalogue.has_representation(module, representation) {
                report.error(Finding::new(
                    file,
                    format!("{path}.representation"),
                    util::message!(
                        "finding.no_representation",
                        module = module,
                        representation = representation.as_str()
                    ),
                ));
            }
            for (key, why) in catalogue.option_problems(module, &instance.options) {
                report.error(Finding::new(
                    file,
                    format!("{path}.options.{key}"),
                    util::message!("finding.option", module = module, key = key, why = why),
                ));
            }
        }
    }
}

fn check_actions(
    layers: &Layers,
    at: &str,
    file: &str,
    catalogue: &dyn Catalogue,
    report: &mut Report,
) {
    for (kind, layer) in layers.each() {
        for area in &layer.areas {
            let path = format!("{at}.layers.{kind}.areas.{}", area.id);
            check_chains(&area.actions, &path, file, catalogue, report);
        }
        for (area, group, instance) in instances_of(layer) {
            let path = format!(
                "{at}.layers.{kind}.areas.{}.groups.{}.children.{}",
                area.id, group.id, instance.id
            );
            check_chains(&instance.actions, &path, file, catalogue, report);
        }
    }
}

fn check_chains(
    actions: &BTreeMap<Trigger, Action>,
    at: &str,
    file: &str,
    catalogue: &dyn Catalogue,
    report: &mut Report,
) {
    for (trigger, action) in actions {
        for line in &action.0 {
            let key = || format!("{at}.actions.{}", trigger.as_str());
            if !catalogue.command_resolves(line) {
                report.error(Finding::new(
                    file,
                    key(),
                    util::message!("finding.unknown_command", line = line),
                ));
            } else if crate::trust::grants_trust(line) {
                report.error(Finding::new(
                    file,
                    key(),
                    util::message!("finding.trust_in_action", line = line),
                ));
            }
        }
    }
}

/// What a copy of a repeated group's children reads as `$item`: an element of the list.
pub const ITEM: &str = "item";
/// What a copy of a repeated group's children reads as `$index`: its place in the list, from 0.
pub const INDEX: &str = "index";

/// The names an expression reads besides the shell's own, by type: `$item` and `$index` in a copy of a repeated group's children, a komponent's parameters in what the komponent holds, and nothing anywhere else.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Locals(Vec<(String, Type)>);

impl Locals {
    /// What a copy of a repeated group's children reads: `$item`, an element of the list, and `$index`, a number.
    pub fn of_copy(item: Type) -> Self {
        Self(vec![
            (ITEM.to_string(), item),
            (INDEX.to_string(), Type::Number),
        ])
    }

    /// What the expressions a komponent holds read: each of its parameters, as the type it declares.
    pub fn of_parameters(komponent: &Komponent) -> Self {
        Self(
            komponent
                .parameters
                .iter()
                .map(|(name, parameter)| (name.clone(), parameter.ty.0.clone()))
                .collect(),
        )
    }

    /// These names and `more` besides.
    pub fn and(mut self, more: Locals) -> Self {
        self.0.extend(more.0);
        self
    }

    pub fn get(&self, name: &str) -> Option<&Type> {
        self.0
            .iter()
            .find(|(held, _)| held == name)
            .map(|(_, ty)| ty)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &Type)> {
        self.0.iter().map(|(name, ty)| (name.as_str(), ty))
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// Everything wrong with `expr` as the binding `path` of an instance of `module`, on the lock layer when `on_lock`: what keeps it from compiling, and a type that does not fit what it drives. `module` is `None` where the level being checked does not say which module the instance shows, and then only the expression's own mistakes can be found. A variable it reads that is not set yet is not wrong ([`Catalogue::awaits_variable`]).
pub fn binding_errors(
    catalogue: &dyn Catalogue,
    module: Option<&str>,
    path: &str,
    expr: &Expr,
    on_lock: bool,
) -> Vec<Mistake> {
    binding_errors_with(catalogue, module, path, expr, on_lock, &Locals::default())
}

/// [`binding_errors`] for an instance that also reads `locals` — a child of a repeated group, whose locals [`repeat_locals`] answers.
pub fn binding_errors_with(
    catalogue: &dyn Catalogue,
    module: Option<&str>,
    path: &str,
    expr: &Expr,
    on_lock: bool,
    locals: &Locals,
) -> Vec<Mistake> {
    binding_problems(catalogue, module, path, expr, on_lock, locals).errors
}

fn binding_problems(
    catalogue: &dyn Catalogue,
    module: Option<&str>,
    path: &str,
    expr: &Expr,
    on_lock: bool,
    locals: &Locals,
) -> Problems {
    let expected = match module.filter(|module| catalogue.knows_module(module)) {
        Some(module) => match catalogue.binding_type(module, path) {
            Ok(ty) => Some(ty),
            Err(why) => {
                return Problems {
                    errors: vec![Mistake::whole(expr, why)],
                    awaiting: Vec::new(),
                };
            }
        },
        None => None,
    };
    typed_problems(catalogue, expr, expected.as_ref(), on_lock, locals)
}

/// Everything wrong with `expr` as an area's `visible`, which has to give a bool.
pub fn visible_errors(catalogue: &dyn Catalogue, expr: &Expr, on_lock: bool) -> Vec<Mistake> {
    visible_problems(catalogue, expr, on_lock).errors
}

fn visible_problems(catalogue: &dyn Catalogue, expr: &Expr, on_lock: bool) -> Problems {
    typed_problems(
        catalogue,
        expr,
        Some(&Type::Bool),
        on_lock,
        &Locals::default(),
    )
}

fn typed_problems(
    catalogue: &dyn Catalogue,
    expr: &Expr,
    expected: Option<&Type>,
    on_lock: bool,
    locals: &Locals,
) -> Problems {
    Problems::of(
        catalogue,
        expr,
        catalogue.compile_with(&expr.0, on_lock, locals),
        |compiled| match expected {
            Some(expected) => compiled
                .require(expected)
                .map(drop)
                .map_err(|error| Mistake::of(catalogue, &error)),
            None => Ok(()),
        },
    )
}

/// Everything wrong with `expr` as a group's `repeat`, which has to give a list.
pub fn repeat_errors(catalogue: &dyn Catalogue, expr: &Expr, on_lock: bool) -> Vec<Mistake> {
    repeat_problems(catalogue, expr, on_lock).errors
}

fn repeat_problems(catalogue: &dyn Catalogue, expr: &Expr, on_lock: bool) -> Problems {
    repeat_problems_with(catalogue, expr, on_lock, &Locals::default())
}

fn repeat_problems_with(
    catalogue: &dyn Catalogue,
    expr: &Expr,
    on_lock: bool,
    locals: &Locals,
) -> Problems {
    Problems::of(
        catalogue,
        expr,
        catalogue.compile_with(&expr.0, on_lock, locals),
        |compiled| {
            repeat_item(compiled.ty())
                .map(drop)
                .map_err(|why| Mistake::whole(expr, why))
        },
    )
}

/// The type of what a group repeated over a list of `ty` reads as `$item`, or why `ty` is no list to repeat over. A type that fits anything fits a list.
pub fn repeat_item(ty: &Type) -> Result<Type, Message> {
    match ty {
        Type::List(item) => Ok((**item).clone()),
        Type::Never => Ok(Type::Never),
        other => Err(util::message!(
            "finding.repeat_needs_list",
            found = Message::type_name(other)
        )),
    }
}

/// What the copies of a group repeated over `repeat` read: `$item` of the type of the list's elements. A `repeat` that gives no list leaves `$item` fitting anything, so its mistake is reported once, at `repeat`, rather than again by every binding that reads an item.
pub fn repeat_locals(catalogue: &dyn Catalogue, repeat: &Expr, on_lock: bool) -> Locals {
    repeat_locals_with(catalogue, repeat, on_lock, &Locals::default())
}

/// [`repeat_locals`] for a `repeat` that reads `locals` itself: a komponent's, which reads its parameters.
fn repeat_locals_with(
    catalogue: &dyn Catalogue,
    repeat: &Expr,
    on_lock: bool,
    locals: &Locals,
) -> Locals {
    Locals::of_copy(
        catalogue
            .compile_with(&repeat.0, on_lock, locals)
            .ok()
            .and_then(|compiled| repeat_item(compiled.ty()).ok())
            .unwrap_or(Type::Never),
    )
}

/// What is wrong with one expression: what keeps it from being drawn, and each variable it reads that is not set yet, which a layout may name before the shell sets it.
struct Problems {
    errors: Vec<Mistake>,
    awaiting: Vec<Mistake>,
}

impl Problems {
    /// What `compiled`, `expr` compiled, gets wrong, where a compiled expression has to pass `fits` as well.
    fn of(
        catalogue: &dyn Catalogue,
        expr: &Expr,
        compiled: Result<Compiled, Errors>,
        fits: impl FnOnce(Compiled) -> Result<(), Mistake>,
    ) -> Self {
        match compiled {
            Ok(compiled) => Self {
                errors: fits(compiled).err().into_iter().collect(),
                awaiting: Vec::new(),
            },
            Err(errors) => {
                let (awaiting, errors): (Vec<_>, Vec<_>) = errors
                    .into_iter()
                    .partition(|error| catalogue.awaits_variable(&expr.0, error));
                let said = |errors: Vec<telar_expression::Error>| {
                    errors
                        .iter()
                        .map(|error| Mistake::of(catalogue, error))
                        .collect()
                };
                Self {
                    errors: said(errors),
                    awaiting: said(awaiting),
                }
            }
        }
    }
}

/// How an expression that does not check is reported.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Severity {
    Error,
    Warning,
}

impl Severity {
    fn say(self, report: &mut Report, finding: Finding) {
        match self {
            Severity::Error => report.error(finding),
            Severity::Warning => report.warn(finding),
        }
    }
}

/// Where the expressions of one level are checked: the level's own `layers`, and `scope`, that level merged over everything of the same file under it, which says whether a group the level writes into repeats. `inherits` is whether the layout extends another, whose groups this file cannot see.
#[derive(Clone, Copy)]
struct Level<'a> {
    layers: &'a Layers,
    scope: &'a Layers,
    inherits: bool,
    at: &'a str,
    file: &'a str,
}

fn check_expressions(
    level: Level<'_>,
    catalogue: &dyn Catalogue,
    severity: Severity,
    report: &mut Report,
) {
    let Level {
        layers,
        scope,
        inherits,
        at,
        file,
    } = level;
    let found = |report: &mut Report, key: String, expr: &Expr, problems: Problems| {
        say_problems(report, (file, key), expr, problems, severity)
    };
    for (kind, layer) in layers.each() {
        let on_lock = kind == LayerKind::Lock;
        for area in &layer.areas {
            let at = format!("{at}.layers.{kind}.areas.{}", area.id);
            if let Some(visible) = &area.visible {
                found(
                    report,
                    format!("{at}.visible"),
                    visible,
                    visible_problems(catalogue, visible, on_lock),
                );
            }
            for group in &area.groups {
                let at = format!("{at}.groups.{}", group.id);
                let merged = merged_group(scope, kind, &area.id, &group.id);
                let placed = merged.and_then(|merged| merged.kind).or(group.kind);
                let repeat = merged
                    .and_then(|merged| merged.repeat.as_ref())
                    .or(group.repeat.as_ref());
                if let Some(written) = &group.repeat {
                    found(
                        report,
                        format!("{at}.repeat"),
                        written,
                        repeat_problems(catalogue, written, on_lock),
                    );
                }
                let on_cell = matches!(placed, Some(GroupKind::Cell { .. }));
                if on_cell && repeat.is_some() && (group.repeat.is_some() || group.kind.is_some()) {
                    let key = match group.repeat {
                        Some(_) => format!("{at}.repeat"),
                        None => format!("{at}.place"),
                    };
                    let finding = Finding::new(file, key, util::message!("finding.cell_repeats"));
                    severity.say(report, finding);
                }
                let locals = copy_locals(catalogue, placed, repeat, inherits, on_lock);
                for instance in &group.children {
                    for (path, expr) in &instance.bindings {
                        let problems = binding_problems(
                            catalogue,
                            instance.module.as_deref(),
                            path,
                            expr,
                            on_lock,
                            &locals,
                        );
                        found(
                            report,
                            format!("{at}.children.{}.bindings.{path}", instance.id),
                            expr,
                            problems,
                        );
                    }
                }
            }
        }
    }
    for (kind, layer) in layers.each() {
        for area in layer.areas.iter().filter(|area| area.visible.is_some()) {
            if area.reserve == Some(true) {
                report.warn(Finding::new(
                    file,
                    format!("{at}.layers.{kind}.areas.{}.visible", area.id),
                    util::message!("finding.hidden_reserve"),
                ));
            }
        }
    }
}

/// `group` of `area` on the `kind` layer of `scope`, if it is there.
fn merged_group<'a>(
    scope: &'a Layers,
    kind: LayerKind,
    area: &AreaId,
    group: &GroupId,
) -> Option<&'a Group> {
    scope
        .get(kind)
        .areas
        .iter()
        .find(|held| held.id == *area)
        .and_then(|held| held.groups.iter().find(|held| held.id == *group))
}

/// What the children of a group `placed` and repeated over `repeat`, as far as one file says, read besides the shell's names. A group this file never places is one it inherits when it `inherits`: its parent may repeat it, so its children may read `$item` and `$index` as anything rather than be refused for it.
fn copy_locals(
    catalogue: &dyn Catalogue,
    placed: Option<GroupKind>,
    repeat: Option<&Expr>,
    inherits: bool,
    on_lock: bool,
) -> Locals {
    match repeat {
        Some(repeat) => repeat_locals(catalogue, repeat, on_lock),
        None if placed.is_none() && inherits => Locals::of_copy(Type::Never),
        None => Locals::default(),
    }
}

/// What a child of `group` in `area`, where `site` writes it, reads besides the shell's names: `$item` and `$index` when the group repeats, as `layout` and the broader rules under `site` say — what validation checks that child's bindings against, for a caller writing one.
pub fn child_locals(
    layout: &Layout,
    catalogue: &dyn Catalogue,
    site: &crate::ops::Site,
    area: &AreaId,
    group: &GroupId,
) -> Locals {
    let mut scope = output_level(layout, &site.output);
    if let Some(workspace) = &site.workspace {
        let written = layout
            .outputs
            .iter()
            .filter(|rule| rule.matches == site.output)
            .flat_map(|rule| rule.workspaces.iter())
            .filter(|rule| rule.matches == *workspace);
        for rule in written {
            merge_session_layers(&mut scope, &rule.layers);
        }
    }
    let merged = merged_group(&scope, site.layer, area, group);
    copy_locals(
        catalogue,
        merged.and_then(|merged| merged.kind),
        merged.and_then(|merged| merged.repeat.as_ref()),
        layout.extends.is_some(),
        site.layer == LayerKind::Lock,
    )
}

/// What can take an expression back, and so which paths its `unset` may name.
#[derive(Clone, Copy)]
enum Holder {
    Area,
    Group,
    Instance,
}

impl Holder {
    fn takes(self, unset: &Unset) -> bool {
        matches!(
            (self, unset),
            (Holder::Area, Unset::Visible)
                | (
                    Holder::Group,
                    Unset::Repeat | Unset::Arrange | Unset::Parameter(_)
                )
                | (Holder::Instance, Unset::Binding(_))
        )
    }

    fn refusal(self, unset: &Unset) -> Message {
        match self {
            Holder::Area => util::message!("finding.unset_on_area", unset = unset),
            Holder::Group => util::message!("finding.unset_on_group", unset = unset),
            Holder::Instance => util::message!("finding.unset_on_instance", unset = unset),
        }
    }
}

/// Everything wrong with what each `unset` of one level names: a path its holder has no expression at, a binding its module cannot have, and a key the same level writes as well, which would leave which of the two it meant to chance.
fn check_unsets(
    level: Level<'_>,
    catalogue: &dyn Catalogue,
    severity: Severity,
    report: &mut Report,
) {
    let Level {
        layers,
        scope,
        at,
        file,
        ..
    } = level;
    let mut say = |key: String, why: Message| severity.say(report, Finding::new(file, key, why));
    for (kind, layer) in layers.each() {
        for area in &layer.areas {
            let at = format!("{at}.layers.{kind}.areas.{}", area.id);
            for (index, unset) in area.unset.iter().enumerate() {
                let key = format!("{at}.unset[{index}]");
                if !Holder::Area.takes(unset) {
                    say(key, Holder::Area.refusal(unset));
                } else if area.visible.is_some() {
                    say(key, both(unset));
                }
            }
            for group in &area.groups {
                let at = format!("{at}.groups.{}", group.id);
                for (index, unset) in group.unset.iter().enumerate() {
                    let key = format!("{at}.unset[{index}]");
                    let written = match unset {
                        Unset::Parameter(name) => group.parameters.contains_key(name),
                        Unset::Arrange => group.writes_arrangement(),
                        _ => group.repeat.is_some(),
                    };
                    if !Holder::Group.takes(unset) {
                        say(key, Holder::Group.refusal(unset));
                    } else if written {
                        say(key, both(unset));
                    }
                }
                for instance in &group.children {
                    let at = format!("{at}.children.{}", instance.id);
                    let module = instance.module.as_deref().or_else(|| {
                        merged_group(scope, kind, &area.id, &group.id)?
                            .children
                            .iter()
                            .find(|held| held.id == instance.id)?
                            .module
                            .as_deref()
                    });
                    for (index, unset) in instance.unset.iter().enumerate() {
                        let key = format!("{at}.unset[{index}]");
                        let Unset::Binding(path) = unset else {
                            say(key, Holder::Instance.refusal(unset));
                            continue;
                        };
                        if instance.bindings.contains_key(path) {
                            say(key, both(unset));
                            continue;
                        }
                        if let Some(module) = module.filter(|module| catalogue.knows_module(module))
                            && let Err(why) = catalogue.binding_type(module, path)
                        {
                            say(
                                key,
                                util::message!(
                                    "finding.unset_no_binding",
                                    module = module,
                                    path = path,
                                    why = why
                                ),
                            );
                        }
                    }
                }
            }
        }
    }
}

fn both(unset: &Unset) -> Message {
    util::message!("finding.unset_and_written", unset = unset)
}

/// What `layout` takes back that nothing under it writes: every `unset` of its own rules, against its `extends` chain and the rules of its own applied before each one.
///
/// A warning rather than an error, because taking back nothing changes nothing. It needs the layouts `layout` extends, which [`validate`] does not see. A rule counts as under another when both could speak for one output, so a pattern that could be the same monitor is never said to have nothing under it; a path that names no expression at all is [`validate`]'s to report.
pub fn validate_unsets(layout: &Layout, known: &Library) -> Report {
    let mut report = Report::default();
    let file = format!("layouts/{}.toml", layout.id);
    let chain = crate::resolve::chain_of(layout, known, &mut Report::default());
    let parents = &chain[..chain.len().saturating_sub(1)];
    let mut order: Vec<&OutputRule> = layout.outputs.iter().collect();
    order.sort_by_key(|rule| rule.matches.specificity());

    let inherited = |rule: &OutputRule| {
        let mut under = BTreeSet::new();
        for other in parents.iter().flat_map(|level| level.outputs.iter()) {
            if may_share_an_output(&rule.matches, &other.matches) {
                written_in(other.layers.each(), &mut under);
                for workspace in &other.workspaces {
                    written_in(workspace.layers.each(), &mut under);
                }
            }
        }
        under
    };
    for (position, rule) in order.iter().enumerate() {
        let mut under = inherited(rule);
        for earlier in &order[..position] {
            if may_share_an_output(&rule.matches, &earlier.matches) {
                written_in(earlier.layers.each(), &mut under);
            }
        }
        let at = format!("outputs.{}", rule.matches.0);
        unset_nothing(rule.layers.each(), &under, &at, &file, &mut report);
    }
    let mut ruled: BTreeSet<KeyAt> = BTreeSet::new();
    for rule in &order {
        let mut under = inherited(rule);
        for other in order
            .iter()
            .filter(|other| may_share_an_output(&rule.matches, &other.matches))
        {
            written_in(other.layers.each(), &mut under);
        }
        for workspace in &rule.workspaces {
            under.extend(ruled.iter().cloned());
            let at = format!(
                "outputs.{}.workspaces.{}",
                rule.matches.0, workspace.matches.0
            );
            unset_nothing(workspace.layers.each(), &under, &at, &file, &mut report);
            written_in(workspace.layers.each(), &mut ruled);
        }
    }
    report
}

/// Whether some output could be spoken for by both patterns. Two patterns with a wildcard each are taken to overlap, since nothing short of every output name answers it.
fn may_share_an_output(one: &OutputMatch, other: &OutputMatch) -> bool {
    match (one.0.contains('*'), other.0.contains('*')) {
        (false, _) => other.matches(&one.0),
        (true, false) => one.matches(&other.0),
        (true, true) => true,
    }
}

fn written_in<'a>(
    layers: impl IntoIterator<Item = (LayerKind, &'a Layer)>,
    into: &mut BTreeSet<KeyAt>,
) {
    for (kind, layer) in layers {
        for area in &layer.areas {
            if area.visible.is_some() {
                into.insert((
                    kind,
                    area.id.clone(),
                    None,
                    None,
                    Unset::Visible.to_string(),
                ));
            }
            for group in &area.groups {
                let held = Some(group.id.clone());
                if group.repeat.is_some() {
                    into.insert((
                        kind,
                        area.id.clone(),
                        held.clone(),
                        None,
                        Unset::Repeat.to_string(),
                    ));
                }
                if group.writes_arrangement() {
                    into.insert((
                        kind,
                        area.id.clone(),
                        held.clone(),
                        None,
                        Unset::Arrange.to_string(),
                    ));
                }
                for name in group.parameters.keys() {
                    into.insert((
                        kind,
                        area.id.clone(),
                        held.clone(),
                        None,
                        Unset::parameter(name).to_string(),
                    ));
                }
                for instance in &group.children {
                    for path in instance.bindings.keys() {
                        into.insert((
                            kind,
                            area.id.clone(),
                            held.clone(),
                            Some(instance.id.clone()),
                            Unset::binding(path).to_string(),
                        ));
                    }
                }
            }
        }
    }
}

fn unset_nothing<'a>(
    layers: impl IntoIterator<Item = (LayerKind, &'a Layer)>,
    under: &BTreeSet<KeyAt>,
    at: &str,
    file: &str,
    report: &mut Report,
) {
    let mut warn = |key: String, unset: &Unset| {
        report.warn(Finding::new(
            file,
            key,
            util::message!("finding.unset_nothing", unset = unset),
        ));
    };
    for (kind, layer) in layers {
        for area in &layer.areas {
            let at = format!("{at}.layers.{kind}.areas.{}", area.id);
            for (index, unset) in area.unset.iter().enumerate() {
                let found = (kind, area.id.clone(), None, None, unset.to_string());
                if Holder::Area.takes(unset) && !under.contains(&found) {
                    warn(format!("{at}.unset[{index}]"), unset);
                }
            }
            for group in &area.groups {
                let at = format!("{at}.groups.{}", group.id);
                let held = Some(group.id.clone());
                for (index, unset) in group.unset.iter().enumerate() {
                    let found = (kind, area.id.clone(), held.clone(), None, unset.to_string());
                    if Holder::Group.takes(unset) && !under.contains(&found) {
                        warn(format!("{at}.unset[{index}]"), unset);
                    }
                }
                for instance in &group.children {
                    let at = format!("{at}.children.{}", instance.id);
                    for (index, unset) in instance.unset.iter().enumerate() {
                        let found = (
                            kind,
                            area.id.clone(),
                            held.clone(),
                            Some(instance.id.clone()),
                            unset.to_string(),
                        );
                        if Holder::Instance.takes(unset) && !under.contains(&found) {
                            warn(format!("{at}.unset[{index}]"), unset);
                        }
                    }
                }
            }
        }
    }
}

/// Moves the span of every expression finding in `report` from the expression into the layout file `text`, so `file:line:column` points at the mistake itself. A finding whose expression cannot be found in the text, or is written with escapes that make its bytes differ from what was read, points at the whole value instead.
pub fn locate_expressions(text: &str, report: &mut Report) {
    let written = written_values(text, |holder, table, at| {
        let spanned = |key: &str, item: Option<&toml_edit::Item>| {
            item.and_then(toml_edit::Item::span)
                .map(|span| (format!("{at}.{key}"), span))
        };
        match holder {
            Holder::Area => spanned("visible", table.get("visible"))
                .into_iter()
                .collect(),
            Holder::Group => spanned("repeat", table.get("repeat"))
                .into_iter()
                .chain(
                    table
                        .get("parameters")
                        .and_then(|it| it.as_table_like())
                        .into_iter()
                        .flat_map(|parameters| parameters.iter())
                        .filter_map(|(name, expr)| {
                            expr.span()
                                .map(|span| (format!("{at}.parameters.{name}"), span))
                        }),
                )
                .collect(),
            Holder::Instance => table
                .get("bindings")
                .and_then(|it| it.as_table_like())
                .into_iter()
                .flat_map(|bindings| bindings.iter())
                .filter_map(|(path, expr)| {
                    expr.span()
                        .map(|span| (format!("{at}.bindings.{path}"), span))
                })
                .collect(),
        }
    });
    for finding in report.findings_mut() {
        let (Some(value), Some(span)) = (written.get(&finding.key), &finding.span) else {
            continue;
        };
        finding.span = Some(Span::within_toml_string(
            text,
            value.clone(),
            span.bytes.clone(),
        ));
    }
}

/// Points every finding about an entry of an `unset` in `report` at that entry in the layout file `text`, so `file:line:column` is the path that takes nothing back or clashes.
pub fn locate_unsets(text: &str, report: &mut Report) {
    let written = written_values(text, |_, table, at| {
        let paths = table.get("unset").and_then(|it| it.as_array());
        paths
            .into_iter()
            .flatten()
            .enumerate()
            .filter_map(|(index, path)| {
                path.span()
                    .map(|span| (format!("{at}.unset[{index}]"), span))
            })
            .collect()
    });
    for finding in report.findings_mut() {
        if let Some(value) = written.get(&finding.key) {
            finding.span = Some(Span::locate(text, value.clone()));
        }
    }
}

/// Where each value `pick` names sits in the layout file `text`, by the key validation reports it at. `pick` is shown every area, group and instance the file writes, with the key it is reported at.
fn written_values(
    text: &str,
    mut pick: impl FnMut(Holder, &toml_edit::Table, &str) -> Vec<(String, Range<usize>)>,
) -> BTreeMap<String, Range<usize>> {
    let mut written = BTreeMap::new();
    let Ok(parsed) = toml_edit::Document::parse(text) else {
        return written;
    };
    let Some(outputs) = parsed.get("outputs").and_then(|it| it.as_array_of_tables()) else {
        return written;
    };
    let mut visit =
        |holder, table: &toml_edit::Table, at: &str| written.extend(pick(holder, table, at));
    for rule in outputs {
        let at = format!(
            "outputs.{}",
            rule.get("match").and_then(|it| it.as_str()).unwrap_or("*")
        );
        holders_of(rule, &at, &mut visit);
        for workspace in rule
            .get("workspaces")
            .and_then(|it| it.as_array_of_tables())
            .into_iter()
            .flatten()
        {
            let matches = workspace
                .get("match")
                .and_then(|it| it.as_str())
                .unwrap_or("");
            holders_of(workspace, &format!("{at}.workspaces.{matches}"), &mut visit);
        }
    }
    written
}

/// Shows `visit` every area, group and instance one output rule or workspace rule of a layout file writes, with the key validation reports it at.
fn holders_of(
    level: &toml_edit::Table,
    at: &str,
    visit: &mut impl FnMut(Holder, &toml_edit::Table, &str),
) {
    let Some(layers) = level.get("layers").and_then(|it| it.as_table()) else {
        return;
    };
    let id = |table: &toml_edit::Table| {
        table
            .get("id")
            .and_then(|it| it.as_str())
            .unwrap_or("")
            .to_string()
    };
    for (layer, value) in layers.iter() {
        let areas = value
            .as_table()
            .and_then(|it| it.get("areas"))
            .and_then(|it| it.as_array_of_tables());
        for area in areas.into_iter().flatten() {
            let at = format!("{at}.layers.{layer}.areas.{}", id(area));
            visit(Holder::Area, area, &at);
            let groups = area.get("groups").and_then(|it| it.as_array_of_tables());
            for group in groups.into_iter().flatten() {
                let at = format!("{at}.groups.{}", id(group));
                visit(Holder::Group, group, &at);
                let children = group.get("children").and_then(|it| it.as_array_of_tables());
                for instance in children.into_iter().flatten() {
                    visit(
                        Holder::Instance,
                        instance,
                        &format!("{at}.children.{}", id(instance)),
                    );
                }
            }
        }
    }
}

/// Everything wrong with `expr` as a value of a komponent parameter of type `ty`, read where the group using it is drawn — on the lock layer when `on_lock`.
pub fn parameter_errors(
    catalogue: &dyn Catalogue,
    ty: &Type,
    expr: &Expr,
    on_lock: bool,
) -> Vec<Mistake> {
    typed_problems(catalogue, expr, Some(ty), on_lock, &Locals::default()).errors
}

/// Reports what `problems` finds in `expr`, written at `key` of `file`: what keeps it from being drawn as `severity` says, and a variable not set yet as a warning.
fn say_problems(
    report: &mut Report,
    (file, key): (&str, String),
    expr: &Expr,
    problems: Problems,
    severity: Severity,
) {
    let said = problems
        .errors
        .into_iter()
        .map(|mistake| (severity, mistake))
        .chain(
            problems
                .awaiting
                .into_iter()
                .map(|mistake| (Severity::Warning, mistake)),
        );
    for (severity, mistake) in said {
        let mut finding = Finding::new(file, key.clone(), mistake.message);
        finding.span = Some(Span::locate(&expr.0, mistake.span.range()));
        severity.say(report, finding);
    }
}

/// Everything wrong with the komponent `id` as its own file says it, wherever it is used: a parameter's name and its default, a child's id, module, options and actions, and each expression it holds, checked with its parameters — and, in a copy of what it repeats, `$item` and `$index` — in scope. What depends on where it is used — the lock layer's rules, what a use sets — is [`validate_komponents`]'s.
pub fn validate_komponent(
    id: &KomponentId,
    komponent: &Komponent,
    catalogue: &dyn Catalogue,
) -> Report {
    let mut report = Report::default();
    let file = komponent_path(id);
    crate::names::unreadable_in_komponent(komponent, &file, &mut report);
    for (name, parameter) in &komponent.parameters {
        let at = format!("parameters.{name}");
        if !is_identifier(name) {
            report.error(Finding::new(
                &file,
                at.clone(),
                util::message!("finding.parameter_name", name = name),
            ));
        } else if name == ITEM || name == INDEX {
            report.error(Finding::new(
                &file,
                at.clone(),
                util::message!("finding.parameter_local", name = name),
            ));
        } else if catalogue.is_service_source(name) {
            report.error(Finding::new(
                &file,
                at.clone(),
                util::message!("finding.parameter_shadows", name = name),
            ));
        }
        let problems = typed_problems(
            catalogue,
            &parameter.default,
            Some(&parameter.ty.0),
            false,
            &Locals::default(),
        );
        say_problems(
            &mut report,
            (&file, format!("{at}.default")),
            &parameter.default,
            problems,
            Severity::Error,
        );
    }
    if let Some(arrange) = komponent.arrange.filter(|arrange| places_each(*arrange))
        && komponent.repeat.is_some()
    {
        report.error(Finding::new(
            &file,
            "repeat",
            util::message!("finding.arranged_repeats", arrange = arrange.as_str()),
        ));
    }
    check_tracks(
        (komponent.cols, komponent.rows, komponent.gap),
        str::to_string,
        &file,
        &mut report,
    );
    check_unarranged(
        [
            (komponent.cols.is_some(), true),
            (komponent.rows.is_some(), true),
            (komponent.gap.is_some(), true),
        ],
        (komponent.arrange, true),
        str::to_string,
        &file,
        &mut report,
    );
    let parameters = Locals::of_parameters(komponent);
    let locals = match &komponent.repeat {
        Some(repeat) => {
            let problems = repeat_problems_with(catalogue, repeat, false, &parameters);
            say_problems(
                &mut report,
                (&file, "repeat".to_string()),
                repeat,
                problems,
                Severity::Error,
            );
            parameters
                .clone()
                .and(repeat_locals_with(catalogue, repeat, false, &parameters))
        }
        None => parameters,
    };
    let mut ids = BTreeSet::new();
    for child in &komponent.children {
        let at = format!("children.{}", child.id);
        if !child.id.is_empty() && !ids.insert(child.id.clone()) {
            report.error(Finding::new(
                &file,
                at.clone(),
                util::message!("finding.shared_child_id"),
            ));
        }
        check_instance(child, &at, &file, catalogue, &mut report);
        check_child_place(
            (child, Some(child)),
            (komponent.arrange, true),
            &at,
            &file,
            &mut report,
        );
        check_style(&child.style, Styled::Instance, &at, &file, &mut report);
        check_chains(&child.actions, &at, &file, catalogue, &mut report);
        for (path, expr) in &child.bindings {
            let problems = binding_problems(
                catalogue,
                child.module.as_deref(),
                path,
                expr,
                false,
                &locals,
            );
            say_problems(
                &mut report,
                (&file, format!("{at}.bindings.{path}")),
                expr,
                problems,
                Severity::Error,
            );
        }
    }
    report
}

/// What a module the instance shows, the size it is drawn at and the options it sets get wrong, reported under `at`.
fn check_instance(
    instance: &Instance,
    at: &str,
    file: &str,
    catalogue: &dyn Catalogue,
    report: &mut Report,
) {
    let Some(module) = &instance.module else {
        return;
    };
    if !catalogue.knows_module(module) {
        report.error(Finding::new(
            file,
            format!("{at}.module"),
            util::message!("finding.unknown_module", module = module),
        ));
        return;
    }
    let representation = instance.representation.unwrap_or(Representation::Chip);
    if !catalogue.has_representation(module, representation) {
        report.error(Finding::new(
            file,
            format!("{at}.representation"),
            util::message!(
                "finding.no_representation",
                module = module,
                representation = representation.as_str()
            ),
        ));
    }
    for (key, why) in catalogue.option_problems(module, &instance.options) {
        report.error(Finding::new(
            file,
            format!("{at}.options.{key}"),
            util::message!("finding.option", module = module, key = key, why = why),
        ));
    }
}

/// Everything wrong with how `layout` uses komponents that only the komponents in `library` can say: a parameter a use sets, checked as the type the komponent declares; a panel owned by a child of a use that the komponent does not hold; and on the lock layer, a komponent holding a control or an action, or an expression the lock screen cannot read (TA-8). A komponent the library does not hold, and a parameter it does not declare, are resolution's to report, where they are drawn as placeholders.
pub fn validate_komponents(
    layout: &Layout,
    library: &Library,
    catalogue: &dyn Catalogue,
) -> Report {
    let mut report = komponent_uses(layout, library, catalogue, LayerKind::ALL.as_slice());
    check_komponent_owners(layout, library, &mut report);
    report
}

/// A panel whose owner is addressed as a child of a komponent's use (`bar-top.end/battery`) that the komponent `library` holds under that name does not have: resolution drops such a panel as it drops one whose owner is nowhere. [`check_panels`] reads no komponent file, so it takes any child under a use. Judged on the panel and its owner as each level and the broader ones of the same file merge them.
fn check_komponent_owners(layout: &Layout, library: &Library, report: &mut Report) {
    let file = format!("layouts/{}.toml", layout.id);
    for rule in &layout.outputs {
        let at = format!("outputs.{}", rule.matches.0);
        let scope = output_level(layout, &rule.matches);
        let workspaces = rule.workspaces.iter().map(|workspace| {
            let mut scope = scope.clone();
            merge_session_layers(&mut scope, &workspace.layers);
            (
                format!("{at}.workspaces.{}", workspace.matches.0),
                workspace.layers.each().to_vec(),
                scope,
            )
        });
        let levels = std::iter::once((at.clone(), rule.layers.each().to_vec(), scope.clone()))
            .chain(workspaces);
        for (at, written, scope) in levels {
            for (kind, layer) in written
                .into_iter()
                .filter(|(kind, _)| *kind != LayerKind::Lock)
            {
                let merged = scope.get(kind);
                for area in &layer.areas {
                    let Some(AreaKind::Panel {
                        owner: writes_owner,
                        ..
                    }) = &area.kind
                    else {
                        continue;
                    };
                    let held = merged
                        .areas
                        .iter()
                        .find(|held| held.id == area.id)
                        .unwrap_or(area);
                    let Some(owner) = held.kind.as_ref().and_then(AreaKind::owner) else {
                        continue;
                    };
                    let Some((komponent, child)) = merged
                        .areas
                        .iter()
                        .find_map(|holder| holder.komponent_child(owner))
                    else {
                        continue;
                    };
                    let Some(used) = library.komponent(komponent) else {
                        continue;
                    };
                    if used.children.iter().any(|held| held.id.as_str() == child) {
                        continue;
                    }
                    let key = match writes_owner {
                        Some(_) => "owner",
                        None => "kind",
                    };
                    report.error(Finding::new(
                        &file,
                        format!("{at}.layers.{kind}.areas.{}.{key}", area.id),
                        util::message!("finding.panel_owner_missing", owner = owner, layer = kind),
                    ));
                }
            }
        }
    }
}

/// [`validate_komponents`] for the lock layer alone, for the session opener, which decides between the layout's lock screen and the minimal one: a control or an action is still an error, while an expression the lock screen cannot read is a warning, since it is left out where it is drawn.
pub fn validate_komponents_lock(
    layout: &Layout,
    library: &Library,
    catalogue: &dyn Catalogue,
) -> Report {
    komponent_uses(layout, library, catalogue, &[LayerKind::Lock])
}

fn komponent_uses(
    layout: &Layout,
    library: &Library,
    catalogue: &dyn Catalogue,
    layers: &[LayerKind],
) -> Report {
    let mut report = Report::default();
    let file = format!("layouts/{}.toml", layout.id);
    let lock_only = layers == [LayerKind::Lock];
    let severity = match lock_only {
        true => Severity::Warning,
        false => Severity::Error,
    };
    for rule in &layout.outputs {
        let at = format!("outputs.{}", rule.matches.0);
        let scope = output_level(layout, &rule.matches);
        let levels = std::iter::once((at.clone(), rule.layers.each().to_vec())).chain(
            rule.workspaces.iter().map(|workspace| {
                (
                    format!("{at}.workspaces.{}", workspace.matches.0),
                    workspace.layers.each().to_vec(),
                )
            }),
        );
        for (at, written) in levels {
            for (kind, layer) in written
                .into_iter()
                .filter(|(kind, _)| layers.contains(kind))
            {
                let on_lock = kind == LayerKind::Lock;
                for area in &layer.areas {
                    for group in &area.groups {
                        let at =
                            format!("{at}.layers.{kind}.areas.{}.groups.{}", area.id, group.id);
                        let used = group.komponent.clone().or_else(|| {
                            merged_group(&scope, kind, &area.id, &group.id)?
                                .komponent
                                .clone()
                        });
                        let Some(komponent) = used.as_ref().and_then(|id| library.komponent(id))
                        else {
                            continue;
                        };
                        let placed = group
                            .kind
                            .or_else(|| merged_group(&scope, kind, &area.id, &group.id)?.kind);
                        if group.komponent.is_some()
                            && komponent.repeat.is_some()
                            && matches!(placed, Some(GroupKind::Cell { .. }))
                        {
                            severity.say(
                                &mut report,
                                Finding::new(
                                    &file,
                                    format!("{at}.komponent"),
                                    util::message!("finding.cell_repeats"),
                                ),
                            );
                        }
                        if group.komponent.is_some()
                            && let Some(arrange) = komponent.arrange
                            && arrange != Arrange::Pages
                            && matches!(placed, Some(GroupKind::Zone { .. }))
                        {
                            severity.say(
                                &mut report,
                                Finding::new(
                                    &file,
                                    format!("{at}.komponent"),
                                    util::message!(
                                        "finding.zone_arrange",
                                        arrange = arrange.as_str()
                                    ),
                                ),
                            );
                        }
                        for (name, expr) in &group.parameters {
                            let Some(declared) = komponent.parameters.get(name) else {
                                continue;
                            };
                            let problems = typed_problems(
                                catalogue,
                                expr,
                                Some(&declared.ty.0),
                                on_lock,
                                &Locals::default(),
                            );
                            say_problems(
                                &mut report,
                                (&file, format!("{at}.parameters.{name}")),
                                expr,
                                problems,
                                severity,
                            );
                        }
                        if on_lock && group.komponent.is_some() {
                            let id = used.as_ref().expect("a komponent was found");
                            lock_komponent(
                                (id, komponent),
                                (&file, &format!("{at}.komponent")),
                                catalogue,
                                severity,
                                &mut report,
                            );
                        }
                    }
                }
            }
        }
    }
    report
}

/// What the lock layer refuses in the komponent `id`, wherever a group there would use it: everything [`validate_komponents`] reports for a use on the lock layer, as errors.
pub(crate) fn lock_problems(
    id: &KomponentId,
    komponent: &Komponent,
    catalogue: &dyn Catalogue,
) -> Report {
    let mut report = Report::default();
    lock_komponent(
        (id, komponent),
        ("", ""),
        catalogue,
        Severity::Error,
        &mut report,
    );
    report
}

/// What the lock layer refuses in the komponent `id` a group there uses, reported at the use's `komponent` key: a child drawn as a control, an action, and an expression the lock screen cannot read — the last as `severity` says.
fn lock_komponent(
    (id, komponent): (&KomponentId, &Komponent),
    (file, at): (&str, &str),
    catalogue: &dyn Catalogue,
    severity: Severity,
    report: &mut Report,
) {
    let parameters = Locals::of_parameters(komponent);
    let locals = match &komponent.repeat {
        Some(repeat) => {
            parameters
                .clone()
                .and(repeat_locals_with(catalogue, repeat, true, &parameters))
        }
        None => parameters.clone(),
    };
    let unreadable = |what: String, problems: Problems, report: &mut Report| {
        for mistake in problems.errors {
            let message = util::message!(
                "finding.komponent_lock_expression",
                komponent = id,
                what = &what,
                why = mistake.message
            );
            severity.say(report, Finding::new(file, at, message));
        }
    };
    for (name, parameter) in &komponent.parameters {
        let problems = typed_problems(
            catalogue,
            &parameter.default,
            Some(&parameter.ty.0),
            true,
            &Locals::default(),
        );
        unreadable(format!("parameters.{name}.default"), problems, report);
    }
    if let Some(repeat) = &komponent.repeat {
        let problems = repeat_problems_with(catalogue, repeat, true, &parameters);
        unreadable("repeat".to_string(), problems, report);
    }
    for child in &komponent.children {
        if !child.actions.is_empty() {
            report.error(Finding::new(
                file,
                at,
                util::message!(
                    "finding.komponent_lock_action",
                    komponent = id,
                    child = &child.id
                ),
            ));
        }
        if let Some(module) = &child.module {
            let representation = child.representation.unwrap_or(Representation::Chip);
            if catalogue.knows_module(module) && !catalogue.is_read_only(module, representation) {
                report.error(Finding::new(
                    file,
                    at,
                    util::message!(
                        "finding.komponent_lock_interactive",
                        komponent = id,
                        child = &child.id,
                        module = module,
                        representation = representation.as_str()
                    ),
                ));
            }
        }
        for (path, expr) in &child.bindings {
            let problems = binding_problems(
                catalogue,
                child.module.as_deref(),
                path,
                expr,
                true,
                &locals,
            );
            unreadable(
                format!("children.{}.bindings.{path}", child.id),
                problems,
                report,
            );
        }
    }
}

/// Moves the span of every expression finding about the komponent file `text` into it, as [`locate_expressions`] does for a layout file: a parameter's default, the komponent's `repeat` and each child's bindings.
pub fn locate_komponent_expressions(text: &str, report: &mut Report) {
    let Ok(parsed) = toml_edit::Document::parse(text) else {
        return;
    };
    let mut written: BTreeMap<String, Range<usize>> = BTreeMap::new();
    if let Some(span) = parsed.get("repeat").and_then(toml_edit::Item::span) {
        written.insert("repeat".to_string(), span);
    }
    let parameters = parsed.get("parameters").and_then(|it| it.as_table_like());
    for (name, parameter) in parameters.into_iter().flat_map(|it| it.iter()) {
        if let Some(span) = parameter
            .as_table_like()
            .and_then(|it| it.get("default"))
            .and_then(toml_edit::Item::span)
        {
            written.insert(format!("parameters.{name}.default"), span);
        }
    }
    let children = parsed
        .get("children")
        .and_then(|it| it.as_array_of_tables());
    for child in children.into_iter().flatten() {
        let id = child.get("id").and_then(|it| it.as_str()).unwrap_or("");
        let bindings = child.get("bindings").and_then(|it| it.as_table_like());
        for (path, expr) in bindings.into_iter().flat_map(|it| it.iter()) {
            if let Some(span) = expr.span() {
                written.insert(format!("children.{id}.bindings.{path}"), span);
            }
        }
    }
    for finding in report.findings_mut() {
        let (Some(value), Some(span)) = (written.get(&finding.key), &finding.span) else {
            continue;
        };
        finding.span = Some(Span::within_toml_string(
            text,
            value.clone(),
            span.bytes.clone(),
        ));
    }
}

fn check_lock_layer(
    lock: &Layer,
    at: &str,
    file: &str,
    catalogue: &dyn Catalogue,
    report: &mut Report,
) {
    let refuse = |path: String, report: &mut Report| {
        report.error(Finding::new(
            file,
            format!("{path}.actions"),
            util::message!("finding.lock_action"),
        ));
    };
    for area in lock.areas.iter().filter(|area| !area.actions.is_empty()) {
        refuse(format!("{at}.layers.lock.areas.{}", area.id), report);
    }
    for area in &lock.areas {
        if matches!(area.kind, Some(AreaKind::Panel { .. })) {
            report.error(Finding::new(
                file,
                format!("{at}.layers.lock.areas.{}.kind", area.id),
                util::message!("finding.panel_on_lock"),
            ));
        }
    }
    for (area, group, instance) in instances_of(lock) {
        let path = format!(
            "{at}.layers.lock.areas.{}.groups.{}.children.{}",
            area.id, group.id, instance.id
        );
        if !instance.actions.is_empty() {
            refuse(path.clone(), report);
        }
        let Some(module) = &instance.module else {
            continue;
        };
        let representation = instance.representation.unwrap_or(Representation::Chip);
        if catalogue.knows_module(module) && !catalogue.is_read_only(module, representation) {
            report.error(Finding::new(
                file,
                format!("{path}.module"),
                util::message!(
                    "finding.lock_interactive",
                    module = module,
                    representation = representation.as_str()
                ),
            ));
        }
    }
}

fn check_workspace_rules(
    layout: &Layout,
    rule: &OutputRule,
    at: &str,
    file: &str,
    catalogue: &dyn Catalogue,
    report: &mut Report,
) {
    let reserving = reserving_under(layout, &rule.matches);

    for workspace in &rule.workspaces {
        let at = format!("{at}.workspaces.{}", workspace.matches.0);
        let layers = Layers {
            background: workspace.layers.background.clone(),
            desktop: workspace.layers.desktop.clone(),
            top: workspace.layers.top.clone(),
            overlay: workspace.layers.overlay.clone(),
            lock: Layer::default(),
        };

        for (kind, layer) in layers.each() {
            for id in &layer.remove {
                if reserving.contains(id) {
                    report.error(Finding::new(
                        file,
                        format!("{at}.layers.{kind}.remove"),
                        util::message!("finding.workspace_removes_reserving", id = id),
                    ));
                }
            }
            for area in &layer.areas {
                let path = format!("{at}.layers.{kind}.areas.{}", area.id);
                if area.reserve.is_some() {
                    report.error(Finding::new(
                        file,
                        format!("{path}.reserve"),
                        util::message!("finding.workspace_reserve"),
                    ));
                }
                if reserving.contains(&area.id) && area.kind.is_some() {
                    report.error(Finding::new(
                        file,
                        format!("{path}.kind"),
                        util::message!("finding.workspace_reserving_kind", id = &area.id),
                    ));
                }
            }
        }

        check_actions(&layers, &at, file, catalogue, report);
        check_modules(&layers, &at, file, catalogue, report);
        check_styles(&layers, &at, file, report);
        let mut scope = output_level(layout, &rule.matches);
        merge_session_layers(&mut scope, &workspace.layers);
        let level = Level {
            layers: &layers,
            scope: &scope,
            inherits: layout.extends.is_some(),
            at: &at,
            file,
        };
        check_expressions(level, catalogue, Severity::Error, report);
        check_unsets(level, catalogue, Severity::Error, report);
        check_containers(level, report);
        check_panels(level, report);
    }
}

/// What reserves space under the workspace rules of the output rule `matches` names: that rule and every broader one of the same layout, merged the way resolution merges them. A workspace rule may change what is inside one of these areas, never whether it is there, what it reserves or how big it is (TA-2).
pub(crate) fn reserving_under(layout: &Layout, matches: &OutputMatch) -> BTreeSet<AreaId> {
    output_level(layout, matches)
        .each()
        .into_iter()
        .flat_map(|(_, layer)| layer.areas.iter())
        .filter(|area| area.reserve == Some(true))
        .map(|area| area.id.clone())
        .collect()
}

/// The output rule `matches` names and every broader one of the same layout, merged the way resolution merges them: what that rule's output is drawn from, as far as this file says.
fn output_level(layout: &Layout, matches: &OutputMatch) -> Layers {
    let mut merged = Layers::default();
    for other in &layout.outputs {
        if other.matches.specificity() <= matches.specificity() {
            merge_layers(&mut merged, &other.layers);
        }
    }
    merged
}

fn instances_of(layer: &Layer) -> impl Iterator<Item = (&Area, &Group, &Instance)> {
    layer.areas.iter().flat_map(|area| {
        area.groups.iter().flat_map(move |group| {
            group
                .children
                .iter()
                .map(move |instance| (area, group, instance))
        })
    })
}
