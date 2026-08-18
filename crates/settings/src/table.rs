//! The widgets a form of repeating rows is built from — a table with an add button, a delete per row,
//! and the pills a membership list is edited through.

use ui::scale::{corner, paint, space};
use std::rc::Rc;

use telar::{
    AlignItems, Color, Container, JustifyContent, LayoutError, LayoutItem, LayoutStyle,
    ReactiveList, RectStyle, RwSignal, SizeDimension, StyledContainer, signal,
};

use crate::form::*;
use config::theme::NordTheme;
use ui::icon::icon_view;

/// K13, second half: a `[[list]]` of config tables, edited as rows with an Add button and a remove control on
/// each — `[[battery.warn_levels]]` and `[[idle.stages]]`.
///
/// Two pieces of state, deliberately. The *order* is a signal, so adding or removing a row redraws the list.
/// The *values* are a plain map behind an `Rc`, because a row's fields change on every keystroke and a signal
/// there would rebuild the row being typed into — the trap every keyed list in this shell documents.
///
/// Rows are keyed on a synthetic id rather than on their index. An index-keyed list reuses row 1's widgets for
/// what used to be row 2 when row 1 is deleted, because the key it reconciles on did not change: the user
/// deletes one warning and the form quietly shows them another one's values under the first one's heading.
pub(crate) struct TableList<T> {
    order: RwSignal<Vec<u64>>,
    values: Rc<std::cell::RefCell<std::collections::HashMap<u64, T>>>,
    next: Rc<std::cell::Cell<u64>>,
}

impl<T: Clone + 'static> TableList<T> {
    pub(crate) fn new(entries: Vec<T>) -> Self {
        let list = Self {
            order: signal(Vec::new()),
            values: Rc::new(std::cell::RefCell::new(std::collections::HashMap::new())),
            next: Rc::new(std::cell::Cell::new(0)),
        };
        for entry in entries {
            list.add(entry);
        }
        list
    }

    pub(crate) fn clone_handle(&self) -> Self {
        Self {
            order: self.order.clone(),
            values: Rc::clone(&self.values),
            next: Rc::clone(&self.next),
        }
    }

    pub(crate) fn add(&self, entry: T) {
        let id = self.next.get();
        self.next.set(id + 1);
        self.values.borrow_mut().insert(id, entry);
        let mut order = self.order.peek();
        order.push(id);
        self.order.set(order);
    }

    pub(crate) fn remove(&self, id: u64) {
        self.values.borrow_mut().remove(&id);
        let order: Vec<u64> = self
            .order
            .peek()
            .into_iter()
            .filter(|existing| *existing != id)
            .collect();
        self.order.set(order);
    }

    pub(crate) fn edit(&self, id: u64, apply: impl FnOnce(&mut T)) {
        if let Some(entry) = self.values.borrow_mut().get_mut(&id) {
            apply(entry);
        }
    }

    pub(crate) fn get(&self, id: u64) -> Option<T> {
        self.values.borrow().get(&id).cloned()
    }

    /// The list as the config carries it, in the order the rows are drawn in.
    pub(crate) fn collect(&self) -> Vec<T> {
        let values = self.values.borrow();
        self.order
            .peek()
            .into_iter()
            .filter_map(|id| values.get(&id).cloned())
            .collect()
    }

    pub(crate) fn view(
        &self,
        row: impl Fn(u64) -> Result<Box<dyn LayoutItem>, LayoutError> + 'static,
    ) -> Result<Box<dyn LayoutItem>, LayoutError> {
        let order = self.order.read_only();
        Ok(Box::new(ReactiveList::new(
            move || order.get(),
            |id: &u64| id.to_string(),
            move |id: u64| row(id),
            space::lg(),
        )?))
    }
}

/// A text field bound to one field of a [`TableList`] entry, writing back on every keystroke.
///
/// Returns the effect for the row to hold: a bare `effect(…)` statement runs once and stops, which looks like
/// a field that accepts the first character and then ignores the rest of the word.
pub(crate) fn bound_field<T: Clone + 'static>(
    label: impl Fn() -> String + 'static,
    list: &TableList<T>,
    id: u64,
    initial: String,
    placeholder: &str,
    theme: NordTheme,
    apply: impl Fn(&mut T, &str) + 'static,
) -> Result<(Box<dyn LayoutItem>, telar::Effect), LayoutError> {
    let value = signal(initial);
    let watched = value.read_only();
    let list = list.clone_handle();
    let sync = telar::effect(move || {
        let text = watched.get();
        list.edit(id, |entry| apply(entry, &text));
    });
    Ok((text_field(label, value, placeholder, theme)?, sync))
}

/// [`bound_field`] for a switch.
pub(crate) fn bound_toggle<T: Clone + 'static>(
    label: impl Fn() -> String + 'static,
    list: &TableList<T>,
    id: u64,
    initial: bool,
    theme: NordTheme,
    apply: impl Fn(&mut T, bool) + 'static,
) -> Result<(Box<dyn LayoutItem>, telar::Effect), LayoutError> {
    let value = signal(initial);
    let watched = value.read_only();
    let list = list.clone_handle();
    let sync = telar::effect(move || {
        let on = watched.get();
        list.edit(id, |entry| apply(entry, on));
    });
    Ok((toggle_field(label, value, theme)?, sync))
}

/// One entry of a [`TableList`]: its fields in a filled card, with the control that deletes it.
pub(crate) fn entry_card<T: Clone + 'static>(
    mut fields: Vec<Box<dyn LayoutItem>>,
    list: &TableList<T>,
    id: u64,
    theme: NordTheme,
    subscriptions: Vec<telar::Effect>,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let remove = {
        let list = list.clone_handle();
        toggle_pill("trash-2", false, theme.red, theme, move || list.remove(id))?
    };
    fields.push(Box::new(Container::new(
        LayoutStyle::new()
            .flex_row()
            .justify_content(JustifyContent::END)
            .width(SizeDimension::Percent(1.0)),
        vec![remove],
    )?));
    let card = StyledContainer::new(
        LayoutStyle::new()
            .flex_column()
            .gap(space::md())
            .padding_all(space::lg())
            .width(SizeDimension::Percent(1.0)),
        paint::md(theme.surface),
        fields,
    )?;
    Ok(Box::new(telar::Holding::new(Box::new(card), subscriptions)))
}

/// Adds `id` to a list or takes it out again — what both switches on an application row do.
pub(crate) fn toggle_membership(list: RwSignal<Vec<String>>, id: String) -> impl Fn() + 'static {
    move || {
        let mut ids = list.peek();
        match ids.iter().position(|existing| *existing == id) {
            Some(index) => {
                ids.remove(index);
            }
            None => ids.push(id.clone()),
        }
        list.set(ids);
    }
}

/// A square icon button that reads as on or off — the row-sized form of [`toggle_field`], which is a labelled
/// row and far too wide to put two of on every application.
pub(crate) fn toggle_pill(
    glyph: &'static str,
    on: bool,
    tint: Color,
    theme: NordTheme,
    on_press: impl Fn() + 'static,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let ink = if on { theme.base } else { theme.muted };
    let icon = icon_view(move || glyph.to_string(), move || ink, 16.0)?;
    let rounded = corner::md();
    Ok(Box::new(
        StyledContainer::new(
            LayoutStyle::new()
                .flex_shrink(0.0)
                .padding_all(space::md())
                .align_items(AlignItems::CENTER)
                .justify_content(JustifyContent::CENTER),
            move |_r| {
                let fill = if on { tint } else { theme.overlay };
                RectStyle::filled(fill, rounded)
            },
            vec![icon],
        )?
        .on_press(on_press),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use config::IdleStage;
    /// The bug an index-keyed list would ship: deleting one entry has to take *that* entry's values with it,
    /// and leave every other row still holding its own. Nothing about the rendered form says which is which,
    /// so it is only visible as a user finding someone else's numbers in the box they were editing.
    #[test]
    fn removing_one_entry_leaves_the_others_holding_their_own_values() {
        telar::reset_runtime();
        let list = TableList::new(vec![
            IdleStage {
                timeout: 300,
                action: "lock on".into(),
                return_action: String::new(),
            },
            IdleStage {
                timeout: 600,
                action: "shell dpms off".into(),
                return_action: "shell dpms on".into(),
            },
            IdleStage {
                timeout: 900,
                action: "session do suspend".into(),
                return_action: String::new(),
            },
        ]);
        let ids = list.order.peek();
        assert_eq!(ids.len(), 3);

        list.edit(ids[2], |stage| stage.timeout = 1200);
        list.remove(ids[0]);

        let left = list.collect();
        assert_eq!(
            left.iter().map(|s| s.timeout).collect::<Vec<_>>(),
            vec![600, 1200],
            "the survivors keep their own values, including one edited before the removal"
        );
        assert_eq!(left[0].return_action, "shell dpms on");

        // And an added row is a new entry, never a reused slot: an id is spent even when one has been freed.
        list.add(IdleStage::default());
        let after = list.order.peek();
        assert_eq!(after.len(), 3);
        assert!(
            !after.contains(&ids[0]),
            "the removed id is not handed out again"
        );
        assert_eq!(list.collect().len(), 3);
    }
}
