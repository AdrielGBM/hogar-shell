use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use platform_wayland::timeout;
use telar::{
    AlignItems, Container, Effect, Input, JustifyContent, LayoutError, LayoutItem, LayoutStyle,
    NodeId, ReactiveList, ReadSignal, Rect, RectStyle, RwSignal, SizeDimension, StyledContainer,
    Text, TextArea, box_item, effect, signal, track_layout, use_theme,
};

use config::theme::{FontRole, NordTheme};
use services::notes::{self, Note};
use surfaces::drawer::content_radius;
use ui::scale::{corner, space};
use ui::icon::{icon_picker_overlay, icon_view};
use ui::module::{icon_px, module_fg, surface_env};

/// How long after the last edit to a note before it is written to disk (trailing-edge debounce).
const SAVE_DEBOUNCE: Duration = Duration::from_millis(400);

/// Panel-wide state. `notes` holds only plain data — never nested signals, since a signal that stored other
/// signals in its value would re-enter the reactive runtime's borrow when the surface tears down (dropping the
/// value while `drop_signal` holds the borrow). Each note's editable fields are per-card signals owned by the
/// card's widgets; a card's sync effect (parked in `effects` for the panel's lifetime) mirrors those edits
/// back into `notes` and schedules a save.
#[derive(Clone)]
struct PanelState {
    notes: RwSignal<Vec<Note>>,
    save_generation: Rc<Cell<u64>>,
    effects: Rc<RefCell<Vec<Effect>>>,
}

/// The bar chip: a sticky-note glyph that opens the notes panel.
pub fn notes_chip() -> Result<Box<dyn LayoutItem>, LayoutError> {
    let fg = module_fg();
    icon_view(|| "sticky-note".to_string(), move || fg.get(), icon_px())
}

/// The notes panel: a header (title + add) over the editable note list, each note an icon, title, body, and
/// delete, with an inline icon picker per note. Loads from disk on open; every edit persists (debounced).
pub fn notes_panel() -> Result<Box<dyn LayoutItem>, LayoutError> {
    if let Some(env) = surface_env() {
        services::locale::attach(env.config.language());
    }
    let theme = use_theme::<NordTheme>();
    let radius = content_radius();
    let state = PanelState {
        notes: signal(notes::load()),
        save_generation: Rc::new(Cell::new(0)),
        effects: Rc::new(RefCell::new(Vec::new())),
    };

    let header = header(&state, theme)?;
    let list = note_list(&state, theme, radius)?;
    let panel = Container::new(
        LayoutStyle::new()
            .flex_column()
            .gap(space::LG)
            .width(SizeDimension::Percent(1.0)),
        vec![header, list],
    )?;
    Ok(Box::new(panel))
}

fn header(state: &PanelState, theme: NordTheme) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let title = Text::auto(
        || telar::t!("notes.title"),
        LayoutStyle::new(),
        move || {
            theme
                .text_style(FontRole::Title, theme.text)
                .with_weight(700)
        },
    )?;
    let add_state = state.clone();
    let add = pill_button(
        || telar::t!("notes.new"),
        move || add_note(&add_state),
        theme,
    )?;
    let header = Container::new(
        LayoutStyle::new()
            .flex_row()
            .align_items(AlignItems::CENTER)
            .justify_content(JustifyContent::SPACE_BETWEEN)
            .gap(space::MD)
            .width(SizeDimension::Percent(1.0)),
        vec![Box::new(title), add],
    )?;
    Ok(Box::new(header))
}

fn note_list(
    state: &PanelState,
    theme: NordTheme,
    radius: f32,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let notes = state.notes.read_only();
    let build_state = state.clone();
    let list = ReactiveList::new(
        move || notes.get(),
        |n: &Note| n.id,
        move |note: Note| note_card(&build_state, note, theme, radius),
        space::MD,
    )?;
    let column = Container::new(
        LayoutStyle::new()
            .flex_column()
            .gap(space::MD)
            .width(SizeDimension::Percent(1.0)),
        vec![Box::new(list) as Box<dyn LayoutItem>],
    )?;
    Ok(Box::new(column))
}

fn note_card(
    state: &PanelState,
    note: Note,
    theme: NordTheme,
    radius: f32,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let id = note.id;
    // Per-card editable state (owned by this card's widgets, not stored inside the `notes` signal).
    let icon = signal(note.icon);
    let title = signal(note.title);
    let body = signal(note.body);
    let picking = signal(false);
    wire_persist(state, id, icon.clone(), title.clone(), body.clone());

    let glyph = {
        let icon = icon.clone();
        move || icon.get().unwrap_or_else(|| "plus".to_string())
    };
    let toggle_picking = {
        let picking = picking.clone();
        move || picking.update(|p| *p = !*p)
    };
    // Resolved once and captured: a style closure runs on every paint, and the lookup behind it is not free.
    let rounded = corner::md();
    let icon_button = StyledContainer::new(
        square_style(),
        move |_| RectStyle::filled(theme.base, rounded),
        vec![icon_view(glyph, move || theme.text, 20.0)?],
    )?
    .hover_style(move |_| RectStyle::filled(theme.overlay, rounded))
    .on_press(toggle_picking);
    // The icon button anchors the picker overlay; its rect (filled in by layout) positions the popover.
    let trigger_node = icon_button.layout_node();
    let trigger_rect = track_layout(trigger_node).expect("icon button node is registered");

    let title_input = Input::new(
        title,
        LayoutStyle::new()
            .flex_grow(1.0)
            .height(theme.font(FontRole::Body) * 1.4),
        move || {
            theme
                .text_style(FontRole::Body, theme.text)
                .with_weight(700)
        },
    )?
    .placeholder(telar::t!("notes.title_placeholder"));

    let delete_state = state.clone();
    let rounded = corner::md();
    let delete = StyledContainer::new(
        square_style(),
        move |_| RectStyle::filled(theme.base, rounded),
        vec![icon_view(|| "x".to_string(), move || theme.muted, 16.0)?],
    )?
    .hover_style(move |_| RectStyle::filled(theme.overlay, rounded))
    .on_press(move || delete_note(&delete_state, id));

    let row = Container::new(
        LayoutStyle::new()
            .flex_row()
            .align_items(AlignItems::CENTER)
            .gap(space::MD)
            .width(SizeDimension::Percent(1.0)),
        vec![
            Box::new(icon_button),
            box_item(title_input),
            Box::new(delete),
        ],
    )?;

    let body_area = TextArea::new(
        body,
        LayoutStyle::new().width(SizeDimension::Percent(1.0)),
        move || theme.text_style(FontRole::Body, theme.subtle),
    )?
    .placeholder(telar::t!("notes.body_placeholder"));

    let card = StyledContainer::new(
        LayoutStyle::new()
            .flex_column()
            .gap(space::MD)
            .padding_all(space::LG)
            .width(SizeDimension::Percent(1.0)),
        move |_| RectStyle::filled(theme.surface, radius),
        vec![
            Box::new(row),
            box_item(body_area),
            picker_overlay(icon, picking, trigger_node, trigger_rect)?,
        ],
    )?;
    Ok(Box::new(card))
}

/// Mirrors a card's edits back into the plain `notes` list and schedules a debounced save. The effect only
/// subscribes to this card's fields, so it never re-fires from the list write it performs.
fn wire_persist(
    state: &PanelState,
    id: u64,
    icon: RwSignal<Option<String>>,
    title: RwSignal<String>,
    body: RwSignal<String>,
) {
    let notes = state.notes.clone();
    let generation = Rc::clone(&state.save_generation);
    let sync = effect(move || {
        let icon = icon.get();
        let title = title.get();
        let body = body.get();
        notes.update(|list| {
            if let Some(note) = list.iter_mut().find(|n| n.id == id) {
                note.icon = icon;
                note.title = title;
                note.body = body;
            }
        });
        schedule_save(notes.read_only(), Rc::clone(&generation));
    });
    state.effects.borrow_mut().push(sync);
}

/// The per-note icon picker, opened as a floating popover anchored to the note's icon button while that
/// note's picker is open. Choosing an icon sets the note's icon (which persists via its sync effect); the
/// popover closes on pick or on a click outside.
fn picker_overlay(
    icon: RwSignal<Option<String>>,
    picking: RwSignal<bool>,
    trigger_node: NodeId,
    trigger_rect: RwSignal<Rect>,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let source_picking = picking.clone();
    let list = ReactiveList::new(
        move || vec![source_picking.get()],
        |open: &bool| *open,
        move |open: bool| -> Result<Box<dyn LayoutItem>, LayoutError> {
            if !open {
                return Ok(Box::new(Container::new(LayoutStyle::new(), vec![])?));
            }
            let icon = icon.clone();
            let closing = picking.clone();
            icon_picker_overlay(
                trigger_node,
                trigger_rect.clone(),
                move |id: String| icon.set(Some(id)),
                move || closing.set(false),
            )
        },
        0.0,
    )?;
    Ok(Box::new(list))
}

fn add_note(state: &PanelState) {
    state.notes.update(|list| {
        list.push(Note {
            id: notes::next_id(list),
            icon: None,
            title: String::new(),
            body: String::new(),
        });
    });
    notes::save(&state.notes.get());
}

fn delete_note(state: &PanelState, id: u64) {
    state.notes.update(|list| list.retain(|n| n.id != id));
    notes::save(&state.notes.get());
}

/// Schedules a save `SAVE_DEBOUNCE` from now, superseding any pending one — only the latest scheduled save
/// (matching generation) writes, so a burst of keystrokes collapses to a single trailing write.
fn schedule_save(notes: ReadSignal<Vec<Note>>, generation: Rc<Cell<u64>>) {
    let seq = generation.get().wrapping_add(1);
    generation.set(seq);
    let guard = Rc::clone(&generation);
    timeout(SAVE_DEBOUNCE, move || {
        if guard.get() == seq {
            notes::save(&notes.get());
        }
    });
}

fn square_style() -> LayoutStyle {
    LayoutStyle::new()
        .width(36.0)
        .height(36.0)
        .flex_shrink(0.0)
        .align_items(AlignItems::CENTER)
        .justify_content(JustifyContent::CENTER)
}

fn pill_button(
    label: impl Fn() -> String + 'static,
    on_press: impl Fn() + 'static,
    theme: NordTheme,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let text = Text::auto(label, LayoutStyle::new(), move || {
        theme.text_style(FontRole::Caption, theme.text)
    })?;
    let rounded = corner::md();
    let pill = StyledContainer::new(
        LayoutStyle::new()
            .padding_horizontal(space::LG)
            .padding_vertical(space::SM),
        move |_| RectStyle::filled(theme.base, rounded),
        vec![Box::new(text) as Box<dyn LayoutItem>],
    )?
    .hover_style(move |_| RectStyle::filled(theme.overlay, rounded))
    .on_press(on_press);
    Ok(Box::new(pill))
}
