//! The one card the shell draws: a heading of icon, title and trailing reading over a body, in a box; a module describes its card and whoever places it decides the density.

use std::rc::Rc;

use telar::{
    AlignItems, Color, Container, LayoutError, LayoutItem, LayoutStyle, RectStyle, SizeDimension,
    StyledContainer, Text, box_item, use_theme,
};

use config::theme::{FontRole, NordTheme};
use util::reactive::{Live, fixed, fixed_text};

use crate::icon::icon_view;
use crate::panel::{content_radius, panel_fill};
use crate::scale::space;
use crate::widget;

type Built = Result<Box<dyn LayoutItem>, LayoutError>;

type Deferred = Box<dyn FnOnce(Parts) -> Built>;

/// How much room a card is given: a glance beside a chip, a section of a page, or a widget's whole footprint.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Density {
    Compact,
    Page,
    Widget,
}

impl Density {
    fn meter_height(self) -> f32 {
        match self {
            Density::Compact | Density::Widget => 6.0,
            Density::Page => 8.0,
        }
    }

    fn icon_size(self) -> f32 {
        match self {
            Density::Compact => 26.0,
            Density::Page | Density::Widget => 18.0,
        }
    }

    fn heading_gap(self) -> f32 {
        match self {
            Density::Compact => space::lg(),
            Density::Page | Density::Widget => space::md(),
        }
    }

    fn gap(self) -> f32 {
        match self {
            Density::Compact | Density::Widget => space::md(),
            Density::Page => 10.0,
        }
    }

    fn padding(self) -> f32 {
        match self {
            Density::Compact | Density::Page => space::xl(),
            Density::Widget => space::lg(),
        }
    }

    fn title_weight(self) -> Option<u16> {
        match self {
            Density::Compact => None,
            Density::Page | Density::Widget => Some(700),
        }
    }

    fn icon_tint(self, theme: NordTheme) -> Color {
        match self {
            Density::Compact => theme.text,
            Density::Page | Density::Widget => theme.subtle,
        }
    }

    /// A widget sits on the wallpaper or the lock's background, so it takes the translucent panel fill a popout does rather than the page's `base`, which is only distinct against a `surface` panel.
    fn fill(self, theme: NordTheme) -> Color {
        match self {
            Density::Compact | Density::Widget => panel_fill(),
            Density::Page => theme.base,
        }
    }
}

enum Part {
    Meter(Live<f32>, Live<Color>),
    Figure(Live<String>, Option<Live<Color>>),
    Row(Live<String>, Live<String>),
    Detail(Live<String>),
    Item(Deferred),
}

#[derive(Default)]
struct Frame {
    fill: Option<Color>,
    radius: Option<f32>,
    width: Option<f32>,
    on_hover: Option<Rc<dyn Fn(bool)>>,
}

/// A card described rather than built, so the same description can be shown at either density.
#[derive(Default)]
pub struct Card {
    icon: Option<Live<String>>,
    icon_tint: Option<Live<Color>>,
    title: Option<Live<String>>,
    subtitle: Option<Live<String>>,
    trailing: Option<Live<String>>,
    body: Vec<Part>,
    frame: Frame,
}

impl Card {
    pub fn new(title: Live<String>) -> Self {
        Self {
            title: Some(title),
            ..Self::default()
        }
    }

    pub fn titled(title: impl Into<String>) -> Self {
        Self::new(fixed_text(title))
    }

    /// A card with no heading, for a body that carries its own.
    pub fn bare() -> Self {
        Self::default()
    }

    pub fn icon(mut self, glyph: Live<String>) -> Self {
        self.icon = Some(glyph);
        self
    }

    pub fn icon_tint(mut self, tint: Live<Color>) -> Self {
        self.icon_tint = Some(tint);
        self
    }

    pub fn subtitle(mut self, text: Live<String>) -> Self {
        self.subtitle = Some(text);
        self
    }

    /// The headline reading, right-aligned against the title.
    pub fn trailing(mut self, value: Live<String>) -> Self {
        self.trailing = Some(value);
        self
    }

    pub fn meter(mut self, fraction: Live<f32>, tint: Live<Color>) -> Self {
        self.body.push(Part::Meter(fraction, tint));
        self
    }

    /// The reading a glance is for, at display size.
    pub fn figure(mut self, value: Live<String>) -> Self {
        self.body.push(Part::Figure(value, None));
        self
    }

    pub fn figure_tinted(mut self, value: Live<String>, tint: Live<Color>) -> Self {
        self.body.push(Part::Figure(value, Some(tint)));
        self
    }

    pub fn row(mut self, label: Live<String>, value: Live<String>) -> Self {
        self.body.push(Part::Row(label, value));
        self
    }

    /// A line of de-emphasised detail: the sentence a number needs to mean something.
    pub fn detail(mut self, text: Live<String>) -> Self {
        self.body.push(Part::Detail(text));
        self
    }

    /// Anything else the body holds, built when the card is.
    pub fn child(mut self, build: impl FnOnce() -> Built + 'static) -> Self {
        self.body.push(Part::Item(Box::new(move |_| build())));
        self
    }

    /// A child that arranges the card's own parts itself — a meter it wraps, rows in a list — drawn at the density the card is built at.
    pub fn composed(mut self, build: impl FnOnce(Parts) -> Built + 'static) -> Self {
        self.body.push(Part::Item(Box::new(build)));
        self
    }

    pub fn fill(mut self, fill: Color) -> Self {
        self.frame.fill = Some(fill);
        self
    }

    pub fn radius(mut self, radius: f32) -> Self {
        self.frame.radius = Some(radius);
        self
    }

    /// A fixed width the card keeps rather than shrinks from; without one it spans its container.
    pub fn width(mut self, width: f32) -> Self {
        self.frame.width = Some(width);
        self
    }

    /// Registers the box as a hover target, which is also what makes it take input.
    pub fn on_hover(mut self, hovered: impl Fn(bool) + 'static) -> Self {
        self.frame.on_hover = Some(Rc::new(hovered));
        self
    }

    pub fn build(self, density: Density) -> Result<Box<dyn LayoutItem>, LayoutError> {
        let theme = use_theme::<NordTheme>();
        let mut children: Vec<Box<dyn LayoutItem>> = Vec::with_capacity(self.body.len() + 1);
        if self.title.is_some() || self.icon.is_some() || self.trailing.is_some() {
            children.push(heading(
                density,
                theme,
                Heading {
                    icon: self.icon,
                    icon_tint: self.icon_tint,
                    title: self.title,
                    subtitle: self.subtitle,
                    trailing: self.trailing,
                },
            )?);
        }
        let parts = Parts { density, theme };
        for part in self.body {
            children.push(part.build(parts)?);
        }
        frame(self.frame, density, theme, children)
    }
}

impl Part {
    fn build(self, parts: Parts) -> Built {
        let theme = parts.theme;
        match self {
            Part::Meter(fraction, tint) => parts.meter(fraction, tint),
            Part::Figure(value, tint) => Ok(box_item(Text::new(
                move || value.get(),
                LayoutStyle::new(),
                move || {
                    let color = tint.as_ref().map_or(theme.text, |tint| tint.get());
                    theme
                        .text_style(FontRole::Display, color)
                        .with_font_weight(600)
                },
            )?)),
            Part::Row(label, value) => parts.row(label, value),
            Part::Detail(text) => Ok(box_item(Text::new(
                move || text.get(),
                LayoutStyle::new().width(SizeDimension::Percent(1.0)),
                move || theme.text_style(FontRole::Caption, theme.subtle),
            )?)),
            Part::Item(build) => build(parts),
        }
    }
}

/// The meter and the row a card draws, at the density it is being built at.
#[derive(Clone, Copy)]
pub struct Parts {
    density: Density,
    theme: NordTheme,
}

impl Parts {
    pub fn meter(self, fraction: Live<f32>, tint: Live<Color>) -> Built {
        widget::meter(
            fraction,
            tint,
            self.theme.overlay,
            self.density.meter_height(),
        )
    }

    pub fn row(self, label: Live<String>, value: Live<String>) -> Built {
        widget::label_value(
            label,
            value,
            self.theme.font(FontRole::Caption),
            self.theme.muted,
            self.theme.text,
        )
    }
}

fn frame(
    frame: Frame,
    density: Density,
    theme: NordTheme,
    children: Vec<Box<dyn LayoutItem>>,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let fill = frame.fill.unwrap_or_else(|| density.fill(theme));
    let radius = frame.radius.unwrap_or_else(content_radius);
    let style = LayoutStyle::new()
        .flex_column()
        .gap(density.gap())
        .padding_all(density.padding());
    let style = match frame.width {
        Some(width) => style.width(width).flex_shrink(0.0),
        None => style.width(SizeDimension::Percent(1.0)),
    };
    let style = match density {
        Density::Widget => style.height(SizeDimension::Percent(1.0)),
        Density::Compact | Density::Page => style,
    };
    let card = crate::layout::painted_chrome(
        StyledContainer::new(style, move |_| RectStyle::filled(fill, radius), children)?,
        fill,
    );
    Ok(match frame.on_hover {
        Some(hovered) => Box::new(card.on_hover(move |inside| hovered(inside))),
        None => Box::new(card),
    })
}

struct Heading {
    icon: Option<Live<String>>,
    icon_tint: Option<Live<Color>>,
    title: Option<Live<String>>,
    subtitle: Option<Live<String>>,
    trailing: Option<Live<String>>,
}

fn heading(
    density: Density,
    theme: NordTheme,
    parts: Heading,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let weighted = move |role: FontRole, color: Color| {
        let style = theme.text_style(role, color);
        match density.title_weight() {
            Some(weight) => style.with_font_weight(weight),
            None => style,
        }
    };

    let mut row: Vec<Box<dyn LayoutItem>> = Vec::with_capacity(3);
    if let Some(glyph) = parts.icon {
        let tint = parts.icon_tint;
        let fallback = density.icon_tint(theme);
        row.push(icon_view(
            move || glyph.get(),
            move || tint.as_ref().map_or(fallback, |t| t.get()),
            density.icon_size(),
        )?);
    }

    let mut label: Vec<Box<dyn LayoutItem>> = Vec::with_capacity(2);
    if let Some(title) = parts.title {
        label.push(box_item(Text::new(
            move || title.get(),
            LayoutStyle::new(),
            move || weighted(FontRole::Title, theme.text),
        )?));
    }
    if let Some(line) = parts.subtitle {
        label.push(box_item(Text::new(
            move || line.get(),
            LayoutStyle::new(),
            move || theme.text_style(FontRole::Body, theme.subtle),
        )?));
    }
    row.push(Box::new(Container::new(
        LayoutStyle::new()
            .flex_column()
            .flex_grow(1.0)
            .gap(space::xs()),
        label,
    )?));

    if let Some(value) = parts.trailing {
        row.push(box_item(Text::new(
            move || value.get(),
            LayoutStyle::new().flex_shrink(0.0),
            move || weighted(FontRole::Title, theme.accent),
        )?));
    }

    Ok(Box::new(Container::new(
        LayoutStyle::new()
            .flex_row()
            .align_items(AlignItems::CENTER)
            .gap(density.heading_gap())
            .width(SizeDimension::Percent(1.0)),
        row,
    )?))
}

fn sample(density: Density) -> Card {
    let theme = use_theme::<NordTheme>();
    let card = Card::titled("Volume")
        .icon(fixed_text("volume-2"))
        .meter(fixed(0.64), fixed(theme.accent))
        .row(fixed_text("Device"), fixed_text("Built-in Audio"));
    match density {
        Density::Compact => card.subtitle(fixed_text("64%")),
        Density::Page | Density::Widget => card
            .trailing(fixed_text("64%"))
            .detail(fixed_text("Muted on headphones")),
    }
}

pub(crate) fn compact_preview() -> Result<Box<dyn LayoutItem>, LayoutError> {
    sample(Density::Compact)
        .width(264.0)
        .build(Density::Compact)
}

pub(crate) fn page_preview() -> Result<Box<dyn LayoutItem>, LayoutError> {
    sample(Density::Page).build(Density::Page)
}
