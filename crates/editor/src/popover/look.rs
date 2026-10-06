//! The look rows of every popover, one implementation over whatever holds a style ([`Styled`]).

use std::rc::Rc;

use telar::{LayoutError, LayoutItem, Rect, RwSignal, batch, effect};

use layout::{
    Area, AreaKind, Border, Corners, GroupId, InstanceId, LayoutOp, ResolvedArea, ResolvedAreaKind,
    Sides, Style,
};
use surfaces::reconcile::Desktop;
use surfaces::rects::Node;
use ui::descriptor::Built;

use crate::written::Written;

use super::area::help;
use super::draft::{AreaDraft, GroupDraft, InstanceDraft, group_entry};
use super::handles;
use super::rows::{self, Range, Rows, label};

pub(crate) trait Styled: Clone + 'static {
    fn style<T: Clone + PartialEq + 'static>(
        &self,
        name: &'static str,
        key: &'static str,
        read: impl Fn(&Style) -> T + 'static,
        write: impl Fn(&mut Style, &T) + 'static,
    ) -> RwSignal<T>;

    /// Shared by name with handles, but never written by itself.
    fn shared<T: Clone + PartialEq + 'static>(&self, name: &'static str, seed: T) -> RwSignal<T>;

    fn marked(&self, keys: &[&'static str], row: Box<dyn LayoutItem>) -> Built;

    fn node(&self) -> &Node;

    fn rect(&self) -> Option<Rect>;

    fn area_kind(&self) -> &'static str;

    fn documented(&self, key: &'static str) -> Option<String> {
        help("Style", key)
    }
}

impl Styled for AreaDraft {
    fn style<T: Clone + PartialEq + 'static>(
        &self,
        name: &'static str,
        key: &'static str,
        read: impl Fn(&Style) -> T + 'static,
        write: impl Fn(&mut Style, &T) + 'static,
    ) -> RwSignal<T> {
        self.setting(
            name,
            key,
            move |area| read(&area.style),
            move |area, value| write(&mut area.style, value),
        )
    }

    fn shared<T: Clone + PartialEq + 'static>(&self, name: &'static str, seed: T) -> RwSignal<T> {
        self.value(name, || seed, |_, _| {})
    }

    fn marked(&self, keys: &[&'static str], row: Box<dyn LayoutItem>) -> Built {
        AreaDraft::marked(self, keys, row)
    }

    fn rect(&self) -> Option<Rect> {
        AreaDraft::rect(self)
    }

    fn node(&self) -> &Node {
        &self.node
    }

    fn area_kind(&self) -> &'static str {
        self.kind()
    }
}

impl Styled for GroupDraft {
    fn style<T: Clone + PartialEq + 'static>(
        &self,
        name: &'static str,
        key: &'static str,
        read: impl Fn(&Style) -> T + 'static,
        write: impl Fn(&mut Style, &T) + 'static,
    ) -> RwSignal<T> {
        self.setting(
            name,
            key,
            move |group| read(&group.style),
            move |group, value| write(&mut group.style, value),
        )
    }

    fn shared<T: Clone + PartialEq + 'static>(&self, name: &'static str, seed: T) -> RwSignal<T> {
        self.area.value(name, || seed, |_, _| {})
    }

    fn marked(&self, keys: &[&'static str], row: Box<dyn LayoutItem>) -> Built {
        GroupDraft::marked(self, keys, row)
    }

    fn rect(&self) -> Option<Rect> {
        self.area.rect()
    }

    fn node(&self) -> &Node {
        &self.area.node
    }

    fn area_kind(&self) -> &'static str {
        self.area.kind()
    }
}

impl Styled for InstanceDraft {
    fn style<T: Clone + PartialEq + 'static>(
        &self,
        name: &'static str,
        _: &'static str,
        read: impl Fn(&Style) -> T + 'static,
        write: impl Fn(&mut Style, &T) + 'static,
    ) -> RwSignal<T> {
        self.setting(
            name,
            move |instance| read(&instance.style),
            move |instance, value| write(&mut instance.style, value),
        )
    }

    fn shared<T: Clone + PartialEq + 'static>(&self, name: &'static str, seed: T) -> RwSignal<T> {
        self.view(name, seed)
    }

    fn marked(&self, keys: &[&'static str], row: Box<dyn LayoutItem>) -> Built {
        InstanceDraft::marked(self, keys, row)
    }

    fn rect(&self) -> Option<Rect> {
        surfaces::rects::rect(&self.node)
    }

    fn node(&self) -> &Node {
        &self.node
    }

    fn area_kind(&self) -> &'static str {
        self.area_kind
    }
}

/// A bar's style as its look rows edit it: the area's own, but for its corners, which a bar writes as its shape's `radius`.
#[derive(Clone)]
pub(crate) struct BarStyle(pub(crate) AreaDraft);

impl Styled for BarStyle {
    fn style<T: Clone + PartialEq + 'static>(
        &self,
        name: &'static str,
        key: &'static str,
        read: impl Fn(&Style) -> T + 'static,
        write: impl Fn(&mut Style, &T) + 'static,
    ) -> RwSignal<T> {
        self.0.setting(
            name,
            on_bar(key),
            move |area| read(&bar_style(area)),
            move |area, value| restyle_bar(area, |style| write(style, value)),
        )
    }

    fn shared<T: Clone + PartialEq + 'static>(&self, name: &'static str, seed: T) -> RwSignal<T> {
        self.0.value(name, || seed, |_, _| {})
    }

    fn marked(&self, keys: &[&'static str], row: Box<dyn LayoutItem>) -> Built {
        let keys: Vec<&'static str> = keys.iter().map(|key| on_bar(key)).collect();
        self.0.marked(&keys, row)
    }

    fn node(&self) -> &Node {
        &self.0.node
    }

    fn rect(&self) -> Option<Rect> {
        self.0.rect()
    }

    fn area_kind(&self) -> &'static str {
        self.0.kind()
    }

    fn documented(&self, key: &'static str) -> Option<String> {
        match key {
            "radius" => help("BarShape", key),
            _ => help("Style", key),
        }
    }
}

fn on_bar(key: &'static str) -> &'static str {
    match key {
        "style.radius" => "shape.radius",
        _ => key,
    }
}

fn bar_style(area: &ResolvedArea) -> Style {
    let mut style = area.style.clone();
    if let ResolvedAreaKind::Bar { shape, .. } = &area.kind {
        style.radius = shape.radius;
    }
    style
}

/// A bar's kind is made a partial entry only when its corners change, so a change to the rest of its style writes nothing else.
fn restyle_bar(area: &mut Area, change: impl FnOnce(&mut Style)) {
    let corners = match &area.kind {
        Some(AreaKind::Bar { shape, .. }) => shape.radius,
        _ => None,
    };
    let mut style = Style {
        radius: corners,
        ..area.style.clone()
    };
    change(&mut style);
    if style.radius != corners
        && let Some(AreaKind::Bar { shape, .. }) = AreaDraft::kind_mut(area, "bar")
    {
        shape.radius = style.radius;
    }
    area.style = Style {
        radius: area.style.radius,
        ..style
    };
}

/// The style a tool writes without a popover: where a draft of the same node writes it, straight into what a level of the layout writes.
pub(crate) struct WrittenStyle {
    pub(crate) written: Written,
    pub(crate) of: StyleOf,
}

/// Which style, of what a level writes of an area, a look goes into.
pub(crate) enum StyleOf {
    Area,
    /// The area's own, its corners its shape's, as [`BarStyle`] edits it.
    Bar,
    Group(GroupId),
    Instance(GroupId, InstanceId),
}

impl WrittenStyle {
    pub(crate) fn ops(&self, change: impl FnOnce(&mut Style)) -> Vec<LayoutOp> {
        let mut area = self.written.area.clone();
        match &self.of {
            StyleOf::Area => change(&mut area.style),
            StyleOf::Bar => restyle_bar(&mut area, change),
            StyleOf::Group(id) => change(&mut group_entry(&mut area, id).style),
            StyleOf::Instance(group, id) => {
                let held = self.written.instance(group, id);
                let mut instance = held.instance.clone();
                change(&mut instance.style);
                return held.ops(&instance);
            }
        }
        self.written.ops(&area)
    }
}

pub(crate) fn fill(holder: &impl Styled) -> Built {
    let fill = holder.style(
        "style.fill",
        "style.fill",
        |style| style.fill.clone().unwrap_or_default(),
        |style, token: &String| style.fill = (!token.is_empty()).then(|| token.clone()),
    );
    holder.marked(
        &["style.fill"],
        rows::colour(
            label!("editor.look.fill"),
            holder.documented("fill"),
            fill,
            Rc::from(config::theme::PAINT_TOKENS),
            Rc::new(ui::form::swatch_row::is_colour),
        )?,
    )
}

/// The lock's prompt never fades below [`layout::FAINTEST_PROMPT`], nor does anything in it, so a locked screen always shows where to type.
pub(crate) fn opacity(holder: &impl Styled) -> Built {
    let faintest = match holder.area_kind() {
        "prompt" => layout::FAINTEST_PROMPT,
        _ => 0.0,
    };
    let opacity = holder.style(
        "style.opacity",
        "style.opacity",
        |style| style.opacity.unwrap_or(1.0),
        |style, value: &f32| style.opacity = Some(*value),
    );
    holder.marked(
        &["style.opacity"],
        rows::number(
            label!("editor.look.opacity"),
            holder.documented("opacity"),
            opacity,
            Range::new(faintest, 1.0, 0.05),
        )?,
    )
}

pub(crate) fn radius(holder: &impl Styled) -> Built {
    Ok(rounded(holder)?.row)
}

/// The radius rows of a holder, and the four corners they edit, which handles on the box drag too.
pub(crate) struct Rounded {
    pub(crate) row: Box<dyn LayoutItem>,
    pub(crate) corners: [RwSignal<f32>; 4],
}

pub(crate) fn rounded(holder: &impl Styled) -> Result<Rounded, LayoutError> {
    let most = most_in(holder, handles::most_radius_in, ROUNDEST);
    let default = drawn(holder, crate::tools::target::radius_of);
    let [all, corners @ ..] = four(holder, &RADIUS, default);
    let range = Range::whole(0.0, most);
    let mut list = vec![rows::number(
        label!("editor.look.radius"),
        holder.documented("radius"),
        all,
        range,
    )?];
    for (corner, label) in corners.into_iter().zip(corner_labels()) {
        list.push(rows::number(label, None, corner, range)?);
    }
    Ok(Rounded {
        row: holder.marked(&[RADIUS.key], rows::together(list)?)?,
        corners,
    })
}

/// The rows naming each corner, clockwise from the top left as the layout lists a radius per corner.
pub(crate) fn corner_labels() -> [telar::Reactive<String>; 4] {
    [
        label!("editor.look.top_left"),
        label!("editor.look.top_right"),
        label!("editor.look.bottom_right"),
        label!("editor.look.bottom_left"),
    ]
}

pub(crate) fn padding(holder: &impl Styled) -> Built {
    let most = most_in(holder, handles::most_padding_in, WIDEST_PADDING);
    let default = drawn(holder, crate::tools::target::padding_of);
    let [all, sides @ ..] = four(holder, &PADDING, default);
    let labels = [
        label!("editor.look.top"),
        label!("editor.look.right"),
        label!("editor.look.bottom"),
        label!("editor.look.left"),
    ];
    let range = Range::whole(0.0, most);
    let mut list = vec![rows::number(
        label!("editor.look.padding"),
        holder.documented("padding"),
        all,
        range,
    )?];
    for (side, label) in sides.into_iter().zip(labels) {
        list.push(rows::number(label, None, side, range)?);
    }
    holder.marked(&[PADDING.key], rows::together(list)?)
}

pub(crate) fn edges(holder: &impl Styled) -> Rows {
    let width = holder.style(
        "style.border.width",
        "style.border.width",
        |style| style.border.as_ref().map_or(0.0, Border::width),
        |style, width: &f32| style.border.get_or_insert_with(Border::default).width = Some(*width),
    );
    let color = holder.style(
        "style.border.color",
        "style.border.color",
        |style| {
            style
                .border
                .as_ref()
                .and_then(|border| border.color.clone())
                .unwrap_or_default()
        },
        |style, color: &String| {
            let border = style.border.get_or_insert_with(Border::default);
            border.color = (!color.is_empty()).then(|| color.clone());
            if border.is_empty() {
                style.border = None;
            }
        },
    );
    let shadow = holder.style(
        "style.shadow",
        "style.shadow",
        |style| {
            style
                .shadow
                .map(|step| step.to_string())
                .unwrap_or_default()
        },
        |style, step: &String| {
            style.shadow = step
                .parse::<u8>()
                .ok()
                .filter(|step| *step <= Style::DEEPEST_SHADOW)
        },
    );
    let steps: Rc<[(String, String)]> = Rc::from([
        (String::new(), telar::t!("editor.look.shadow_kind")),
        ("0".to_string(), telar::t!("editor.look.shadow_none")),
        ("1".to_string(), telar::t!("editor.look.shadow_soft")),
        ("2".to_string(), telar::t!("editor.look.shadow_medium")),
        ("3".to_string(), telar::t!("editor.look.shadow_strong")),
    ]);
    Ok(vec![
        holder.marked(
            &["style.border.width"],
            rows::number(
                label!("editor.look.border"),
                help("Border", "width"),
                width,
                Range::whole(0.0, WIDEST_BORDER),
            )?,
        )?,
        holder.marked(
            &["style.border.color"],
            rows::colour(
                label!("editor.look.border_color"),
                help("Border", "color"),
                color,
                Rc::from(config::theme::PAINT_TOKENS),
                Rc::new(ui::form::swatch_row::is_colour),
            )?,
        )?,
        holder.marked(
            &["style.shadow"],
            rows::listed(
                label!("editor.look.shadow"),
                holder.documented("shadow"),
                shadow,
                steps,
            )?,
        )?,
    ])
}

const ROUNDEST: f32 = 256.0;
const WIDEST_PADDING: f32 = 128.0;
const WIDEST_BORDER: f32 = 16.0;

/// The most a row of `holder` allows, as `of` reads it from the box it is drawn at, or `unknown` where there is no box yet or it is too small to take any.
fn most_in(holder: &impl Styled, of: fn(Rect) -> f32, unknown: f32) -> f32 {
    holder
        .rect()
        .map(of)
        .filter(|most| *most > 0.0)
        .unwrap_or(unknown)
}

/// Four values the file writes as one key — corners or sides — which a row for each and one for all four edit.
pub(crate) struct FourKeys {
    name: &'static str,
    key: &'static str,
    all: &'static str,
    each: [&'static str; 4],
    read: fn(&Style) -> Option<[f32; 4]>,
    pub(crate) write: fn(&mut Style, [f32; 4]),
}

pub(crate) const RADIUS: FourKeys = FourKeys {
    name: "style.radius.corners",
    key: "style.radius",
    all: "style.radius",
    each: [
        "style.radius.top_left",
        "style.radius.top_right",
        "style.radius.bottom_right",
        "style.radius.bottom_left",
    ],
    read: |style| style.radius.map(Corners::to_array),
    write: |style, [a, b, c, d]| style.radius = Some(Corners::each(a, b, c, d)),
};

pub(crate) const PADDING: FourKeys = FourKeys {
    name: "style.padding.sides",
    key: "style.padding",
    all: "style.padding",
    each: [
        "style.padding.top",
        "style.padding.right",
        "style.padding.bottom",
        "style.padding.left",
    ],
    read: |style| style.padding.map(Sides::to_array),
    write: |style, [a, b, c, d]| style.padding = Some(Sides::each(a, b, c, d)),
};

/// What the screen draws as the four where nothing writes them, so a row reads what the canvas shows.
fn drawn(holder: &impl Styled, of: fn(&Desktop, &Node) -> Option<[f32; 4]>) -> [f32; 4] {
    let node = holder.node();
    surfaces::reconcile::with_desktop_now(node.output.as_deref(), |desktop| of(desktop, node))
        .flatten()
        .unwrap_or_default()
}

/// Only the four together are written; "all" reads the largest and sets each, so the five rows never fight over the key.
fn four(holder: &impl Styled, four: &FourKeys, default: [f32; 4]) -> [RwSignal<f32>; 5] {
    let (read, write) = (four.read, four.write);
    let written = holder.style(
        four.name,
        four.key,
        move |style| read(style).unwrap_or(default),
        move |style, each: &[f32; 4]| write(style, *each),
    );
    let seed = written.peek();
    let each: [RwSignal<f32>; 4] = std::array::from_fn(|at| holder.shared(four.each[at], seed[at]));
    let all = holder.shared(four.all, largest(seed));
    for (at, one) in each.into_iter().enumerate() {
        effect(move || {
            let now = one.get();
            if written.peek()[at] != now {
                written.update(|four| four[at] = now);
            }
        });
    }
    effect(move || {
        let now = written.get();
        batch(|| {
            for (at, one) in each.into_iter().enumerate() {
                if one.peek() != now[at] {
                    one.set(now[at]);
                }
            }
            if all.peek() != largest(now) {
                all.set(largest(now));
            }
        });
    });
    effect(move || {
        let wanted = all.get();
        if wanted != largest(written.peek()) {
            written.set([wanted; 4]);
        }
    });
    [all, each[0], each[1], each[2], each[3]]
}

fn largest(four: [f32; 4]) -> f32 {
    four.into_iter().fold(0.0, f32::max)
}
