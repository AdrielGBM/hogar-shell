[logic]
use crate::mode::{self, icon_of, name_of};
use crate::pie::{petals, PETAL, PIE};
use ::layout::LayerKind;
use ::ui::chrome::panel_fill;
use ::ui::icon_glyph::{icon_glyph, IconGlyphProps};
use std::cell::Cell;
use std::rc::Rc;

/// Every other edit mode a press away, and the same modes again as a pie: a quicker reach, never the only one (F-7 — a pie holds at most eight and is never the only path). While the pie is open it is on the dismiss stack, so Esc folds it before it reaches the mode; the switcher key opens it as well, and an arrow then picks the mode that lies that way ([`crate::keys`]).
pub struct Props {
    pub current: LayerKind = LayerKind::Desktop,
}

let current = props.current;
let petals = petals(current);
let pie = crate::pie::shown();
let folding: Rc<Cell<Option<telar::DismissRegistration>>> = Rc::new(Cell::new(None));
effect(move || match pie.get() {
    true => folding.set(Some(telar::DismissRegistration::new(Rc::new(move || pie.set(false))))),
    false => drop(folding.take()),
});
let rad = ::ui::scale::corner::md();
let round = PETAL / 2.0;
let modes_label = telar::t!("editor.modes");

[view]
col align:center gap:(::ui::scale::space::xs())
    row align:center gap:(::ui::scale::space::xs())
        for petal in petals.clone()
            box pad:(::ui::scale::space::xs()) radius:rad hover_style(fill:$theme.overlay) label:(telar::t!("editor.edit", layer = name_of(petal.layer))) on_press:(move || mode::switch(petal.layer))
                icon_glyph name:(Reactive::of(move || icon_of(petal.layer).to_string())) tint:(Reactive::of(move || use_theme::<::config::theme::NordTheme>().text)) size:16
        box pad:(::ui::scale::space::xs()) radius:rad hover_style(fill:$theme.overlay) label:modes_label on_press:(move || pie.update(|open| *open = !*open))
            icon_glyph name:(Reactive::of(|| "circle-dot".to_string())) tint:(Reactive::of(move || use_theme::<::config::theme::NordTheme>().subtle)) size:16
    if $pie
        box width:PIE height:PIE
            for petal in petals.clone()
                box absolute inset_start:petal.x inset_top:petal.y width:PETAL height:PETAL align:center justify:center fill:panel_fill() radius:round hover_style(fill:$theme.overlay) label:(telar::t!("editor.edit", layer = name_of(petal.layer))) on_press:(move || { pie.set(false); mode::switch(petal.layer); })
                    icon_glyph name:(Reactive::of(move || icon_of(petal.layer).to_string())) tint:(Reactive::of(move || use_theme::<::config::theme::NordTheme>().text)) size:16
