[logic]
use crate::icon_glyph::{icon_glyph, IconGlyphProps};
use ::config::theme::{FontRole, NordTheme};
use ::util::reactive::{Live, fixed, fixed_text};
use crate::meter::{MeterProps, meter};

/// One popout's content, described rather than built.
///
/// A popout is a glance, not a panel: a heading, at most one meter, and a handful of label/value rows. Saying that once — as data a module fills in rather than a tree each module builds — is what keeps twelve of them looking like one shell instead of twelve small designs. [`crate::card::Card`] is this struct under the name its builders use.
pub struct Props {
    #[props(some)]
    pub icon: Option<Live<String>> = None,
    #[props(some)]
    pub icon_tint: Option<Live<Color>> = None,
    pub title: Live<String> = fixed_text(""),
    #[props(some)]
    pub subtitle: Option<Live<String>> = None,
    // Only one meter per card: a popout with two is a dashboard card, and this is not the surface for one.
    #[props(some)]
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

let bar = props.meter;
let track = theme.overlay;

[view]
col width:100% gap:crate::scale::space::md()
    row width:100% gap:crate::scale::space::lg() align:center
        match icon
            Some(glyph)
                icon_glyph name:(Reactive::of(move || glyph.get())) tint:(Reactive::of(move || icon_tint.as_ref().map(|t| t.get()).unwrap_or(ink))) size:HEADER_ICON
            None
        col grow:1 gap:crate::scale::space::xs()
            text "{$title}" font_size:heading color:theme.text
            match subtitle
                Some(line)
                    text "{$line}" color:theme.subtle
                None
    match bar
        Some((fraction, tint))
            meter fraction:fraction tint:tint track:track height:METER_HEIGHT
        None
    for (label, value) in rows
        row width:100% gap:crate::scale::space::lg() align:center justify:between font_size:caption
            text "{$label}" color:theme.muted shrink:0
            text "{$value}" color:theme.text

[preview "Popout card"]
popout_card title:(fixed_text("Volume")) subtitle:(fixed_text("64%")) icon:(fixed_text("audio-volume-high")) meter:((fixed(0.64), fixed(use_theme::<NordTheme>().accent))) rows:(vec![(fixed_text("Device"), fixed_text("Built-in Audio"))])
