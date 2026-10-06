//! What the popovers that edit `config.toml` rather than the layout share: one transaction over what they choose, kept when they close and put back on Esc, outside the layout's undo, and written through the same format-preserving save the settings window uses.

use std::cell::Cell;
use std::path::Path;

use serde::Serialize;
use telar::{
    Container, LayoutError, LayoutItem, LayoutStyle, RectStyle, RwSignal, StyledContainer,
    Transaction, box_item, effect, register_transaction, signal, use_theme,
};

use config::Config;
use config::theme::NordTheme;
use surfaces::transient;
use ui::descriptor::Built;

use crate::host::{self, passthrough, whole};

/// Opens a transaction over `chosen` for the popover `id` as its card builds, calling `kept` with what it holds when the popover closes any way but Esc, and closing the popover once it is let go. What it answers is the popover's open state, which closing it keeping its choice sets false.
pub(crate) fn hold<T: Clone + 'static>(
    id: &'static str,
    chosen: RwSignal<T>,
    kept: impl Fn(&T) + 'static,
) -> (RwSignal<bool>, Transaction<T>) {
    let open = signal(false);
    let transaction = Transaction::new(chosen).on_commit(move |_, after| kept(after));
    register_transaction(open, transaction);
    // Opened here, as the card builds: the transient's own entry is on the dismiss stack by now, so the transaction's goes above it and Esc reaches it first.
    open.set(true);
    let was_open = Cell::new(false);
    effect(move || {
        let now = open.get();
        if was_open.replace(now) && !now {
            transient::close(id);
        }
    });
    (open, transaction)
}

/// Closes the popover `id` keeping its choice: what closing it any way but Esc does.
pub(crate) fn close(id: &str, open: Option<RwSignal<bool>>) {
    if let Some(open) = open
        && open.is_alive()
        && open.peek()
    {
        open.set(false);
    }
    transient::close(id);
}

/// The card such a popover is, `width` across, at the top right of what the bars of `output` leave.
pub(crate) fn card(output: &str, width: f32, rows: Vec<Box<dyn LayoutItem>>) -> Built {
    framed(output, width, rows, Vec::new())
}

/// [`card`] with `footer` under its rows and always in view: the rows scroll once they are taller than the screen leaves them.
pub(crate) fn framed(
    output: &str,
    width: f32,
    rows: Vec<Box<dyn LayoutItem>>,
    footer: Vec<Box<dyn LayoutItem>>,
) -> Built {
    let theme = use_theme::<NordTheme>();
    let pad = ui::scale::space::lg();
    let gap = ui::scale::space::md();
    let inner = width - 2.0 * pad;
    let column = |items| {
        Container::new(
            LayoutStyle::new().flex_column().gap(gap).width(inner),
            items,
        )
    };
    let footer = match footer.is_empty() {
        true => None,
        false => Some(column(footer)?),
    };
    let footer_tall = match &footer {
        Some(footer) => Some(telar::track_layout(footer.layout_node()).ok_or_else(|| {
            LayoutError::Engine("a popover's footer has no layout node".to_string())
        })?),
        None => None,
    };
    let room = {
        let output = output.to_string();
        move || {
            let usable = host::usable(Some(&output));
            let below = footer_tall.map_or(0.0, |tall| tall.get().height + gap);
            (usable.height * crate::popover::TALLEST - below - 2.0 * pad).max(0.0)
        }
    };
    let (rows, _) = crate::popover::capped_rows(box_item(column(rows)?), inner, room)?;
    let mut children = vec![rows];
    children.extend(footer.map(box_item));
    let placed = output.to_string();
    let card = StyledContainer::new(
        LayoutStyle::new(),
        move |_| RectStyle::filled(theme.surface, ui::scale::corner::xl()),
        children,
    )?
    .styled_by(move || {
        let usable = host::usable(Some(&placed));
        LayoutStyle::new()
            .absolute()
            .inset_start(usable.x + usable.width - width - pad)
            .inset_top(usable.y + 4.0 * pad)
            .width(width)
            .flex_column()
            .gap(gap)
            .padding_all(pad)
    })
    .input_opaque();
    Ok(Box::new(passthrough(whole(), vec![Box::new(card)])?))
}

/// Writes the `[section]` `change` makes of the config at `path` into it, around whatever else the file says.
pub(crate) fn save<T: Serialize>(
    path: &Path,
    section: &str,
    change: impl FnOnce(Config) -> T,
) -> Result<(), String> {
    Config::save_section(path, section, &change(Config::load_or_default(path)))
        .map(|_| ())
        .map_err(|why| why.to_string())
}

/// The documentation of `[section] key`, which explains its row.
pub(crate) fn documented(section: &str, key: &str) -> Option<String> {
    config::fields::section(section)?
        .into_iter()
        .find(|field| field.key == key)?
        .doc
        .map(str::to_string)
}
