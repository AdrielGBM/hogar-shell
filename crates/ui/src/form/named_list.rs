use std::rc::Rc;

use telar::{
    AlignItems, Children, LayoutItem, LayoutStyle, ReactiveList, ReadSignal, SizeDimension, Text,
    box_item, use_theme,
};

use config::theme::{FontRole, NordTheme};

use crate::descriptor::Built;
use crate::scale::space;

/// What each name in a [`named_list`] offers to do with it.
pub struct NamedActions {
    pub pick: Rc<dyn Fn() -> String>,
    pub on_pick: Rc<dyn Fn(&str)>,
    pub remove: Rc<dyn Fn() -> String>,
    pub on_remove: Rc<dyn Fn(&str)>,
}

/// One row per name `names` holds, as it changes — the name, a button that picks it and one that removes it — or `empty` while it holds none.
pub fn named_list(
    names: ReadSignal<Vec<String>>,
    empty: impl Fn() -> String + 'static,
    actions: NamedActions,
) -> Built {
    let theme = use_theme::<NordTheme>();
    let actions = Rc::new(actions);
    let empty = Rc::new(empty);
    let source = move || match names.with(Vec::is_empty) {
        true => vec![None],
        false => names.get().into_iter().map(Some).collect(),
    };
    let build = move |name: Option<String>| -> Built {
        let Some(name) = name else {
            let empty = Rc::clone(&empty);
            return Ok(box_item(Text::declaring(
                move || empty(),
                LayoutStyle::new(),
                move |inherited| theme.text_over(inherited, FontRole::Caption, theme.subtle),
            )?));
        };
        let shown = name.clone();
        let label = Text::declaring(
            move || shown.clone(),
            LayoutStyle::new().flex_grow(1.0).min_width(0.0),
            move |inherited| theme.text_over(inherited, FontRole::Body, theme.text),
        )?;
        let button = |said: &Rc<dyn Fn() -> String>, act: &Rc<dyn Fn(&str)>| {
            let (said, act, name) = (Rc::clone(said), Rc::clone(act), name.clone());
            telar::button(
                telar::ButtonProps::props()
                    .label(telar::Reactive::of(move || said()))
                    .on_press(Rc::new(move || act(&name)))
                    .build(),
                Children::default(),
            )
        };
        Ok(box_item(telar::Container::new(
            LayoutStyle::new()
                .flex_row()
                .align_items(AlignItems::CENTER)
                .gap(space::sm())
                .width(SizeDimension::Percent(1.0)),
            vec![
                box_item(label),
                button(&actions.pick, &actions.on_pick)?,
                button(&actions.remove, &actions.on_remove)?,
            ],
        )?))
    };
    Ok(Box::new(ReactiveList::with_style(
        LayoutStyle::new()
            .flex_column()
            .gap(space::sm())
            .width(SizeDimension::Percent(1.0)),
        source,
        |name: &Option<String>| name.clone(),
        build,
    )?) as Box<dyn LayoutItem>)
}
