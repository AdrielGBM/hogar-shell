use std::rc::Rc;

use telar::{Children, Reactive, RwSignal, effect, signal};

use crate::descriptor::Built;
use crate::form::labelled::labelled;
use crate::form::recorder::{option_index, pick_option};

/// A labelled row of every option at once, the one `value` holds lit: the picker for a handful of options, where a dropdown would hide choices there is room to show.
#[derive(telar::Props)]
pub struct SegmentedRowProps {
    #[props(into, default)]
    pub label: Reactive<String>,
    #[props(default = signal(String::new()))]
    pub value: RwSignal<String>,
    #[props(into, default = Rc::from(Vec::new()))]
    pub options: Rc<[&'static str]>,
}

pub fn segmented_row(props: SegmentedRowProps, _children: Children) -> Built {
    let SegmentedRowProps {
        label,
        value,
        options,
    } = props;
    let picked = option_index(value, Rc::clone(&options));
    let picking = Rc::clone(&options);
    let seeded = std::cell::Cell::new(false);
    effect(move || {
        let at = picked.get();
        if seeded.replace(true) {
            pick_option(&value, &picking, at);
        }
    });
    let theme = telar::use_theme::<config::theme::NordTheme>();
    let tabs = telar::tabs(
        telar::TabsProps::props()
            .items(options.to_vec())
            .selected(picked)
            .color(Reactive::of(move || theme.accent))
            .build(),
        Children::default(),
    )?;
    labelled(label, tabs)
}
