//! The rows every area's inspector has, and the rows of the kinds no mode edits: a dock's and a free area's.
//!
//! Each kind's rows are a tool registered by kind name ([`super::add_area_tool`]): the mode that edits a kind registers its rows ([`crate::modes`]) — a bar's, a grid's, a stack's, a region's, a texture's, the prompt's — with the handles they share values with by name ([`AreaDraft::value`]). What every area has comes after them ([`common`]), first among it the switch that edits the area for one workspace alone ([`variant_rows`]).

use std::rc::Rc;

use serde::Serialize;
use serde::de::DeserializeOwned;
use telar::Reactive;

use config::Edge;
use layout::{Area, AreaKind, Backdrop, Corners, LayerKind, Rect, ResolvedAreaKind, Within};

use super::draft::{AreaDraft, kind_field};
use super::handles;
use super::rows::{self, Range, Rows, label};
use super::{Inspector, add_area_tool};

/// The theme colours an area's fill is picked from: the surfaces and inks, then the accents.
const FILLS: &[&str] = &[
    "base", "surface", "overlay", "muted", "text", "blue", "cyan", "teal", "red", "orange",
    "yellow", "green", "purple",
];

pub(crate) fn install() {
    add_area_tool("dock", dock);
    add_area_tool("free", free);
}

/// What the file says about `key` of `item`, for a row's help.
pub fn help(item: &str, key: &str) -> Option<String> {
    thread_local! {
        static VOCABULARY: Vec<layout::schema::Item> = layout::schema::vocabulary();
    }
    VOCABULARY.with(|vocabulary| {
        vocabulary
            .iter()
            .find(|found| found.name == item)?
            .keys
            .iter()
            .find(|found| found.name == key)?
            .doc
            .map(str::to_string)
    })
}

/// How a model value is spelled in the layout file.
pub fn spelled<T: Serialize>(value: &T) -> String {
    match toml::Value::try_from(value) {
        Ok(toml::Value::String(text)) => text,
        _ => String::new(),
    }
}

/// The model value the layout file spells as `text`.
pub fn parsed<T: DeserializeOwned>(text: &str) -> Option<T> {
    toml::Value::String(text.to_string()).try_into().ok()
}

/// Every spelling of the variants the layout's schema lists for `kind`.
pub(crate) fn variants(kind: &str) -> Rc<[&'static str]> {
    layout::schema::LAYOUT_VARIANTS
        .iter()
        .chain(config::schema::CONFIG_VARIANTS.iter())
        .filter(|(owner, _)| *owner == kind)
        .map(|(_, variant)| *variant)
        .collect()
}

pub(crate) fn edges() -> Rc<[&'static str]> {
    Edge::ALL
        .iter()
        .filter_map(|edge| {
            let spelled = spelled(edge);
            variants("Edge")
                .iter()
                .copied()
                .find(|known| *known == spelled)
        })
        .collect()
}

/// A picker over a model enum, its value written into the area by `write`.
pub(crate) fn picked<T: Serialize + DeserializeOwned + 'static>(
    draft: &AreaDraft,
    name: &'static str,
    label: Reactive<String>,
    help: Option<String>,
    options: Rc<[&'static str]>,
    seed: T,
    write: impl Fn(&mut Area, T) + 'static,
) -> Rows {
    let value = draft.value(
        name,
        || spelled(&seed),
        move |area, text: &String| {
            if let Some(value) = parsed::<T>(text) {
                write(area, value);
            }
        },
    );
    Ok(vec![rows::choice(label, help, value, options)?])
}

fn dock(draft: &AreaDraft) -> Result<Inspector, telar::LayoutError> {
    let ResolvedAreaKind::Dock { edge, thickness } = draft.resolved.kind.clone() else {
        return Ok(Inspector::default());
    };
    let kind = draft.kind();
    let mut list = picked(
        draft,
        "edge",
        label!("editor.area.edge"),
        help("AreaKind::Dock", "edge"),
        edges(),
        edge,
        move |area, edge: Edge| kind_field!(area, kind, Dock { edge }, edge),
    )?;
    let thickness = draft.value(
        "thickness",
        || thickness,
        move |area, value: &f32| kind_field!(area, kind, Dock { thickness }, *value),
    );
    list.push(rows::number(
        label!("editor.area.thickness"),
        help("AreaKind::Dock", "thickness"),
        thickness,
        handles::THICKNESS,
    )?);
    Ok(Inspector {
        rows: list,
        handles: Vec::new(),
    })
}

/// A free area: a rectangle placed by hand, and nothing else of its own.
fn free(draft: &AreaDraft) -> Result<Inspector, telar::LayoutError> {
    let ResolvedAreaKind::Free { rect } = draft.resolved.kind else {
        return Ok(Inspector::default());
    };
    Ok(Inspector {
        rows: rect_rows(draft, rect)?,
        handles: Vec::new(),
    })
}

/// A rectangle's four fractions of the output, for every kind placed by one — the lock's prompt kept wholly on its output and no smaller than it may be (TA-8).
pub(crate) fn rect_rows(draft: &AreaDraft, seed: Rect) -> Rows {
    let kind = draft.kind();
    let write = move |area: &mut Area, change: &dyn Fn(&mut Rect)| {
        let slot = match AreaDraft::kind_mut(area, kind) {
            Some(
                AreaKind::Grid { rect, .. }
                | AreaKind::WallpaperRegion { rect, .. }
                | AreaKind::Texture { rect, .. }
                | AreaKind::Free { rect }
                | AreaKind::Prompt { rect },
            ) => rect,
            _ => return,
        };
        let rect = slot.get_or_insert(seed);
        change(rect);
        if kind == "prompt" {
            *rect = rect.kept_on_output(layout::SMALLEST_PROMPT);
        }
    };
    let x = draft.value(
        "rect.x",
        || seed.x,
        move |area, value: &f32| write(area, &|rect| rect.x = *value),
    );
    let y = draft.value(
        "rect.y",
        || seed.y,
        move |area, value: &f32| write(area, &|rect| rect.y = *value),
    );
    let w = draft.value(
        "rect.w",
        || seed.w,
        move |area, value: &f32| write(area, &|rect| rect.w = *value),
    );
    let h = draft.value(
        "rect.h",
        || seed.h,
        move |area, value: &f32| write(area, &|rect| rect.h = *value),
    );
    let fraction = Range::new(0.0, 1.0, 0.01);
    Ok(vec![
        rows::number(label!("editor.area.x"), help("Rect", "x"), x, fraction)?,
        rows::number(label!("editor.area.y"), help("Rect", "y"), y, fraction)?,
        rows::number(label!("editor.area.w"), help("Rect", "w"), w, fraction)?,
        rows::number(label!("editor.area.h"), help("Rect", "h"), h, fraction)?,
    ])
}

/// What every area has, after its own kind's rows: how it is painted, and what it asks of the compositor.
pub(crate) fn common(draft: &AreaDraft) -> Rows {
    let style = draft.resolved.style.clone();
    let is_bar = draft.kind() == "bar";
    let mut list = variant_rows(&draft.node)?;
    list.push(rows::heading(|| telar::t!("editor.area.style"))?);
    let fill = draft.value(
        "style.fill",
        || style.fill.clone().unwrap_or_default(),
        |area, token: &String| area.style.fill = (!token.is_empty()).then(|| token.clone()),
    );
    list.push(rows::colour(
        label!("editor.area.fill"),
        help("AreaStyle", "fill"),
        fill,
        Rc::from(FILLS),
    )?);
    let faintest = match draft.kind() {
        "prompt" => layout::FAINTEST_PROMPT,
        _ => 0.0,
    };
    let opacity = draft.value(
        "style.opacity",
        || style.opacity.unwrap_or(1.0),
        |area, value: &f32| area.style.opacity = Some(*value),
    );
    list.push(rows::number(
        label!("editor.area.opacity"),
        help("AreaStyle", "opacity"),
        opacity,
        Range::new(faintest, 1.0, 0.05),
    )?);
    if draft.kind() == "prompt" {
        list.push(crate::modes::lock::contrast_row(draft)?);
    }
    let padding = draft.value(
        "style.padding",
        || style.padding.unwrap_or(0.0),
        |area, value: &f32| area.style.padding = Some(*value),
    );
    list.push(rows::number(
        label!("editor.area.padding"),
        help("AreaStyle", "padding"),
        padding,
        Range::whole(0.0, 64.0),
    )?);
    if !is_bar {
        let radius = draft.value(
            "style.radius",
            || style.radius.map_or(0.0, Corners::largest),
            |area, value: &f32| area.style.radius = Some(Corners::all(*value)),
        );
        list.push(rows::number(
            label!("editor.area.radius"),
            help("AreaStyle", "radius"),
            radius,
            Range::whole(0.0, handles::most_radius(draft)),
        )?);
    }
    list.extend(style_backdrop(draft, style.backdrop.unwrap_or_default())?);

    if draft.node.layer == LayerKind::Lock {
        return Ok(list);
    }
    list.push(rows::heading(|| telar::t!("editor.area.behaviour"))?);
    if matches!(draft.kind(), "bar" | "dock") {
        let reserve = draft.resolved.reserve;
        let reserves = draft.value(
            "reserve",
            || reserve,
            |area, on: &bool| area.reserve = Some(*on),
        );
        list.push(rows::toggle(
            label!("editor.area.reserve"),
            help("Area", "reserve"),
            reserves,
        )?);
    }
    let above = draft.resolved.above_fullscreen;
    let lifted = draft.value(
        "above_fullscreen",
        || above,
        |area, on: &bool| area.above_fullscreen = Some(*on),
    );
    list.push(rows::toggle(
        label!("editor.area.above_fullscreen"),
        help("Area", "above_fullscreen"),
        lifted,
    )?);
    list.push(rows::note(|| telar::t!("editor.area.scanout"))?);
    let within = draft.resolved.within;
    list.extend(picked(
        draft,
        "within",
        label!("editor.area.within"),
        help("Area", "within"),
        variants("Within"),
        within,
        |area, within: Within| area.within = Some(within),
    )?);
    Ok(list)
}

/// The backdrop picker, and under it, while the area asks for a blur only the compositor can give and this one gives none, why it draws translucent instead (F-10.49).
fn style_backdrop(draft: &AreaDraft, seed: Backdrop) -> Rows {
    let mut list = picked(
        draft,
        "style.backdrop",
        label!("editor.area.backdrop"),
        help("AreaStyle", "backdrop"),
        variants("Backdrop"),
        seed,
        |area, backdrop: Backdrop| area.style.backdrop = Some(backdrop),
    )?;
    let Some(chosen) = draft.shared::<String>("style.backdrop") else {
        return Ok(list);
    };
    let mut blurred = (*draft.resolved).clone();
    blurred.style.backdrop = Some(Backdrop::Blur);
    let by_compositor = surfaces::layer_window::blur_of(draft.node.layer, &blurred)
        == Some(surfaces::layer_window::Blur::Compositor);
    list.push(rows::note(move || {
        let unblurred = by_compositor
            && chosen.with(|now| parsed::<Backdrop>(now) == Some(Backdrop::Blur))
            && !platform_wayland::background_effect_supported();
        match unblurred {
            true => telar::t!("editor.texture.blur_unsupported"),
            false => String::new(),
        }
    })?);
    Ok(list)
}

/// "This workspace only", in the popover of anything on a layer with variants where the variant applies to it ([`crate::variant::applies_to`]) and the compositor has said which workspace is up ([`crate::variant::active`]): every change the popover makes is written for that workspace alone while it is on.
pub(crate) fn variant_rows(node: &surfaces::rects::Node) -> Rows {
    let offered = crate::variant::applies_to(node)
        && crate::variant::allowed(node.layer)
        && crate::variant::active().is_some();
    if !offered {
        return Ok(Vec::new());
    }
    Ok(vec![rows::toggle(
        Reactive::of(|| {
            telar::t!(
                "editor.variant.only",
                workspace = crate::variant::active()
                    .map(|workspace| workspace.0)
                    .unwrap_or_default()
            )
        }),
        help("OutputRule", "workspaces"),
        crate::variant::only(),
    )?])
}
