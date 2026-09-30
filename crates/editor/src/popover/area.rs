//! The rows of each kind of area's inspector, and the rows every area has.
//!
//! Each kind's rows are a tool registered by kind name ([`super::add_area_tool`]), the way a mode's tools are: what T-7 adds for a kind — a region's edge handles, a stack's anchor handle — is another tool for the same kind, sharing the values these rows edit by name ([`AreaDraft::value`]).

use std::rc::Rc;

use serde::Serialize;
use serde::de::DeserializeOwned;
use telar::Reactive;

use config::Edge;
use layout::{
    Anchor, Area, AreaKind, AutoHide, Backdrop, Blend, Corners, Extent, Fit, LayerKind, Rect,
    ResolvedAreaKind, StackOutputPolicy, Tile, Transition, Within,
};

use super::draft::AreaDraft;
use super::handles;
use super::rows::{self, Range, Rows};
use super::{Inspector, add_area_tool};

/// The theme colours an area's fill is picked from: the surfaces and inks, then the accents.
const FILLS: &[&str] = &[
    "base", "surface", "overlay", "muted", "text", "blue", "cyan", "teal", "red", "orange",
    "yellow", "green", "purple",
];

macro_rules! said {
    ($key:literal) => {
        Reactive::of(|| telar::t!($key))
    };
}

pub(crate) fn install() {
    add_area_tool("bar", bar);
    add_area_tool("grid", grid);
    add_area_tool("stack", stack);
    add_area_tool("wallpaper_region", wallpaper_region);
    add_area_tool("texture", texture);
    add_area_tool("dock", dock);
    add_area_tool("free", placed_by_hand);
    add_area_tool("prompt", placed_by_hand);
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
fn variants(kind: &str) -> Rc<[&'static str]> {
    layout::schema::LAYOUT_VARIANTS
        .iter()
        .chain(config::schema::CONFIG_VARIANTS.iter())
        .filter(|(owner, _)| *owner == kind)
        .map(|(_, variant)| *variant)
        .collect()
}

fn edges() -> Rc<[&'static str]> {
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
fn picked<T: Serialize + DeserializeOwned + 'static>(
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

fn geometry(draft: &AreaDraft) -> &'static str {
    draft.kind()
}

macro_rules! kind_field {
    ($area:expr, $kind:expr, $variant:ident { $field:ident }, $value:expr) => {{
        let value = $value;
        if let Some(AreaKind::$variant { $field, .. }) = AreaDraft::kind_mut($area, $kind) {
            *$field = Some(value);
        }
    }};
}

fn bar(draft: &AreaDraft) -> Result<Inspector, telar::LayoutError> {
    let ResolvedAreaKind::Bar {
        edge,
        thickness,
        length,
        offset,
        shape,
        autohide,
    } = draft.resolved.kind.clone()
    else {
        return Ok(Inspector::default());
    };
    let kind = geometry(draft);
    let resolved_shape = surfaces::bar::bar_shape(&draft.config, shape);
    let mut list = picked(
        draft,
        "edge",
        said!("editor.area.edge"),
        help("AreaKind::Bar", "edge"),
        edges(),
        edge,
        move |area, edge: Edge| kind_field!(area, kind, Bar { edge }, edge),
    )?;
    let thickness = draft.value(
        "thickness",
        || thickness,
        move |area, value: &f32| kind_field!(area, kind, Bar { thickness }, *value),
    );
    list.push(rows::number(
        said!("editor.area.thickness"),
        help("AreaKind::Bar", "thickness"),
        thickness,
        handles::THICKNESS,
    )?);

    let along = draft
        .rect()
        .map(|rect| match edge.is_vertical() {
            true => rect.height,
            false => rect.width,
        })
        .unwrap_or(0.0);
    let longest = match edge.is_vertical() {
        true => draft.screen.1,
        false => draft.screen.0,
    };
    let fills_edge = length == Extent::Fill;
    let length = draft.value(
        "length",
        || along,
        move |area, value: &f32| kind_field!(area, kind, Bar { length }, Extent::Px(*value)),
    );
    let fills = draft.value(
        "length_fill",
        || fills_edge,
        move |area, fill: &bool| {
            let extent = match fill {
                true => Extent::Fill,
                false => Extent::Px(length.peek()),
            };
            kind_field!(area, kind, Bar { length }, extent)
        },
    );
    let seeded = std::cell::Cell::new(false);
    telar::effect(move || {
        length.with(|_| ());
        if seeded.replace(true) && fills.peek() {
            fills.set(false);
        }
    });
    list.push(rows::toggle(
        said!("editor.area.fill_edge"),
        help("AreaKind::Bar", "length"),
        fills,
    )?);
    list.push(rows::number(
        said!("editor.area.length"),
        help("AreaKind::Bar", "length"),
        length,
        Range::whole(handles::SHORTEST, longest),
    )?);
    let offset = draft.value(
        "offset",
        || offset,
        move |area, value: &f32| kind_field!(area, kind, Bar { offset }, *value),
    );
    list.push(rows::number(
        said!("editor.area.offset"),
        help("AreaKind::Bar", "offset"),
        offset,
        Range::whole(0.0, longest),
    )?);

    list.extend(picked(
        draft,
        "mode",
        said!("editor.area.mode"),
        help("BarShape", "mode"),
        variants("Shape"),
        resolved_shape.mode,
        move |area, mode| set_shape(area, kind, |shape| shape.mode = Some(mode)),
    )?);
    let gap = draft.value(
        "gap",
        || resolved_shape.gap as f32,
        move |area, value: &f32| set_shape(area, kind, |shape| shape.gap = Some(*value)),
    );
    list.push(rows::number(
        said!("editor.area.gap"),
        help("BarShape", "gap"),
        gap,
        handles::GAP,
    )?);
    let spacing = draft.value(
        "spacing",
        || resolved_shape.spacing,
        move |area, value: &f32| set_shape(area, kind, |shape| shape.spacing = Some(*value)),
    );
    list.push(rows::number(
        said!("editor.area.spacing"),
        help("BarShape", "spacing"),
        spacing,
        Range::whole(0.0, 64.0),
    )?);

    let corners = corners(
        draft,
        shape.radius.unwrap_or(Corners::all(resolved_shape.radius)),
    );
    let most = handles::most_radius(draft);
    let all = handles::uniform_radius(draft, corners);
    list.push(rows::number(
        said!("editor.area.radius"),
        help("BarShape", "radius"),
        all,
        Range::whole(0.0, most),
    )?);
    let corner_labels = [
        said!("editor.area.top_left"),
        said!("editor.area.top_right"),
        said!("editor.area.bottom_right"),
        said!("editor.area.bottom_left"),
    ];
    for (corner, label) in corners.into_iter().zip(corner_labels) {
        list.push(rows::number(label, None, corner, Range::whole(0.0, most))?);
    }

    let hides = draft.value(
        "autohide",
        || autohide.is_some(),
        move |area, on: &bool| {
            let hide = on.then(|| autohide_of(area, kind).unwrap_or_default());
            if let Some(AreaKind::Bar { autohide, .. }) = AreaDraft::kind_mut(area, kind) {
                *autohide = hide;
            }
        },
    );
    list.push(rows::toggle(
        said!("editor.area.autohide"),
        help("AreaKind::Bar", "autohide"),
        hides,
    )?);
    let hidden = autohide.unwrap_or_default();
    let peek = draft.value(
        "peek",
        || hidden.peek,
        move |area, value: &f32| set_autohide(area, kind, |hide| hide.peek = *value),
    );
    list.push(rows::number(
        said!("editor.area.peek"),
        help("AutoHide", "peek"),
        peek,
        Range::whole(0.0, 32.0),
    )?);
    let on_hover = draft.value(
        "on_hover",
        || hidden.on_hover,
        move |area, on: &bool| set_autohide(area, kind, |hide| hide.on_hover = *on),
    );
    list.push(rows::toggle(
        said!("editor.area.on_hover"),
        help("AutoHide", "on_hover"),
        on_hover,
    )?);

    Ok(Inspector {
        rows: list,
        handles: handles::bar(
            draft,
            handles::BarValues {
                edge,
                thickness,
                length,
                offset,
                gap,
                corners,
            },
        )?,
    })
}

/// The four corner radii the bar's rows and handles share, top left first and clockwise, each written back as the bar's own corners.
fn corners(draft: &AreaDraft, seed: Corners) -> [telar::RwSignal<f32>; 4] {
    let kind = geometry(draft);
    let seeds = [
        seed.top_left(),
        seed.top_right(),
        seed.bottom_right(),
        seed.bottom_left(),
    ];
    let names = [
        "radius.top_left",
        "radius.top_right",
        "radius.bottom_right",
        "radius.bottom_left",
    ];
    std::array::from_fn(|corner| {
        draft.value(
            names[corner],
            || seeds[corner],
            move |area, value: &f32| {
                set_shape(area, kind, |shape| {
                    let now = shape
                        .radius
                        .unwrap_or(Corners::each(seeds[0], seeds[1], seeds[2], seeds[3]));
                    let mut each = [
                        now.top_left(),
                        now.top_right(),
                        now.bottom_right(),
                        now.bottom_left(),
                    ];
                    each[corner] = *value;
                    shape.radius = Some(Corners::each(each[0], each[1], each[2], each[3]));
                })
            },
        )
    })
}

fn set_shape(area: &mut Area, kind: &'static str, change: impl FnOnce(&mut layout::BarShape)) {
    if let Some(AreaKind::Bar { shape, .. }) = AreaDraft::kind_mut(area, kind) {
        change(shape);
    }
}

fn autohide_of(area: &mut Area, kind: &'static str) -> Option<AutoHide> {
    match AreaDraft::kind_mut(area, kind) {
        Some(AreaKind::Bar { autohide, .. }) => *autohide,
        _ => None,
    }
}

fn set_autohide(area: &mut Area, kind: &'static str, change: impl FnOnce(&mut AutoHide)) {
    if let Some(AreaKind::Bar { autohide, .. }) = AreaDraft::kind_mut(area, kind) {
        change(autohide.get_or_insert_with(AutoHide::default));
    }
}

fn grid(draft: &AreaDraft) -> Result<Inspector, telar::LayoutError> {
    let ResolvedAreaKind::Grid {
        rect,
        cell,
        gap,
        anchor,
    } = draft.resolved.kind.clone()
    else {
        return Ok(Inspector::default());
    };
    let kind = geometry(draft);
    let cell = draft.value(
        "cell",
        || cell,
        move |area, value: &f32| kind_field!(area, kind, Grid { cell }, *value),
    );
    let gap = draft.value(
        "gap",
        || gap,
        move |area, value: &f32| kind_field!(area, kind, Grid { gap }, *value),
    );
    let mut list = vec![
        rows::number(
            said!("editor.area.cell"),
            help("AreaKind::Grid", "cell"),
            cell,
            Range::whole(16.0, 400.0),
        )?,
        rows::number(
            said!("editor.area.gap"),
            help("AreaKind::Grid", "gap"),
            gap,
            Range::whole(0.0, 64.0),
        )?,
    ];
    list.extend(picked(
        draft,
        "anchor",
        said!("editor.area.anchor"),
        help("AreaKind::Grid", "anchor"),
        variants("Anchor"),
        anchor,
        move |area, anchor: Anchor| kind_field!(area, kind, Grid { anchor }, anchor),
    )?);
    list.extend(rect_rows(draft, rect)?);
    Ok(Inspector {
        rows: list,
        handles: Vec::new(),
    })
}

fn stack(draft: &AreaDraft) -> Result<Inspector, telar::LayoutError> {
    let ResolvedAreaKind::Stack {
        anchor,
        width,
        output_policy,
        ..
    } = draft.resolved.kind.clone()
    else {
        return Ok(Inspector::default());
    };
    let kind = geometry(draft);
    let mut list = picked(
        draft,
        "anchor",
        said!("editor.area.anchor"),
        help("AreaKind::Stack", "anchor"),
        variants("Anchor"),
        anchor,
        move |area, anchor: Anchor| kind_field!(area, kind, Stack { anchor }, anchor),
    )?;
    let width = draft.value(
        "width",
        || width,
        move |area, value: &f32| kind_field!(area, kind, Stack { width }, *value),
    );
    list.push(rows::number(
        said!("editor.area.width"),
        help("AreaKind::Stack", "width"),
        width,
        Range::whole(160.0, 960.0),
    )?);
    list.extend(picked(
        draft,
        "output_policy",
        said!("editor.area.output_policy"),
        help("AreaKind::Stack", "output_policy"),
        variants("StackOutputPolicy"),
        output_policy,
        move |area, policy: StackOutputPolicy| {
            kind_field!(area, kind, Stack { output_policy }, policy)
        },
    )?);
    Ok(Inspector {
        rows: list,
        handles: Vec::new(),
    })
}

fn wallpaper_region(draft: &AreaDraft) -> Result<Inspector, telar::LayoutError> {
    let ResolvedAreaKind::WallpaperRegion {
        rect,
        source,
        fit,
        transition,
    } = draft.resolved.kind.clone()
    else {
        return Ok(Inspector::default());
    };
    let kind = geometry(draft);
    let source = draft.value(
        "source",
        || source,
        move |area, path: &String| {
            if let Some(AreaKind::WallpaperRegion { source, .. }) = AreaDraft::kind_mut(area, kind)
            {
                *source = (!path.trim().is_empty()).then(|| path.trim().to_string());
            }
        },
    );
    let mut list = vec![rows::text(
        said!("editor.area.source"),
        help("AreaKind::WallpaperRegion", "source"),
        source,
    )?];
    list.extend(picked(
        draft,
        "fit",
        said!("editor.area.fit"),
        help("AreaKind::WallpaperRegion", "fit"),
        variants("Fit"),
        fit,
        move |area, fit: Fit| kind_field!(area, kind, WallpaperRegion { fit }, fit),
    )?);
    list.extend(picked(
        draft,
        "transition",
        said!("editor.area.transition"),
        help("AreaKind::WallpaperRegion", "transition"),
        variants("Transition"),
        transition,
        move |area, transition: Transition| {
            kind_field!(area, kind, WallpaperRegion { transition }, transition)
        },
    )?);
    list.extend(rect_rows(draft, rect)?);
    Ok(Inspector {
        rows: list,
        handles: Vec::new(),
    })
}

fn texture(draft: &AreaDraft) -> Result<Inspector, telar::LayoutError> {
    let ResolvedAreaKind::Texture {
        rect,
        paint,
        tile,
        blend,
        opacity,
    } = draft.resolved.kind.clone()
    else {
        return Ok(Inspector::default());
    };
    let kind = geometry(draft);
    let mut list = Vec::new();
    match paint {
        layout::Paint::Image(path) => {
            let image = draft.value(
                "image",
                || path,
                move |area, path: &String| {
                    kind_field!(area, kind, Texture { image }, path.trim().to_string())
                },
            );
            list.push(rows::text(
                said!("editor.area.image"),
                help("AreaKind::Texture", "image"),
                image,
            )?);
        }
        layout::Paint::Gradient(gradient) => {
            let seed = gradient.clone();
            let angle = draft.value(
                "gradient.angle",
                || gradient.angle,
                move |area, value: &f32| {
                    if let Some(AreaKind::Texture { gradient, .. }) =
                        AreaDraft::kind_mut(area, kind)
                    {
                        gradient.get_or_insert_with(|| seed.clone()).angle = *value;
                    }
                },
            );
            list.push(rows::number(
                said!("editor.area.angle"),
                help("Gradient", "angle"),
                angle,
                Range::whole(0.0, 360.0),
            )?);
        }
    }
    let tiles: Rc<[&'static str]> = variants("Tile");
    if !matches!(tile, Tile::NineSlice { .. }) {
        list.extend(picked(
            draft,
            "tile",
            said!("editor.area.tile"),
            help("AreaKind::Texture", "tile"),
            tiles,
            tile,
            move |area, tile: Tile| kind_field!(area, kind, Texture { tile }, tile),
        )?);
    }
    list.extend(picked(
        draft,
        "blend",
        said!("editor.area.blend"),
        help("AreaKind::Texture", "blend"),
        variants("Blend"),
        blend,
        move |area, blend: Blend| kind_field!(area, kind, Texture { blend }, blend),
    )?);
    let opacity = draft.value(
        "texture.opacity",
        || opacity,
        move |area, value: &f32| kind_field!(area, kind, Texture { opacity }, *value),
    );
    list.push(rows::number(
        said!("editor.area.opacity"),
        help("AreaKind::Texture", "opacity"),
        opacity,
        Range::new(0.0, 1.0, 0.05),
    )?);
    list.extend(rect_rows(draft, rect)?);
    Ok(Inspector {
        rows: list,
        handles: Vec::new(),
    })
}

fn dock(draft: &AreaDraft) -> Result<Inspector, telar::LayoutError> {
    let ResolvedAreaKind::Dock { edge, thickness } = draft.resolved.kind.clone() else {
        return Ok(Inspector::default());
    };
    let kind = geometry(draft);
    let mut list = picked(
        draft,
        "edge",
        said!("editor.area.edge"),
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
        said!("editor.area.thickness"),
        help("AreaKind::Dock", "thickness"),
        thickness,
        handles::THICKNESS,
    )?);
    Ok(Inspector {
        rows: list,
        handles: Vec::new(),
    })
}

/// A free area and the lock's prompt: a rectangle placed by hand, and nothing else of their own.
fn placed_by_hand(draft: &AreaDraft) -> Result<Inspector, telar::LayoutError> {
    let rect = match draft.resolved.kind {
        ResolvedAreaKind::Free { rect } | ResolvedAreaKind::Prompt { rect } => rect,
        _ => return Ok(Inspector::default()),
    };
    Ok(Inspector {
        rows: rect_rows(draft, rect)?,
        handles: Vec::new(),
    })
}

/// A rectangle's four fractions of the output, for every kind placed by one.
fn rect_rows(draft: &AreaDraft, seed: Rect) -> Rows {
    let kind = geometry(draft);
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
        change(slot.get_or_insert(seed));
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
        rows::number(said!("editor.area.x"), help("Rect", "x"), x, fraction)?,
        rows::number(said!("editor.area.y"), help("Rect", "y"), y, fraction)?,
        rows::number(said!("editor.area.w"), help("Rect", "w"), w, fraction)?,
        rows::number(said!("editor.area.h"), help("Rect", "h"), h, fraction)?,
    ])
}

/// What every area has, after its own kind's rows: how it is painted, and what it asks of the compositor.
pub(crate) fn common(draft: &AreaDraft) -> Rows {
    let style = draft.resolved.style.clone();
    let is_bar = draft.kind() == "bar";
    let mut list = vec![rows::heading(|| telar::t!("editor.area.style"))?];
    let fill = draft.value(
        "style.fill",
        || style.fill.clone().unwrap_or_default(),
        |area, token: &String| area.style.fill = (!token.is_empty()).then(|| token.clone()),
    );
    list.push(rows::colour(
        said!("editor.area.fill"),
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
        said!("editor.area.opacity"),
        help("AreaStyle", "opacity"),
        opacity,
        Range::new(faintest, 1.0, 0.05),
    )?);
    let padding = draft.value(
        "style.padding",
        || style.padding.unwrap_or(0.0),
        |area, value: &f32| area.style.padding = Some(*value),
    );
    list.push(rows::number(
        said!("editor.area.padding"),
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
            said!("editor.area.radius"),
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
            said!("editor.area.reserve"),
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
        said!("editor.area.above_fullscreen"),
        help("Area", "above_fullscreen"),
        lifted,
    )?);
    list.push(rows::note(|| telar::t!("editor.area.scanout"))?);
    let within = draft.resolved.within;
    list.extend(picked(
        draft,
        "within",
        said!("editor.area.within"),
        help("Area", "within"),
        variants("Within"),
        within,
        |area, within: Within| area.within = Some(within),
    )?);
    Ok(list)
}

fn style_backdrop(draft: &AreaDraft, seed: Backdrop) -> Rows {
    picked(
        draft,
        "style.backdrop",
        said!("editor.area.backdrop"),
        help("AreaStyle", "backdrop"),
        variants("Backdrop"),
        seed,
        |area, backdrop: Backdrop| area.style.backdrop = Some(backdrop),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_model_value_round_trips_through_its_spelling() {
        assert_eq!(spelled(&Anchor::BottomRight), "bottom_right");
        assert_eq!(parsed::<Anchor>("top_left"), Some(Anchor::TopLeft));
        assert_eq!(parsed::<Edge>("left"), Some(Edge::Left));
        assert_eq!(parsed::<Anchor>("sideways"), None);
        assert_eq!(&*variants("Fit"), ["cover", "contain", "stretch", "tile"]);
        assert_eq!(&*edges(), ["top", "bottom", "left", "right"]);
    }

    #[test]
    fn a_rows_help_is_the_doc_comment_of_what_it_edits() {
        let thickness = help("AreaKind::Bar", "thickness").expect("documented");
        assert!(thickness.contains("thick"), "{thickness}");
        assert!(help("AreaStyle", "fill").is_some());
        assert_eq!(help("AreaKind::Bar", "nothing"), None);
    }
}
