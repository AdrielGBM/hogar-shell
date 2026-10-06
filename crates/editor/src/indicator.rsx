[logic]
use crate::mode::{self, icon_of, name_of};
use crate::switcher::{switcher, SwitcherProps};
use ::config::theme::FontRole;
use ::layout::LayerKind;
use ::ui::chrome::panel_fill;
use ::ui::icon_glyph::{icon_glyph, IconGlyphProps};
use std::cell::Cell;
use std::rc::Rc;

/// The mode strip: a grip to drag it out of the way, which layer is being edited — pressed, every layer to switch to — and on which screen, and while edits apply to one workspace alone, which one; the mode's `+`, the history to jump through, the strip's actions, the keys the mode answers and the one explicit way out (TA-4, F-7). Under it, the other modes a press away, then what the last thing asked of the mode did or why it was not done, and, where lock mode cannot offer tools, why it cannot.
pub struct Props {
    pub layer: LayerKind = LayerKind::Desktop,
    #[props(into)]
    pub output: String = String::new(),
    pub refused: Option<String> = None,
}

/// How wide the key list's column of chords is.
const CHORDS: f32 = 200.0;

/// What the strip has open under it, one at a time.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Open {
    Layers,
    History,
}

fn done_label() -> String {
    telar::t!("editor.done")
}

fn keys_label() -> String {
    telar::t!("editor.keys.title")
}

fn history_label() -> String {
    format!("{} ▸", telar::t!("editor.history.title"))
}

/// A layer as the strip lists it to switch to, the one being edited ticked.
fn layer_line(each: LayerKind, current: LayerKind) -> String {
    mode::marked(each == current, "✓ ", &name_of(each))
}

let layer = props.layer;
let output = props.output;
let refused = signal(props.refused);
let help = crate::keys::help();
let icon = icon_of(layer);
let said = telar::t!("editor.editing", layer = name_of(layer), output = output.clone());
let rad = ::ui::scale::corner::md();
let variant = memo(move || crate::variant::workspace().map(|workspace| telar::t!("editor.variant.editing", workspace = workspace.0)).unwrap_or_default());
let refusal = memo(move || mode::refusal().get().unwrap_or_default());
let confirmation = memo(move || mode::confirmation().get().unwrap_or_default());
let add = crate::host::add_of(layer);
let actions = crate::host::strip_actions();
let open: RwSignal<Option<Open>> = signal(None);
let folding: Rc<Cell<Option<telar::DismissRegistration>>> = Rc::new(Cell::new(None));
effect(move || match open.get() {
    Some(_) => folding.set(Some(telar::DismissRegistration::new(Rc::new(move || open.set(None))))),
    None => drop(folding.take()),
});
let toggle = move |which: Open| open.update(|shown| *shown = (*shown != Some(which)).then_some(which));
let record = memo(move || {
    ::surfaces::layouts::revision().with(|_| ());
    crate::history::current()
});
let lines = memo(move || crate::history::lines_of(&record.get()));
let walked = memo(move || !record.get().is_empty());
let layer_label = format!("{} ▾", name_of(layer));
let grip_label = telar::t!("editor.strip.grip");
let layers_label = telar::t!("editor.strip.layers");
let add_label = telar::t!("editor.strip.add");
let history_title = history_label();

[view]
col align:center gap:(::ui::scale::space::xs()) pad_x:(::ui::scale::space::lg()) pad_y:(::ui::scale::space::sm()) fill:panel_fill() radius:rad input_opaque label:said
    row align:center gap:(::ui::scale::space::md())
        box pad:(::ui::scale::space::xs()) radius:rad cursor:grab hover_style(fill:$theme.overlay) label:grip_label on_drag:(|_, _| crate::host::strip_dragged()) on_drag_end:(|_, _| crate::host::strip_let_go())
            icon_glyph name:(Reactive::of(|| "grip-vertical".to_string())) tint:(Reactive::of(move || use_theme::<::config::theme::NordTheme>().subtle)) size:16
        icon_glyph name:(Reactive::of(move || icon.to_string())) tint:(Reactive::of(move || use_theme::<::config::theme::NordTheme>().accent)) size:18
        box pad_x:(::ui::scale::space::md()) pad_y:(::ui::scale::space::xs()) radius:rad hover_style(fill:$theme.overlay) label:layers_label on_press:(move || toggle(Open::Layers))
            text "{layer_label.clone()}" color:$theme.text font_size:$theme.font(FontRole::Body)
        text "{output}" color:$theme.subtle font_size:$theme.font(FontRole::Caption)
        if add.is_some()
            box pad_x:(::ui::scale::space::md()) pad_y:(::ui::scale::space::xs()) radius:rad hover_style(fill:$theme.overlay) label:add_label on_press:(move || { open.set(None); if let Some(add) = add { mode::said(add()); } })
                text "+" color:$theme.text font_size:$theme.font(FontRole::Body)
        if $walked
            box pad_x:(::ui::scale::space::md()) pad_y:(::ui::scale::space::xs()) radius:rad hover_style(fill:$theme.overlay) label:history_title on_press:(move || toggle(Open::History))
                text "{history_label()}" color:$theme.text font_size:$theme.font(FontRole::Body)
        for action in actions.clone()
            box pad_x:(::ui::scale::space::md()) pad_y:(::ui::scale::space::xs()) radius:rad hover_style(fill:$theme.overlay) label:((action.0)()) on_press:(move || { open.set(None); (action.1)(); })
                text "{(action.0)()}" color:$theme.text font_size:$theme.font(FontRole::Body)
        box pad_x:(::ui::scale::space::md()) pad_y:(::ui::scale::space::xs()) radius:rad hover_style(fill:$theme.overlay) label:(keys_label()) on_press:(move || help.update(|shown| *shown = !*shown))
            text "{keys_label()}" color:$theme.text font_size:$theme.font(FontRole::Body)
        box pad_x:(::ui::scale::space::md()) pad_y:(::ui::scale::space::xs()) fill:$theme.accent radius:rad hover_style(fill:$theme.accent.darken(0.08)) on_press:(|| { mode::leave(); })
            text "{done_label()}" color:$theme.base font_size:$theme.font(FontRole::Body)
    if $open == Some(Open::Layers)
        col gap:(::ui::scale::space::xs())
            for each in LayerKind::ALL
                box pad_x:(::ui::scale::space::md()) pad_y:(::ui::scale::space::xs()) radius:rad hover_style(fill:$theme.overlay) on_press:(move || { open.set(None); if each != layer { mode::switch(each); } })
                    text "{layer_line(each, layer)}" color:$theme.text font_size:$theme.font(FontRole::Body)
    if $open == Some(Open::History)
        col gap:(::ui::scale::space::xs())
            for line in $lines key line.clone()
                box pad_x:(::ui::scale::space::md()) pad_y:(::ui::scale::space::xs()) radius:rad hover_style(fill:$theme.overlay) on_press:(move || { open.set(None); line.pick(); })
                    row gap:(::ui::scale::space::lg()) justify:between
                        if line.ahead
                            text "{line.marked()}" color:$theme.subtle font_size:$theme.font(FontRole::Body)
                        if !line.ahead
                            text "{line.marked()}" color:$theme.text font_size:$theme.font(FontRole::Body)
                        text "{line.hint.clone()}" color:$theme.subtle font_size:$theme.font(FontRole::Caption)
    switcher current:layer
    if !$variant.is_empty()
        text "{$variant}" color:$theme.accent font_size:$theme.font(FontRole::Caption)
    if !$confirmation.is_empty()
        text "{$confirmation}" color:$theme.text font_size:$theme.font(FontRole::Caption)
    if !$refusal.is_empty()
        text "{$refusal}" color:$theme.warning font_size:$theme.font(FontRole::Caption)
    if $refused.is_some()
        text "{$refused.unwrap_or_default()}" color:$theme.warning font_size:$theme.font(FontRole::Caption)
    if $help
        col gap:(::ui::scale::space::xs())
            for line in crate::keys::help_rows(layer)
                row gap:(::ui::scale::space::md())
                    box width:CHORDS
                        text "{&line.keys}" color:$theme.subtle font_size:$theme.font(FontRole::Caption)
                    text "{&line.what}" color:$theme.text font_size:$theme.font(FontRole::Caption)
