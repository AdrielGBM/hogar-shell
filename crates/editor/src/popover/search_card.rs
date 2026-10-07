//! The card both palettes are: a title, a search field and the rows the query finds, capped to the screen and scrolling past it, the row the arrows point at kept in view.

use std::hash::Hash;

use telar::{
    Container, Key, LayoutItem, LayoutStyle, NamedKey, NodeId, ReactiveList, RectStyle, RwSignal,
    SizeDimension, StyledContainer, Text, box_item, effect, use_theme,
};

use config::theme::{FontRole, NordTheme};
use ui::descriptor::Built;

use crate::host::{passthrough, usable, whole};
use crate::popover::rows::{self, label};

#[derive(Clone, Copy)]
pub(crate) enum Inset {
    Left,
    Centre,
}

pub(crate) struct Search<'a> {
    pub output: &'a str,
    pub width: f32,
    pub inset: Inset,
    pub title: Box<dyn Fn() -> String>,
    pub query: RwSignal<String>,
}

/// The card on `search.output`: `source` says when the rows are built again, and `rows` builds them from what it yields, told the signal to give the node of the row the arrows point at.
pub(crate) fn search_card<K: Hash + Clone + 'static>(
    search: Search<'_>,
    source: impl Fn() -> Vec<K> + 'static,
    rows: impl Fn(K, RwSignal<Option<NodeId>>) -> Built + 'static,
    on_key: impl Fn(&Key) -> bool + 'static,
) -> Built {
    let theme = use_theme::<NordTheme>();
    let pad = ui::scale::space::lg();
    let gap = ui::scale::space::md();
    let Search {
        output,
        width,
        inset,
        title,
        query,
    } = search;
    let pointed_row: RwSignal<Option<NodeId>> = telar::signal(None);
    let list = ReactiveList::with_style(
        LayoutStyle::new()
            .flex_column()
            .gap(ui::scale::space::xs())
            .width(SizeDimension::Percent(1.0)),
        source,
        |held: &K| held.clone(),
        move |held: K| rows(held, pointed_row),
    )?;
    let title = Text::declaring(title, LayoutStyle::new(), move |inherited| {
        theme
            .text_over(inherited, FontRole::Body, theme.text)
            .with_font_weight(700)
    })?;
    let field = rows::text(label!("editor.palette.search"), None, query)?;
    let title_height = super::tracked_height(&title, "title")?;
    let field_height = super::tracked_height(&*field, "search field")?;
    let (lines, viewport) = super::capped_card_rows(
        box_item(list),
        width - 2.0 * pad,
        Some(output.to_string()),
        move |usable| (usable.height - 8.0 * pad).max(120.0),
        move || title_height.get().height + field_height.get().height + 2.0 * gap + 2.0 * pad,
    )?;
    effect(move || {
        if let Some(row) = pointed_row.get() {
            viewport.reveal(row, gap);
        }
    });
    let placed = output.to_string();
    let style = move || {
        let usable = usable(Some(&placed));
        let start = match inset {
            Inset::Left => pad,
            Inset::Centre => ((usable.width - width) / 2.0).max(0.0),
        };
        LayoutStyle::new()
            .absolute()
            .inset_start(usable.x + start)
            .inset_top(usable.y + 4.0 * pad)
            .width(width)
            .flex_column()
            .gap(gap)
            .padding_all(pad)
    };
    let card = StyledContainer::new(
        style(),
        move |_| RectStyle::filled(theme.surface, ui::scale::corner::xl()),
        vec![box_item(title), field, lines],
    )?
    .styled_by(style)
    .input_opaque()
    .on_key(move |key: &Key| on_key(key));
    Ok(Box::new(passthrough(whole(), vec![Box::new(card)])?))
}

/// What a key does on an open search card: `own` answers first, then ↑/↓ move the row pointed at among `count`, Backspace and typing edit the query.
pub(crate) fn answer(
    key: &Key,
    (query, pointed): (RwSignal<String>, RwSignal<usize>),
    count: &dyn Fn() -> usize,
    own: &dyn Fn(&Key) -> bool,
) -> bool {
    let modifiers = telar::modifiers();
    if modifiers.is_ctrl || modifiers.is_alt || modifiers.is_meta {
        return false;
    }
    if own(key) {
        return true;
    }
    match key {
        Key::Named(NamedKey::ArrowDown) => {
            let count = count();
            pointed.update(|at| *at = (*at + 1).min(count.saturating_sub(1)));
        }
        Key::Named(NamedKey::ArrowUp) => pointed.update(|at| *at = at.saturating_sub(1)),
        Key::Named(NamedKey::Backspace) => {
            query.update(|text| {
                text.pop();
            });
            pointed.set(0);
        }
        Key::Char(ch) if !ch.is_control() => {
            query.update(|text| text.push(*ch));
            pointed.set(0);
        }
        _ => return false,
    }
    true
}

/// `items` as a column, or a note that there are none; `pointed` is the node of the one the arrows point at, which is told to `pointed_row`.
pub(crate) fn listing(
    items: Vec<Box<dyn LayoutItem>>,
    pointed: Option<NodeId>,
    pointed_row: RwSignal<Option<NodeId>>,
) -> Built {
    let mut items = items;
    if items.is_empty() {
        items.push(rows::note(|| telar::t!("editor.palette.nothing"))?);
    }
    if pointed_row.peek() != pointed {
        pointed_row.set(pointed);
    }
    Ok(box_item(Container::new(
        LayoutStyle::new()
            .flex_column()
            .gap(ui::scale::space::xs())
            .width(SizeDimension::Percent(1.0)),
        items,
    )?))
}
