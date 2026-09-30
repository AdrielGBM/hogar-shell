use std::rc::Rc;

use telar::{Children, Reactive, RwSignal, effect, signal};

use config::theme::NordTheme;

use crate::descriptor::Built;
use crate::form::labelled::labelled;
use crate::form::recorder::record_field;

/// A labelled row of the theme's colours by name, the one `value` names lit and none while it names none: the picker for a colour the file spells as a token, such as an accent.
#[derive(telar::Props)]
pub struct SwatchRowProps {
    #[props(into, default)]
    pub label: Reactive<String>,
    #[props(default = signal(String::new()))]
    pub value: RwSignal<String>,
    /// The token names offered, each drawn in the colour the theme gives it.
    #[props(into, default = Rc::from(config::theme::ACCENTS))]
    pub tokens: Rc<[&'static str]>,
}

pub fn swatch_row(props: SwatchRowProps, _children: Children) -> Built {
    let SwatchRowProps {
        label,
        value,
        tokens,
    } = props;
    record_field(&value);
    let theme = telar::use_theme::<NordTheme>();
    let index_of = {
        let tokens = Rc::clone(&tokens);
        move |name: &str| {
            tokens
                .iter()
                .position(|token| *token == name)
                .map(|at| at as u32)
        }
    };
    let selected = signal(index_of(&value.peek()));
    let followed = value.read_only();
    effect(move || {
        let at = index_of(&followed.get());
        if selected.peek() != at {
            selected.set(at);
        }
    });
    let picking = Rc::clone(&tokens);
    let swatches = telar::swatches(
        telar::SwatchesProps::props()
            .colors(tokens.iter().map(|token| theme.token(token)).collect())
            .names(tokens.iter().map(|token| token.to_string()).collect())
            .selected(selected)
            .on_select(Rc::new(move |at: u32| {
                if let Some(name) = picking.get(at as usize)
                    && value.peek() != *name
                {
                    value.set(name.to_string());
                }
            }))
            .build(),
        Children::default(),
    )?;
    labelled(label, swatches)
}
