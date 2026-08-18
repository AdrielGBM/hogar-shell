//! The shape every dashboard card takes.
//!
//! A dashboard card is a page section, not a glance: a heading, an optional readout on the right, and whatever
//! the card puts under it. Saying the frame once — as data a page fills in rather than a tree each page builds
//! — is what keeps eight of them reading as one page instead of eight small designs.
//!
//! The card sits on the *base* token while the panel around it is *surface*, which is the pairing every theme
//! guarantees is distinct in both directions: an inset well on a dark palette, a raised one on a light palette,
//! never a card that vanishes into its panel.

use ui::scale::space;
use telar::{
    AlignItems, Color, Container, LayoutError, LayoutItem, LayoutStyle, RectStyle, SizeDimension,
    StyledContainer, Text, box_item,
};

use config::theme::{FontRole, NordTheme};
use surfaces::drawer::content_radius;
use ui::icon::icon_view;
use util::reactive::{Live, fixed_text};

const HEADING_ICON: f32 = 18.0;
/// A page card's meter is thicker than a popout's: it is the card's subject, not a footnote under a heading.
pub const METER_HEIGHT: f32 = 8.0;
/// Tall enough that a minute of history reads as a shape rather than a jagged line, short enough that six of
/// them stack inside one drawer.
pub const CHART_HEIGHT: f32 = 40.0;
pub const CARD_GAP: f32 = 10.0;

pub struct Card {
    icon: Option<Live<String>>,
    icon_tint: Option<Live<Color>>,
    title: Live<String>,
    /// The card's headline number, right-aligned against the title — the one thing a glance is for.
    trailing: Option<Live<String>>,
    body: Vec<Box<dyn LayoutItem>>,
}

impl Card {
    pub fn new(title: Live<String>) -> Self {
        Self {
            icon: None,
            icon_tint: None,
            title,
            trailing: None,
            body: Vec::new(),
        }
    }

    pub fn titled(title: impl Into<String>) -> Self {
        Self::new(fixed_text(title))
    }

    pub fn icon(mut self, glyph: impl Into<String>) -> Self {
        self.icon = Some(fixed_text(glyph));
        self
    }

    pub fn live_icon(mut self, glyph: Live<String>) -> Self {
        self.icon = Some(glyph);
        self
    }

    pub fn icon_tint(mut self, tint: Live<Color>) -> Self {
        self.icon_tint = Some(tint);
        self
    }

    pub fn trailing(mut self, value: Live<String>) -> Self {
        self.trailing = Some(value);
        self
    }

    pub fn child(mut self, item: Box<dyn LayoutItem>) -> Self {
        self.body.push(item);
        self
    }

    pub fn build(self, theme: NordTheme) -> Result<Box<dyn LayoutItem>, LayoutError> {
        let mut children: Vec<Box<dyn LayoutItem>> = Vec::with_capacity(self.body.len() + 1);
        children.push(heading(
            self.icon,
            self.icon_tint,
            self.title,
            self.trailing,
            theme,
        )?);
        children.extend(self.body);
        frame(children, theme)
    }
}

/// The card's own box, at the same corner radius the bar carries so a drawer full of cards rounds like the
/// shell around it.
pub fn frame(
    children: Vec<Box<dyn LayoutItem>>,
    theme: NordTheme,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let radius = content_radius();
    Ok(Box::new(StyledContainer::new(
        LayoutStyle::new()
            .flex_column()
            .gap(CARD_GAP)
            .padding_all(space::xl())
            .width(SizeDimension::Percent(1.0)),
        move |_r| RectStyle::filled(theme.base, radius),
        children,
    )?))
}

fn heading(
    icon: Option<Live<String>>,
    tint: Option<Live<Color>>,
    title: Live<String>,
    trailing: Option<Live<String>>,
    theme: NordTheme,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let mut row: Vec<Box<dyn LayoutItem>> = Vec::with_capacity(3);
    if let Some(glyph) = icon {
        row.push(icon_view(
            move || glyph.get(),
            move || tint.as_ref().map(|t| t.get()).unwrap_or(theme.subtle),
            HEADING_ICON,
        )?);
    }
    row.push(box_item(Text::auto(
        move || title.get(),
        LayoutStyle::new().flex_grow(1.0),
        move || {
            theme
                .text_style(FontRole::Title, theme.text)
                .with_weight(700)
        },
    )?));
    if let Some(value) = trailing {
        row.push(box_item(Text::auto(
            move || value.get(),
            LayoutStyle::new().flex_shrink(0.0),
            move || {
                theme
                    .text_style(FontRole::Title, theme.accent)
                    .with_weight(700)
            },
        )?));
    }
    Ok(Box::new(Container::new(
        LayoutStyle::new()
            .flex_row()
            .align_items(AlignItems::CENTER)
            .gap(space::md())
            .width(SizeDimension::Percent(1.0)),
        row,
    )?))
}

/// A line of de-emphasised detail under a card's subject — the sentence a number needs to mean something.
pub fn detail(text: Live<String>, theme: NordTheme) -> Result<Box<dyn LayoutItem>, LayoutError> {
    Ok(box_item(Text::auto(
        move || text.get(),
        LayoutStyle::new().width(SizeDimension::Percent(1.0)),
        move || theme.text_style(FontRole::Caption, theme.subtle),
    )?))
}

/// A column of cards, which is what every page is.
pub fn page(cards: Vec<Box<dyn LayoutItem>>) -> Result<Box<dyn LayoutItem>, LayoutError> {
    Ok(Box::new(Container::new(
        LayoutStyle::new()
            .flex_column()
            .gap(space::lg())
            .width(SizeDimension::Percent(1.0)),
        cards,
    )?))
}
