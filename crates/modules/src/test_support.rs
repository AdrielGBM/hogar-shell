//! What the modules' tests share to see what a surface draws.

#![cfg(test)]

use telar::{
    AvailableSpace, ComponentList, Container, DrawCommand, LayoutItem, LayoutStyle, set_theme,
};

use config::theme::NordTheme;

pub fn fresh() {
    telar::reset_layout_runtime();
    set_theme(NordTheme::new());
}

/// Every command `item` draws laid out in a box `width` by `height`, one debug line each, without the element pushes and pops that carry no paint.
pub fn drawn(item: Box<dyn LayoutItem>, width: f32, height: f32) -> Vec<String> {
    let page =
        Container::new(LayoutStyle::new().width(width).height(height), vec![item]).expect("a page");
    let root = page.layout_node();
    let tree = ComponentList::new(page);
    telar::compute_layout(
        root,
        AvailableSpace::Definite(width),
        AvailableSpace::Definite(height),
    )
    .expect("the item lays out");
    tree.commands()
        .iter()
        .filter(|command| {
            !matches!(
                command,
                DrawCommand::PushElement { .. } | DrawCommand::PopElement
            )
        })
        .map(|command| format!("{command:?}"))
        .collect()
}
