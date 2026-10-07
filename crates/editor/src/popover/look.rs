//! The look rows of every popover, one implementation over whatever holds a style ([`Styled`]).

use std::rc::Rc;
use std::sync::Arc;

use telar::{
    Color, LayoutError, LayoutItem, LayoutStyle, ReactiveList, Rect, RwSignal, Text, batch,
    box_item, effect, use_theme,
};

use config::Config;
use config::theme::{FontRole, NordTheme};

use layout::restyle::{bar_style, restyle_bar};
use layout::{
    Border, Corners, GroupId, InstanceId, LayoutOp, ResolvedArea, ResolvedAreaKind, Sides, Style,
};
use surfaces::look::{Holder, draws_plate, fill_of, resting_opacity};
use surfaces::reconcile::Desktop;
use surfaces::rects::{Node, Part};
use ui::descriptor::Built;

use crate::written::Written;

use super::area::help;
use super::draft::{AreaDraft, GroupDraft, InstanceDraft, group_entry};
use super::handles;
use super::origin::Provenance;
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

    /// The signal [`Styled::style`] made for `name` already, where a holder makes a new one each time it is asked.
    fn current<T: Clone + PartialEq + 'static>(
        &self,
        name: &'static str,
        key: &'static str,
        read: impl Fn(&Style) -> T + 'static,
        write: impl Fn(&mut Style, &T) + 'static,
    ) -> RwSignal<T> {
        self.style(name, key, read, write)
    }

    /// What a fill of this holder is drawn as and laid over, read off the screen it is on.
    fn backing(&self) -> Option<Backing>;

    /// Whether some level writes `key`, what the popover has changed included. Reactive.
    fn names(&self, key: &'static str) -> bool;

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

    fn backing(&self) -> Option<Backing> {
        Some(Backing::new(
            Arc::clone(&self.config),
            Rc::clone(&self.resolved),
            Held::Area,
        ))
    }

    fn names(&self, key: &'static str) -> bool {
        self.writes(&[key]) || self.provenance(&[key]) != Provenance::Default
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

    fn backing(&self) -> Option<Backing> {
        Some(Backing::new(
            Arc::clone(&self.area.config),
            Rc::clone(&self.area.resolved),
            Held::Group,
        ))
    }

    fn names(&self, key: &'static str) -> bool {
        self.writes(&[key]) || self.provenance(&[key]) != Provenance::Default
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

    fn current<T: Clone + PartialEq + 'static>(
        &self,
        name: &'static str,
        key: &'static str,
        read: impl Fn(&Style) -> T + 'static,
        write: impl Fn(&mut Style, &T) + 'static,
    ) -> RwSignal<T> {
        InstanceDraft::shared(self, name).unwrap_or_else(|| self.style(name, key, read, write))
    }

    fn backing(&self) -> Option<Backing> {
        let Part::Instance(group, _) = &self.node.part else {
            return None;
        };
        surfaces::reconcile::with_desktop_now(self.node.output.as_deref(), |desktop| {
            let area = desktop.resolved.area(self.node.layer, &self.node.area)?;
            let in_bar = matches!(area.kind, ResolvedAreaKind::Bar { .. });
            let plate = area
                .groups
                .iter()
                .find(|held| held.id == *group)
                .filter(|held| draws_plate(held, in_bar))
                .map(|held| held.style.clone());
            Some(Backing::new(
                Arc::clone(&desktop.config),
                Rc::new(area.clone()),
                Held::Instance { plate },
            ))
        })
        .flatten()
    }

    fn names(&self, key: &'static str) -> bool {
        self.writes(&[key]) || self.provenance(&[key]) != Provenance::Default
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

    fn backing(&self) -> Option<Backing> {
        self.0.backing()
    }

    fn names(&self, key: &'static str) -> bool {
        self.0.names(on_bar(key))
    }
}

fn on_bar(key: &'static str) -> &'static str {
    layout::restyle::bar_key(key)
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

fn read_fill(style: &Style) -> String {
    style.fill.clone().unwrap_or_default()
}

fn write_fill(style: &mut Style, token: &str) {
    style.fill = (!token.is_empty()).then(|| token.to_string());
}

pub(crate) fn fill(holder: &impl Styled) -> Built {
    let fill = holder.style(
        "style.fill",
        "style.fill",
        read_fill,
        |style, token: &String| write_fill(style, token),
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
    let backing = holder.backing().map(Rc::new);
    let theme: NordTheme = use_theme();
    let resting = backing.clone();
    let opacity = holder.style(
        "style.opacity",
        "style.opacity",
        move |style| {
            style.opacity.unwrap_or_else(|| {
                resting.as_ref().map_or(1.0, |backing| {
                    backing.resting_opacity(style.fill.as_deref(), &theme)
                })
            })
        },
        |style, value: &f32| style.opacity = Some(*value),
    );
    let row = holder.marked(
        &["style.opacity"],
        rows::number(
            label!("editor.look.opacity"),
            holder.documented("opacity"),
            opacity,
            Range::new(faintest, 1.0, 0.05),
        )?,
    )?;
    match (holder.area_kind(), backing) {
        ("prompt", _) | (_, None) => Ok(row),
        (_, Some(backing)) => rows::together(vec![row, contrast_row(holder, opacity, backing)?]),
    }
}

/// How readable the theme's text is on the fill the rows make, live, and a warning under [`config::scheme::MIN_TEXT_CONTRAST`]. Nothing is refused here: only the lock's prompt has to be read.
fn contrast_row(holder: &impl Styled, opacity: RwSignal<f32>, backing: Rc<Backing>) -> Built {
    let fill = holder.current(
        "style.fill",
        "style.fill",
        read_fill,
        |style, token: &String| write_fill(style, token),
    );
    let naming = holder.clone();
    let ratio = move || {
        let (token, opacity) = (fill.get(), opacity.get());
        if token.is_empty() {
            return None;
        }
        let style = Style {
            fill: Some(token),
            opacity: naming.names("style.opacity").then_some(opacity),
            ..Style::default()
        };
        backing.ratio(&style, &use_theme())
    };
    let present = ratio.clone();
    let shown = ReactiveList::with_style(
        LayoutStyle::new(),
        move || present().into_iter().map(|_| true).collect::<Vec<_>>(),
        |built: &bool| *built,
        move |_| {
            let (said, tinted) = (ratio.clone(), ratio.clone());
            Ok(box_item(Text::declaring(
                move || contrast_said(said().unwrap_or_default()),
                LayoutStyle::new(),
                move |inherited| {
                    let theme: NordTheme = use_theme();
                    let tint = match tinted()
                        .is_some_and(|ratio| !config::scheme::readable_ratio(ratio))
                    {
                        true => theme.warning,
                        false => theme.subtle,
                    };
                    theme.text_over(inherited, FontRole::Caption, tint)
                },
            )?))
        },
    )?;
    Ok(Box::new(shown))
}

/// What a fill of a holder is drawn as, through the paint its surface draws it with ([`fill_of`]), and what it is laid over: the area's own box under a group's plate, and that plate too under an instance where its group draws one. The bottom is the layer's base, since a wallpaper, a picture or a sibling is not known here.
pub(crate) struct Backing {
    config: Arc<Config>,
    area: Rc<ResolvedArea>,
    held: Held,
}

pub(crate) enum Held {
    Area,
    Group,
    /// An instance, over its group's plate where the group draws one.
    Instance {
        plate: Option<Style>,
    },
}

impl Backing {
    pub(crate) fn new(config: Arc<Config>, area: Rc<ResolvedArea>, held: Held) -> Self {
        Self { config, area, held }
    }

    fn holder(&self) -> Holder<'_> {
        match self.held {
            Held::Area => Holder::Area(&self.area),
            Held::Group => Holder::Plate,
            Held::Instance { .. } => Holder::Instance(&self.area),
        }
    }

    /// The contrast of the theme's text on what the holder draws with `style`, or `None` where that names nothing its box paints.
    pub(crate) fn ratio(&self, style: &Style, theme: &NordTheme) -> Option<f32> {
        let fill = fill_of(self.holder(), style, &self.config, theme)?;
        Some(layout::text_contrast(fill, self.under(theme), theme))
    }

    pub(crate) fn resting_opacity(&self, fill: Option<&str>, theme: &NordTheme) -> f32 {
        resting_opacity(self.holder(), fill, &self.config, theme)
    }

    fn under(&self, theme: &NordTheme) -> Color {
        let plate = match &self.held {
            Held::Area => return theme.base,
            Held::Group => None,
            Held::Instance { plate } => plate.as_ref(),
        };
        let area = self.over(
            Holder::Area(&self.area),
            &self.area.style,
            theme.base,
            theme,
        );
        plate.map_or(area, |plate| self.over(Holder::Plate, plate, area, theme))
    }

    fn over(&self, holder: Holder<'_>, style: &Style, under: Color, theme: &NordTheme) -> Color {
        fill_of(holder, style, &self.config, theme)
            .map_or(under, |fill| layout::laid_over(fill, under))
    }
}

pub(crate) fn contrast_said(ratio: f32) -> String {
    let shown = format!("{ratio:.1}");
    match config::scheme::readable_ratio(ratio) {
        true => telar::t!("editor.look.contrast", ratio = shown),
        false => telar::t!(
            "editor.look.contrast_low",
            ratio = shown,
            least = config::scheme::MIN_TEXT_CONTRAST.to_string()
        ),
    }
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
