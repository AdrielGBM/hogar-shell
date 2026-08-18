[logic]
use ::config::theme::{FontRole, NordTheme};
use ::util::reactive::{Live, fixed, fixed_text};
use crate::widget;

/// One popout's content, described rather than built.
///
/// A popout is a glance, not a panel: a heading, at most one meter, and a handful of label/value rows. Saying
/// that once — as data a module fills in rather than a tree each module builds — is what keeps twelve of them
/// looking like one shell instead of twelve small designs. [`crate::card::Card`] is this struct under the name
/// its builders use.
pub struct Props {
    pub icon: Option<Live<String>> = None,
    pub icon_tint: Option<Live<Color>> = None,
    pub title: Live<String> = fixed_text(""),
    pub subtitle: Option<Live<String>> = None,
    // Only one meter per card: a popout with two is a dashboard card, and this is not the surface for one.
    pub meter: Option<(Live<f32>, Live<Color>)> = None,
    pub rows: Vec<(Live<String>, Live<String>)> = Vec::new(),
}

const HEADER_ICON: f32 = 26.0;
const METER_HEIGHT: f32 = 6.0;

let theme = use_theme::<NordTheme>();
let heading = theme.font(FontRole::Title);
let caption = theme.font(FontRole::Caption);
let ink = theme.text;

let icon = props.icon;
let icon_tint = props.icon_tint;
let title = props.title;
let subtitle = props.subtitle;
let rows = props.rows;

// The bar grows from its left edge by transforming a filled child against its own laid-out rect, which is the
// one piece of a card no attribute reaches: a declarative scale pivots on the centre.
let bar = props
    .meter
    .map(|(fraction, tint)| widget::meter(fraction, tint, theme.overlay, METER_HEIGHT))
    .transpose()?;

[view]
col width:100% gap(crate::scale::space::md())
    row width:100% gap(crate::scale::space::lg()) align:center
        match icon
            Some(glyph)
                icon_glyph name(move || glyph.get()) tint(move || icon_tint.as_ref().map(|t| t.get()).unwrap_or(ink)) size:HEADER_ICON
            None
        col grow:1 gap(crate::scale::space::xs())
            text "{$title}" size:heading color:text
            match subtitle
                Some(line)
                    text "{$line}" size:caption color:subtle
                None
    match bar
        Some(bar)
            widget "bar"
        None
    for (label, value) in rows
        row width:100% gap(crate::scale::space::lg()) align:center justify:between
            text "{$label}" size:caption color:muted shrink:0
            text "{$value}" size:caption color:text

[preview "Popout card"]
popout_card title:(fixed_text("Volume")) subtitle:(fixed_text("64%")) icon:(fixed_text("audio-volume-high")) meter:((fixed(0.64), fixed(use_theme::<NordTheme>().accent))) rows:(vec![(fixed_text("Device"), fixed_text("Built-in Audio"))])
