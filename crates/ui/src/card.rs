//! The builder every popout fills in.
//!
//! The shape itself is [`crate::popout_card::popout_card`]; this is the name its callers know it by, and the one-field-at-a-time way they say what goes in it. A module describes its card and hands it over — the surface decides the frame.

use telar::{Color, LayoutError, LayoutItem};

use util::reactive::{Live, fixed_text};

use crate::popout_card::{PopoutCardProps, popout_card};

pub type Card = PopoutCardProps;

impl Card {
    pub fn new(title: Live<String>) -> Self {
        Self::props().title(title).build()
    }

    pub fn titled(title: impl Into<String>) -> Self {
        Self::new(fixed_text(title))
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

    pub fn meter(mut self, fraction: Live<f32>, tint: Live<Color>) -> Self {
        self.meter = Some((fraction, tint));
        self
    }

    pub fn row(mut self, label: Live<String>, value: Live<String>) -> Self {
        self.rows.push((label, value));
        self
    }

    pub fn build(self) -> Result<Box<dyn LayoutItem>, LayoutError> {
        popout_card(self, telar::Children::default())
    }
}
