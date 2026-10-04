//! The trust dialog (DEC-30): every item of an imported bundle that waits for an answer, with its exact text, where it is written and what it says about the lock screen, each accepted or declined on its own or a bundle's at once.
//!
//! **Closing decides nothing.** Esc, "Decide later" and the session locking all close it and leave every item waiting, so there is no way out of it that runs something. It has no default button: Enter answers only the control the keyboard was moved to.
//!
//! **Where it lives.** An unanchored transient, so it opens in the overlay window of the focused screen only while it is up (DEC-9, F-6.1), takes the keyboard, and holds Tab and the arrows inside itself. It never opens while the session is locked, and the lock closes it (TA-8).
//!
//! **When it opens.** Right after `layout import` brings something to answer for, from the notice's "Review…", and from `hogar-shell layout trust --dialog`. What waits at startup raises the notice alone ([`install`]), which follows how much waits and goes once nothing does.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use telar::{
    AlignItems, Children, Container, JustifyContent, Key, LayoutError, LayoutItem,
    LayoutScrollArea, LayoutStyle, NamedKey, NodeId, ReactiveList, RectStyle, RwSignal,
    SizeDimension, StyledContainer, Text, box_item, detached, effect, focus, on_cleanup, signal,
    use_theme,
};

use config::theme::{FontRole, NordTheme};
use layout::{Item, ItemKind, Within};
use platform_wayland::KeyboardMode;
use services::notifications::{Urgency, notify_status, withdraw_status};
use ui::chrome::{Chrome, content_radius, panel_fill};
use ui::descriptor::Built;
use ui::scale::{corner, paint, space};
use util::report::Message;

use crate::bundles;
use crate::transient::{self, Motion, Place, Slot, Spec};

/// The transient the dialog is.
pub const ID: &str = "trust";

/// The request line that opens the dialog: the notice's "Review…" button runs it.
pub const REVIEW: &str = "layout trust --dialog";

pub(crate) const WIDTH: f32 = 560.0;

/// The share of the screen's height the dialog may take before its list scrolls.
const TALLEST: f32 = 0.85;

/// One imported bundle and what of it waits for an answer, each item once.
#[derive(Clone, Debug, PartialEq)]
pub struct Waiting {
    pub bundle: String,
    pub items: Vec<Item>,
}

/// What waits for an answer, by bundle in name order, leaving out a bundle with nothing waiting.
pub fn waiting() -> Result<Vec<Waiting>, Message> {
    Ok(bundles::bundles()?
        .into_iter()
        .map(|bundle| {
            let mut items: Vec<Item> = Vec::new();
            for item in bundle.pending() {
                if !items.contains(item) {
                    items.push(item.clone());
                }
            }
            Waiting {
                bundle: bundle.name,
                items,
            }
        })
        .filter(|waiting| !waiting.items.is_empty())
        .collect())
}

fn waiting_count() -> usize {
    waiting()
        .map(|all| all.iter().map(|bundle| bundle.items.len()).sum())
        .unwrap_or(0)
}

/// Opens the dialog on the focused screen, answering how many items it lists; refused while the session is locked and while nothing waits. Open already, it stays as it is.
pub fn open() -> Result<usize, Message> {
    open_unless(services::lock::locked())
}

pub(crate) fn open_unless(locked: bool) -> Result<usize, Message> {
    if locked {
        return Err(util::message!("finding.trust_dialog_locked"));
    }
    let count = waiting()?.iter().map(|bundle| bundle.items.len()).sum();
    if count == 0 {
        return Err(util::message!("finding.trust_nothing_waits"));
    }
    if transient::is_open(ID) {
        return Ok(count);
    }
    let output = transient::focused_output();
    transient::open(
        Spec::new(ID, Place::Centred, Rc::new(tree))
            .slot(Slot::Standing)
            .output(output)
            .keyboard(KeyboardMode::Exclusive)
            .motion(Motion::Fade),
    );
    Ok(count)
}

/// Closes the dialog, answering nothing.
pub fn close() {
    transient::close(ID);
}

pub fn is_open() -> bool {
    transient::is_open(ID)
}

thread_local! {
    static INSTALLED: Cell<bool> = const { Cell::new(false) };
    static NOTICE: RefCell<Notice> = RefCell::new(Notice::default());
}

/// The notice that says something waits: which of the daemon's notices it is, and how many items it last counted.
#[derive(Default)]
struct Notice {
    id: Option<u32>,
    counted: usize,
}

/// Starts following what waits, on the driver thread once the layout store is installed: the notice is raised as soon as anything waits — at startup too, where it is all there is — popped again when more comes to wait, and withdrawn with the dialog once nothing does. The session locking closes the dialog. A second call changes nothing.
pub fn install() {
    if INSTALLED.with(|installed| installed.replace(true)) {
        return;
    }
    detached(|| {
        close_while(services::lock::locked);
        effect(|| {
            crate::layouts::revision().get();
            bundles::pending().get();
            follow(waiting_count());
        });
    });
}

/// Closes the dialog whenever `locked` holds, read reactively.
pub(crate) fn close_while(locked: impl Fn() -> bool + 'static) {
    effect(move || {
        if locked() {
            close();
        }
    });
}

fn follow(count: usize) {
    let raise = NOTICE.with(|notice| {
        let mut notice = notice.borrow_mut();
        let grew = count > notice.counted;
        notice.counted = count;
        match count {
            0 => {
                if let Some(id) = notice.id.take() {
                    withdraw_status(id);
                }
                false
            }
            _ => grew,
        }
    });
    if count == 0 {
        close();
    }
    if !raise {
        return;
    }
    let review = telar::t!("notice.trust_review");
    let id = notify_status(
        NOTICE.with(|notice| notice.borrow().id),
        "hogar-shell",
        &telar::t!("notice.trust_title", count = count),
        &telar::t!("notice.trust_body"),
        "dialog-question",
        Urgency::Normal,
        &[(REVIEW, &review)],
    );
    NOTICE.with(|notice| notice.borrow_mut().id = id);
}

/// How many items the notice last counted as waiting: none while it is down.
pub fn noticed() -> usize {
    NOTICE.with(|notice| notice.borrow().counted)
}

/// One row of the dialog's list: a bundle's heading with the items listed under it, or one item of it. A row answers for exactly what it holds, so an item that came to wait after the row was built is never answered by it: the key names what the row holds, and a list that changes builds a new row.
#[derive(Clone)]
enum Row {
    Bundle(String, Vec<Item>),
    Item(String, Item),
}

impl Row {
    fn key(&self) -> String {
        match self {
            Row::Bundle(bundle, items) => {
                format!("bundle\0{bundle}\0{}", layout::set_of(items))
            }
            Row::Item(bundle, item) => format!("item\0{bundle}\0{}", item.id()),
        }
    }
}

fn rows() -> Vec<Row> {
    waiting()
        .unwrap_or_default()
        .into_iter()
        .flat_map(|waiting| {
            let bundle = waiting.bundle;
            std::iter::once(Row::Bundle(bundle.clone(), waiting.items.clone())).chain(
                waiting
                    .items
                    .into_iter()
                    .map(move |item| Row::Item(bundle.clone(), item)),
            )
        })
        .collect()
}

/// What the dialog holds while it is built: why the last answer was refused, and where each row is, to give the keyboard back to the row that takes the place of one answered from it.
#[derive(Clone)]
struct Held {
    said: RwSignal<Option<String>>,
    nodes: Rc<RefCell<HashMap<String, NodeId>>>,
}

impl Held {
    /// Records the answer `decide` gives for the row `key`. Given from the keyboard, the keyboard moves on to the row now where that one was.
    fn answer(&self, key: &str, decide: impl FnOnce() -> Result<Vec<Item>, Message>) {
        let from_keyboard = focus::current().is_some_and(focus::is_focus_visible);
        let at = rows().iter().position(|row| row.key() == key);
        match decide() {
            Ok(_) => self.said.set(None),
            Err(why) => {
                self.said.set(Some(why.render()));
                return;
            }
        }
        if !from_keyboard {
            return;
        }
        let after = rows();
        let next = at.and_then(|at| after.get(at.min(after.len().saturating_sub(1))));
        let node = next.and_then(|row| self.nodes.borrow().get(&row.key()).copied());
        if let Some(node) = node {
            focus::focus_first_in(node);
        }
    }
}

fn tree(chrome: &Chrome) -> Built {
    let theme = use_theme::<NordTheme>();
    let pad = space::xl();
    let inner = WIDTH - 2.0 * pad;
    let held = Held {
        said: signal(None),
        nodes: Rc::new(RefCell::new(HashMap::new())),
    };

    let title = Text::new(
        || telar::t!("trust.title"),
        LayoutStyle::new().width(SizeDimension::Percent(1.0)),
        move || {
            theme
                .text_style(FontRole::Title, theme.text)
                .with_font_weight(700)
        },
    )?;
    let intro = Text::new(
        || telar::t!("trust.intro"),
        LayoutStyle::new().width(SizeDimension::Percent(1.0)),
        move || theme.text_style(FontRole::Body, theme.subtle),
    )?;
    let header = Container::new(
        LayoutStyle::new()
            .flex_column()
            .gap(space::sm())
            .width(SizeDimension::Percent(1.0)),
        vec![box_item(title), box_item(intro)],
    )?;
    let header_height = tracked(&header, "header")?;

    let built = held.clone();
    let list = ReactiveList::with_style(
        LayoutStyle::new()
            .flex_column()
            .gap(space::md())
            .width(inner),
        move || {
            crate::layouts::revision().get();
            rows()
        },
        Row::key,
        move |row| row_of(row, &built, theme),
    )?;
    let content = tracked(&list, "list")?;
    let viewport = Rc::new(RefCell::new(None));
    let scroll = {
        let captured = Rc::clone(&viewport);
        LayoutScrollArea::new_with(
            LayoutStyle::new()
                .width(inner)
                .height(SizeDimension::Percent(1.0)),
            move |scrolling| {
                *captured.borrow_mut() = Some(scrolling);
                Ok(box_item(list))
            },
        )?
    };
    if let Some(viewport) = viewport.take() {
        keep_focus_in_view(viewport);
    }

    let footer = footer(&held, theme)?;
    let footer_height = tracked(&footer, "footer")?;
    let output = chrome.output.clone();
    let gap = space::lg();
    let room = move || {
        let usable = crate::reconcile::desktop(output.as_deref())
            .map(|desktop| desktop.reserved.box_of(Within::Usable, desktop.size));
        usable.map(|usable| {
            (usable.height * TALLEST
                - header_height.get().height
                - footer_height.get().height
                - 2.0 * gap
                - 2.0 * pad)
                .max(0.0)
        })
    };
    let fitted = move || {
        let wanted = content.get().height;
        LayoutStyle::new()
            .width(inner)
            .height(room().map_or(wanted, |room| wanted.min(room)))
    };
    let list_box =
        StyledContainer::new(fitted(), |_| RectStyle::default(), vec![Box::new(scroll)])?
            .styled_by(fitted);

    let card = StyledContainer::new(
        LayoutStyle::new()
            .flex_column()
            .gap(gap)
            .width(WIDTH)
            .padding_all(pad),
        move |_| RectStyle::filled(panel_fill(), content_radius()),
        vec![box_item(header), box_item(list_box), box_item(footer)],
    )?
    .input_opaque()
    .on_key(steer);
    let scope = focus::register_scope(card.layout_node(), || true, true);
    on_cleanup(move || focus::unregister_scope(scope));
    Ok(Box::new(card))
}

/// Tab and the arrows walk the dialog's controls: the arrows always, Tab only while none holds the keyboard, since a control that does steps on its own.
fn steer(key: &Key) -> bool {
    let backwards = || {
        focus::focus_prev();
        true
    };
    let forwards = || {
        focus::focus_next();
        true
    };
    match key {
        Key::Named(NamedKey::ArrowDown | NamedKey::ArrowRight) => forwards(),
        Key::Named(NamedKey::ArrowUp | NamedKey::ArrowLeft) => backwards(),
        Key::Named(NamedKey::Tab) if focus::current().is_none() => {
            match telar::modifiers().is_shift {
                true => backwards(),
                false => forwards(),
            }
        }
        _ => false,
    }
}

fn tracked(item: &dyn LayoutItem, what: &str) -> Result<RwSignal<telar::Rect>, LayoutError> {
    telar::track_layout(item.layout_node())
        .ok_or_else(|| LayoutError::Engine(format!("the trust dialog's {what} has no layout node")))
}

/// Scrolls the list so the control holding the keyboard is on screen.
fn keep_focus_in_view(viewport: telar::ScrollViewport) {
    effect(move || {
        let Some(focused) = focus::current() else {
            return;
        };
        let Some(control) = focus::exposed().into_iter().find(|at| at.id == focused) else {
            return;
        };
        let inside = telar::enclosing_scroll_viewport(control.node)
            .is_some_and(|found| found.area() == viewport.area());
        if inside {
            viewport.reveal(control.node, space::md());
        }
    });
}

fn row_of(row: Row, held: &Held, theme: NordTheme) -> Built {
    let key = row.key();
    let built = match row {
        Row::Bundle(bundle, items) => bundle_row(bundle, items, &key, held, theme)?,
        Row::Item(bundle, item) => item_row(bundle, item, &key, held, theme)?,
    };
    held.nodes
        .borrow_mut()
        .insert(key.clone(), built.layout_node());
    let nodes = Rc::clone(&held.nodes);
    on_cleanup(move || {
        nodes.borrow_mut().remove(&key);
    });
    Ok(built)
}

fn bundle_row(bundle: String, items: Vec<Item>, key: &str, held: &Held, theme: NordTheme) -> Built {
    let heading = Text::new(
        {
            let (named, count) = (bundle.clone(), items.len());
            move || telar::t!("trust.bundle", bundle = named.clone(), count = count)
        },
        LayoutStyle::new().flex_grow(1.0).flex_shrink(1.0),
        move || {
            theme
                .text_style(FontRole::Body, theme.text)
                .with_font_weight(700)
        },
    )?;
    let buttons = answers(
        (key, held),
        theme,
        (|| telar::t!("trust.decline_all"), {
            let (bundle, items) = (bundle.clone(), items.clone());
            move || bundles::decline(&bundle, &items)
        }),
        (
            || telar::t!("trust.accept_all"),
            move || bundles::accept(&bundle, &items),
        ),
    )?;
    Ok(box_item(Container::new(
        LayoutStyle::new()
            .flex_row()
            .align_items(AlignItems::CENTER)
            .gap(space::md())
            .width(SizeDimension::Percent(1.0)),
        vec![box_item(heading), buttons],
    )?))
}

/// What an item runs as its row draws it: written out where it could disguise itself ([`Item::shown`]), and laid out left to right as one block ([`util::text::isolated`]), since telar's text takes its direction from what it holds and a right-to-left letter in a command would carry the pipes and paths around it into another order.
pub(crate) fn shown_command(item: &Item) -> String {
    util::text::isolated(&item.shown())
}

fn item_row(bundle: String, item: Item, key: &str, held: &Held, theme: NordTheme) -> Built {
    let kind = item.kind;
    let mut column: Vec<Box<dyn LayoutItem>> = vec![
        box_item(Text::new(
            move || kind_label(kind),
            LayoutStyle::new().width(SizeDimension::Percent(1.0)),
            move || {
                theme
                    .text_style(FontRole::Body, theme.text)
                    .with_font_weight(700)
            },
        )?),
        box_item(Text::new(
            {
                let (file, at) = (util::text::shown(&item.file), util::text::shown(&item.key));
                move || telar::t!("trust.where", file = file.clone(), key = at.clone())
            },
            LayoutStyle::new().width(SizeDimension::Percent(1.0)),
            move || theme.text_style(FontRole::Caption, theme.subtle),
        )?),
    ];
    if item.lock_safe {
        column.push(box_item(Text::new(
            || telar::t!("trust.lock_safe"),
            LayoutStyle::new().width(SizeDimension::Percent(1.0)),
            move || theme.text_style(FontRole::Caption, theme.warning),
        )?));
    }
    let text = shown_command(&item);
    column.push(box_item(StyledContainer::new(
        LayoutStyle::new()
            .width(SizeDimension::Percent(1.0))
            .padding_horizontal(space::md())
            .padding_vertical(space::sm()),
        paint::xs(theme.base),
        vec![box_item(Text::new(
            move || text.clone(),
            LayoutStyle::new().width(SizeDimension::Percent(1.0)),
            move || {
                theme
                    .text_style(FontRole::Body, theme.text)
                    .with_font_family(telar::FontFamily::Monospace)
            },
        )?)],
    )?));
    let buttons = answers(
        (key, held),
        theme,
        (|| telar::t!("trust.decline"), {
            let (bundle, item) = (bundle.clone(), item.clone());
            move || bundles::decline(&bundle, std::slice::from_ref(&item))
        }),
        (
            || telar::t!("trust.accept"),
            move || bundles::accept(&bundle, std::slice::from_ref(&item)),
        ),
    )?;
    column.push(buttons);
    Ok(box_item(StyledContainer::new(
        LayoutStyle::new()
            .flex_column()
            .gap(space::sm())
            .width(SizeDimension::Percent(1.0))
            .padding_all(space::md()),
        move |_| RectStyle::filled(theme.overlay, corner::md()),
        column,
    )?))
}

fn kind_label(kind: ItemKind) -> String {
    match kind {
        ItemKind::Poll => telar::t!("trust.kind.poll"),
        ItemKind::Listen => telar::t!("trust.kind.listen"),
        ItemKind::Http => telar::t!("trust.kind.http"),
        ItemKind::Action => telar::t!("trust.kind.action"),
    }
}

type Label = fn() -> String;

/// Decline then Accept, at the end of the row `key`: Accept is the filled one, and neither is pressed for the user by Enter or by closing.
fn answers(
    (key, held): (&str, &Held),
    theme: NordTheme,
    decline: (Label, impl Fn() -> Result<Vec<Item>, Message> + 'static),
    accept: (Label, impl Fn() -> Result<Vec<Item>, Message> + 'static),
) -> Built {
    let press = |decide: Rc<dyn Fn() -> Result<Vec<Item>, Message>>| {
        let (key, held) = (key.to_string(), held.clone());
        Rc::new(move || held.answer(&key, || decide())) as Rc<dyn Fn()>
    };
    let declining = telar::button(
        telar::ButtonProps::props()
            .label(telar::Reactive::of(decline.0))
            .outline(telar::Reactive::of(move || theme.muted))
            .on_press(press(Rc::new(decline.1)))
            .build(),
        Children::default(),
    )?;
    let accepting = telar::button(
        telar::ButtonProps::props()
            .label(telar::Reactive::of(accept.0))
            .on_press(press(Rc::new(accept.1)))
            .build(),
        Children::default(),
    )?;
    Ok(box_item(Container::new(
        LayoutStyle::new()
            .flex_row()
            .justify_content(JustifyContent::END)
            .align_items(AlignItems::CENTER)
            .gap(space::sm())
            .flex_shrink(0.0),
        vec![declining, accepting],
    )?))
}

/// The keys, why the last answer was refused, and the way out that answers nothing.
fn footer(held: &Held, theme: NordTheme) -> Result<Container, LayoutError> {
    let said = held.said;
    let refusal = Text::new(
        move || said.get().unwrap_or_default(),
        LayoutStyle::new().width(SizeDimension::Percent(1.0)),
        move || theme.text_style(FontRole::Caption, theme.error),
    )?;
    let keys = Text::new(
        || telar::t!("trust.keys"),
        LayoutStyle::new().flex_grow(1.0).flex_shrink(1.0),
        move || theme.text_style(FontRole::Caption, theme.subtle),
    )?;
    let later = telar::button(
        telar::ButtonProps::props()
            .label(telar::Reactive::of(|| telar::t!("trust.later")))
            .ghost(true)
            .on_press(Rc::new(close))
            .build(),
        Children::default(),
    )?;
    let row = Container::new(
        LayoutStyle::new()
            .flex_row()
            .align_items(AlignItems::CENTER)
            .gap(space::md())
            .width(SizeDimension::Percent(1.0)),
        vec![box_item(keys), later],
    )?;
    Container::new(
        LayoutStyle::new()
            .flex_column()
            .gap(space::sm())
            .width(SizeDimension::Percent(1.0)),
        vec![box_item(refusal), box_item(row)],
    )
}
