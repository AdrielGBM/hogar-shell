//! The rows a popover is made of: the shell's form rows, each explained by the doc comment of what it edits.

use std::rc::Rc;

use telar::{
    Children, LayoutError, LayoutItem, LayoutStyle, Reactive, RwSignal, Text, box_item, use_theme,
};

use config::theme::{FontRole, NordTheme};
use ui::descriptor::Built;
use ui::form::enum_row::{EnumRowProps, enum_row};
use ui::form::scrub_row::{ScrubRowProps, scrub_row};
use ui::form::segmented_row::{SegmentedRowProps, segmented_row};
use ui::form::swatch_row::{SwatchRowProps, swatch_row};
use ui::form::text_row::{TextRowProps, text_row};
use ui::form::toggle_row::{ToggleRowProps, toggle_row};

/// A picker shows every option at once up to this many, and a dropdown past it.
pub const SEGMENTS: usize = 4;

/// A number with no more steps than this between its bounds gets a − and + beside it as well.
const COUNTABLE: f32 = 12.0;

/// How a number is scrubbed: its bounds, its step, and whether it is whole.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Range {
    pub min: f32,
    pub max: f32,
    pub step: f32,
    pub integer: bool,
}

impl Range {
    pub const fn new(min: f32, max: f32, step: f32) -> Self {
        Self {
            min,
            max,
            step,
            integer: false,
        }
    }

    pub const fn whole(min: f32, max: f32) -> Self {
        Self {
            min,
            max,
            step: 1.0,
            integer: true,
        }
    }

    fn countable(&self) -> bool {
        self.integer
            && self.min.is_finite()
            && self.max.is_finite()
            && (self.max - self.min) / self.step <= COUNTABLE
    }
}

/// `row` with `help` shown beside it while the pointer rests on it.
pub fn explained(label: Reactive<String>, help: Option<String>, row: Box<dyn LayoutItem>) -> Built {
    let Some(help) = help.filter(|help| !help.trim().is_empty()) else {
        return Ok(row);
    };
    let mut slots = telar::Slots::new();
    slots.push(None, row);
    telar::tooltip(
        telar::TooltipProps::props()
            .text(label)
            .description(Reactive::of(move || help.clone()))
            .stretch(true)
            .build(),
        Children::from(slots),
    )
}

pub fn number(
    label: Reactive<String>,
    help: Option<String>,
    value: RwSignal<f32>,
    range: Range,
) -> Built {
    let row = scrub_row(
        ScrubRowProps::props()
            .label(label.clone())
            .value(value)
            .min(range.min)
            .max(range.max)
            .step(range.step)
            .integer(range.integer)
            .stepper(range.countable())
            .build(),
        Children::default(),
    )?;
    explained(label, help, row)
}

pub fn toggle(label: Reactive<String>, help: Option<String>, value: RwSignal<bool>) -> Built {
    let row = toggle_row(
        ToggleRowProps::props()
            .label(label.clone())
            .value(value)
            .build(),
        Children::default(),
    )?;
    explained(label, help, row)
}

/// One of `options`, all of them shown at once when there are few enough, else in a dropdown.
pub fn choice(
    label: Reactive<String>,
    help: Option<String>,
    value: RwSignal<String>,
    options: Rc<[&'static str]>,
) -> Built {
    let row = match options.len() <= SEGMENTS {
        true => segmented_row(
            SegmentedRowProps::props()
                .label(label.clone())
                .value(value)
                .options(options)
                .build(),
            Children::default(),
        )?,
        false => enum_row(
            EnumRowProps::props()
                .label(label.clone())
                .value(value)
                .options(options)
                .build(),
            Children::default(),
        )?,
    };
    explained(label, help, row)
}

/// One of the theme's colours, by name.
pub fn colour(
    label: Reactive<String>,
    help: Option<String>,
    value: RwSignal<String>,
    tokens: Rc<[&'static str]>,
) -> Built {
    let row = swatch_row(
        SwatchRowProps::props()
            .label(label.clone())
            .value(value)
            .tokens(tokens)
            .build(),
        Children::default(),
    )?;
    explained(label, help, row)
}

pub fn text(label: Reactive<String>, help: Option<String>, value: RwSignal<String>) -> Built {
    let row = text_row(
        TextRowProps::props()
            .label(label.clone())
            .value(value)
            .build(),
        Children::default(),
    )?;
    explained(label, help, row)
}

/// A line of quiet text under the rows it is about.
pub fn note(said: impl Fn() -> String + 'static) -> Built {
    let theme = use_theme::<NordTheme>();
    Ok(box_item(Text::new(said, LayoutStyle::new(), move || {
        theme.text_style(FontRole::Caption, theme.subtle)
    })?))
}

/// The heading of a run of rows.
pub fn heading(said: impl Fn() -> String + 'static) -> Built {
    let theme = use_theme::<NordTheme>();
    Ok(box_item(Text::new(said, LayoutStyle::new(), move || {
        theme
            .text_style(FontRole::Caption, theme.muted)
            .with_font_weight(700)
    })?))
}

pub(crate) type Rows = Result<Vec<Box<dyn LayoutItem>>, LayoutError>;
