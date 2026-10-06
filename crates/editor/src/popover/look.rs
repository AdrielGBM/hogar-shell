//! The look rows of every popover, one implementation over whatever holds a style ([`Styled`]).

use std::rc::Rc;

use telar::{LayoutItem, Rect, RwSignal, batch, effect};

use layout::{Border, Corners, Sides, Style};
use surfaces::reconcile::Desktop;
use surfaces::rects::Node;
use ui::descriptor::Built;

use super::area::help;
use super::draft::{AreaDraft, GroupDraft, InstanceDraft};
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
            help("Style", "fill"),
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
            help("Style", "opacity"),
            opacity,
            Range::new(faintest, 1.0, 0.05),
        )?,
    )
}

pub(crate) fn radius(holder: &impl Styled) -> Built {
    let most = most_in(holder, handles::most_radius_in, ROUNDEST);
    let [all, corners @ ..] = four(
        holder,
        FourKeys {
            name: "style.radius.corners",
            key: "style.radius",
            all: "style.radius",
            each: [
                "style.radius.top_left",
                "style.radius.top_right",
                "style.radius.bottom_right",
                "style.radius.bottom_left",
            ],
            default: drawn(holder, crate::tools::target::radius_of),
            read: |style| style.radius.map(Corners::to_array),
            write: |style, [a, b, c, d]| style.radius = Some(Corners::each(a, b, c, d)),
        },
    );
    let range = Range::whole(0.0, most);
    let mut list = vec![rows::number(
        label!("editor.look.radius"),
        help("Style", "radius"),
        all,
        range,
    )?];
    for (corner, label) in corners.into_iter().zip(corner_labels()) {
        list.push(rows::number(label, None, corner, range)?);
    }
    holder.marked(&["style.radius"], rows::together(list)?)
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
    let [all, sides @ ..] = four(
        holder,
        FourKeys {
            name: "style.padding.sides",
            key: "style.padding",
            all: "style.padding",
            each: [
                "style.padding.top",
                "style.padding.right",
                "style.padding.bottom",
                "style.padding.left",
            ],
            default: drawn(holder, crate::tools::target::padding_of),
            read: |style| style.padding.map(Sides::to_array),
            write: |style, [a, b, c, d]| style.padding = Some(Sides::each(a, b, c, d)),
        },
    );
    let labels = [
        label!("editor.look.top"),
        label!("editor.look.right"),
        label!("editor.look.bottom"),
        label!("editor.look.left"),
    ];
    let range = Range::whole(0.0, most);
    let mut list = vec![rows::number(
        label!("editor.look.padding"),
        help("Style", "padding"),
        all,
        range,
    )?];
    for (side, label) in sides.into_iter().zip(labels) {
        list.push(rows::number(label, None, side, range)?);
    }
    holder.marked(&["style.padding"], rows::together(list)?)
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
                help("Style", "shadow"),
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
struct FourKeys {
    name: &'static str,
    key: &'static str,
    all: &'static str,
    each: [&'static str; 4],
    default: [f32; 4],
    read: fn(&Style) -> Option<[f32; 4]>,
    write: fn(&mut Style, [f32; 4]),
}

/// What the screen draws as the four where nothing writes them, so a row reads what the canvas shows.
fn drawn(holder: &impl Styled, of: fn(&Desktop, &Node) -> Option<[f32; 4]>) -> [f32; 4] {
    let node = holder.node();
    surfaces::reconcile::desktop_now(node.output.as_deref())
        .and_then(|desktop| of(&desktop, node))
        .unwrap_or_default()
}

/// Only the four together are written; "all" reads the largest and sets each, so the five rows never fight over the key.
fn four(holder: &impl Styled, four: FourKeys) -> [RwSignal<f32>; 5] {
    let (read, write, default) = (four.read, four.write, four.default);
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
