[logic]
use crate::mode::{self, icon_of, name_of};
use crate::switcher::{switcher, SwitcherProps};
use ::config::theme::FontRole;
use ::layout::LayerKind;
use ::ui::chrome::panel_fill;
use ::ui::icon_glyph::{icon_glyph, IconGlyphProps};

/// The mode strip: which layer is being edited and on which screen, the other modes a press away, the keys the mode answers, and the one explicit way out (TA-4, F-7). Where lock mode cannot offer tools, why it cannot sits under it instead.
pub struct Props {
    pub layer: LayerKind = LayerKind::Desktop,
    #[props(into)]
    pub output: String = String::new(),
    pub refused: Option<String> = None,
}

/// How wide the key list's column of chords is.
const CHORDS: f32 = 200.0;

fn done_label() -> String {
    telar::t!("editor.done")
}

fn keys_label() -> String {
    telar::t!("editor.keys.title")
}

let layer = props.layer;
let output = props.output;
let refused = signal(props.refused);
let help = crate::keys::help();
let icon = icon_of(layer);
let said = telar::t!("editor.editing", layer = name_of(layer), output = output.clone());
let rad = ::ui::scale::corner::md();

[view]
col align:center gap:(::ui::scale::space::xs()) pad_x:(::ui::scale::space::lg()) pad_y:(::ui::scale::space::sm()) fill:panel_fill() radius:rad input_opaque label:said
    row align:center gap:(::ui::scale::space::md())
        icon_glyph name:(Reactive::of(move || icon.to_string())) tint:(Reactive::of(move || use_theme::<::config::theme::NordTheme>().accent)) size:18
        text "{name_of(layer)}" color:$theme.text font_size:$theme.font(FontRole::Body)
        text "{output}" color:$theme.subtle font_size:$theme.font(FontRole::Caption)
        box pad_x:(::ui::scale::space::md()) pad_y:(::ui::scale::space::xs()) radius:rad hover_style(fill:$theme.overlay) label:(keys_label()) on_press:(move || help.update(|shown| *shown = !*shown))
            text "{keys_label()}" color:$theme.text font_size:$theme.font(FontRole::Body)
        box pad_x:(::ui::scale::space::md()) pad_y:(::ui::scale::space::xs()) fill:$theme.accent radius:rad hover_style(fill:$theme.accent.darken(0.08)) on_press:(|| { mode::leave(); })
            text "{done_label()}" color:$theme.base font_size:$theme.font(FontRole::Body)
    switcher current:layer
    if $refused.is_some()
        text "{$refused.unwrap_or_default()}" color:$theme.warning font_size:$theme.font(FontRole::Caption)
    if $help
        col gap:(::ui::scale::space::xs())
            for line in crate::keys::help_rows(layer)
                row gap:(::ui::scale::space::md())
                    box width:CHORDS
                        text "{&line.keys}" color:$theme.subtle font_size:$theme.font(FontRole::Caption)
                    text "{&line.what}" color:$theme.text font_size:$theme.font(FontRole::Caption)
