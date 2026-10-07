//! The command palette: one list of everything an edit mode can do — every row of its key table that can run from a list, each way a row can go its own entry with its chord, every module and komponent the add palette offers, the strip's actions, the history, the other modes and Done — narrowed by what is typed. The arrows walk it, Enter or a press runs the entry against the selection, and Esc closes it. An entry the selection would refuse stays listed, dimmed, saying why.

use std::rc::Rc;

use telar::{
    Key, LayoutItem, LayoutStyle, NamedKey, NodeId, RectStyle, RwSignal, SizeDimension,
    StyledContainer, Text, box_item, signal, use_theme,
};

use config::theme::{FontRole, NordTheme};
use layout::LayerKind;
use platform_wayland::KeyboardMode;
use surfaces::transient::{self, Place, Spec};
use ui::chrome::Chrome;
use ui::descriptor::Built;
use util::search;

use crate::keys::{self, Chord, Row};
use crate::mode;
use crate::modes::palette::{self, Line, Pick};
use crate::popover::{self, Inset, Search, search_card};
use crate::session::{self, EditError};

pub const ID: &str = "editor:command-palette";

const WIDTH: f32 = 520.0;

#[derive(Clone)]
enum Act {
    Key(Row, Chord),
    Place(Pick),
    Press(fn()),
    Travel(isize),
    Switch(LayerKind),
    Leave,
}

/// One entry of the palette: what it is called, the chord or category beside it, and why it would be refused now, if it would.
#[derive(Clone)]
pub struct Entry {
    pub name: String,
    pub hint: String,
    pub refused: Option<String>,
    act: Act,
}

impl Entry {
    fn new(name: String, hint: String, act: Act) -> Self {
        Self {
            name,
            hint,
            refused: None,
            act,
        }
    }

    fn run(&self, layer: LayerKind) {
        match &self.act {
            Act::Key(row, chord) => mode::said(keys::perform(row, chord)),
            Act::Place(pick) => mode::said(palette::put_near(pick, layer)),
            Act::Press(press) => press(),
            Act::Travel(steps) => session::travel_saying(*steps),
            Act::Switch(to) => mode::switch(*to),
            Act::Leave => {
                mode::leave();
            }
        }
    }
}

/// Everything the palette lists in the mode of `layer`, in the order it lists it before anything is typed.
pub fn commands(layer: LayerKind) -> Vec<Entry> {
    let rows = keys::listed(layer);
    let palette_offered = rows.iter().any(|row| row.name == "widget-add");
    let mut all = Vec::new();
    for row in rows.into_iter().filter(Row::runnable_from_palette) {
        let name = (row.label)();
        let ways = row.ways();
        let alone = ways.len() == 1;
        for chord in ways {
            let hint = match alone {
                true => keys::spell(&row.keys),
                false => chord.spelled(),
            };
            let refused = keys::refusal(&row, &chord).map(|why| why.to_string());
            all.push(Entry {
                refused,
                ..Entry::new(name.clone(), hint, Act::Key(row.clone(), chord))
            });
        }
    }
    if palette_offered {
        all.extend(placements(layer));
    }
    let blocked = mode::current().and_then(|mode| mode.refused.map(|why| why.render()));
    all.extend(
        crate::host::toolbar_of(layer)
            .into_iter()
            .map(|(said, press)| Entry {
                refused: blocked.clone(),
                ..Entry::new(said(), String::new(), Act::Press(press))
            }),
    );
    all.extend(
        crate::host::strip_actions()
            .into_iter()
            .map(|(said, press)| Entry::new(said(), String::new(), Act::Press(press))),
    );
    all.extend(
        crate::history::lines()
            .into_iter()
            .filter(|line| !line.now)
            .map(|line| {
                Entry::new(
                    telar::t!("editor.command.history", step = line.label.clone()),
                    line.hint,
                    Act::Travel(line.steps),
                )
            }),
    );
    all.extend(
        LayerKind::ALL
            .into_iter()
            .filter(|other| *other != layer)
            .map(|other| {
                Entry::new(
                    telar::t!("editor.edit", layer = mode::name_of(other)),
                    String::new(),
                    Act::Switch(other),
                )
            }),
    );
    all.push(Entry::new(
        telar::t!("editor.done"),
        String::new(),
        Act::Leave,
    ));
    all
}

fn placements(layer: LayerKind) -> Vec<Entry> {
    let mut category = String::new();
    let mut placed = Vec::new();
    for line in palette::lines(layer, "") {
        match line {
            Line::Heading(said) => category = said,
            Line::Entry { pick, name, .. } => placed.push(Entry::new(
                telar::t!("editor.command.add", name = name),
                category.clone(),
                Act::Place(pick),
            )),
        }
    }
    placed
}

/// What `query` finds among `commands`, the best match first: its letters in order in a name, word starts counting most, or the whole of it in a chord or a category. Between equal matches what would run comes before what would be refused, then the order they were listed in.
pub fn found(commands: &[Entry], query: &str) -> Vec<Entry> {
    let query = query.trim();
    let mut scored: Vec<(i32, bool, usize)> = commands
        .iter()
        .enumerate()
        .filter_map(|(at, command)| {
            let score = search::score(&command.name, query, search::Mode::Fuzzy)
                .or_else(|| search::score(&command.hint, query, search::Mode::Substring))?;
            Some((score, command.refused.is_some(), at))
        })
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
    scored
        .into_iter()
        .map(|(_, _, at)| commands[at].clone())
        .collect()
}

/// Opens the palette on the screen being edited, closing whichever popover, menu or card was open.
pub(crate) fn open() -> Result<(), EditError> {
    let mode = mode::required()?;
    crate::host::close_transients();
    let (output, layer) = (mode.output.clone(), mode.layer);
    transient::open(
        Spec::new(
            ID,
            Place::Whole,
            Rc::new(move |_: &Chrome| tree(&output, layer)),
        )
        .output(Some(mode.output.clone()))
        .keyboard(KeyboardMode::Exclusive)
        .dismiss_on_outside(),
    );
    Ok(())
}

/// Closes the palette, then runs `command`; one that would be refused does nothing.
fn choose(command: &Entry, layer: LayerKind) {
    if command.refused.is_some() {
        return;
    }
    transient::close(ID);
    command.run(layer);
}

/// The palette's card: a title, the search field and the entries `query` finds, centred at the top of what the reserving areas leave, the entry the arrows point at kept in view.
pub(crate) fn tree(output: &str, layer: LayerKind) -> Built {
    let listed = Rc::new(commands(layer));
    let query = signal(String::new());
    let pointed = signal(0usize);
    let shown = {
        let listed = Rc::clone(&listed);
        Rc::new(move || found(&listed, &query.peek()))
    };
    search_card(
        Search {
            output,
            width: WIDTH,
            inset: Inset::Centre,
            title: Box::new(|| telar::t!("editor.command.title")),
            query,
        },
        move || vec![(query.get(), pointed.get())],
        move |(typed, at): (String, usize), pointed_row| {
            entries(found(&listed, &typed), (at, pointed_row), layer)
        },
        move |key: &Key| {
            popover::answer(key, (query, pointed), &|| shown().len(), &|key| match key {
                Key::Named(NamedKey::Enter) => {
                    if let Some(command) = shown().get(pointed.peek()) {
                        choose(command, layer);
                    }
                    true
                }
                _ => false,
            })
        },
    )
}

/// The entries as rows, the one `at` counts to highlighted and its node told to `pointed_row`, which keeps it in view.
fn entries(
    found: Vec<Entry>,
    (at, pointed_row): (usize, RwSignal<Option<NodeId>>),
    layer: LayerKind,
) -> Built {
    let mut built: Vec<Box<dyn LayoutItem>> = Vec::with_capacity(found.len());
    let mut pointed = None;
    for (index, command) in found.into_iter().enumerate() {
        let row = row_of(command, index == at, layer)?;
        if index == at {
            pointed = Some(row.layout_node());
        }
        built.push(row);
    }
    popover::listing(built, pointed, pointed_row)
}

fn row_of(command: Entry, pointed: bool, layer: LayerKind) -> Built {
    let theme = use_theme::<NordTheme>();
    let refused = command.refused.is_some();
    let ink = match refused {
        true => theme.muted,
        false => theme.text,
    };
    let said = command.name.clone();
    let name = Text::declaring(
        move || said.clone(),
        LayoutStyle::new().flex_grow(1.0).flex_shrink(1.0),
        move |inherited| theme.text_over(inherited, FontRole::Body, ink),
    )?;
    let hinted = command.hint.clone();
    let hint = Text::declaring(
        move || hinted.clone(),
        LayoutStyle::new().flex_shrink(0.0),
        move |inherited| theme.text_over(inherited, FontRole::Caption, theme.subtle),
    )?;
    let line = telar::Container::new(
        LayoutStyle::new()
            .flex_row()
            .gap(ui::scale::space::sm())
            .align_items(telar::AlignItems::CENTER)
            .width(SizeDimension::Percent(1.0)),
        vec![box_item(name), box_item(hint)],
    )?;
    let mut children: Vec<Box<dyn LayoutItem>> = vec![box_item(line)];
    if let Some(why) = command.refused.clone() {
        children.push(box_item(Text::declaring(
            move || why.clone(),
            LayoutStyle::new(),
            move |inherited| theme.text_over(inherited, FontRole::Caption, theme.muted),
        )?));
    }
    let fill = match pointed {
        true => theme.overlay,
        false => theme.surface,
    };
    Ok(Box::new(
        StyledContainer::new(
            LayoutStyle::new()
                .flex_column()
                .gap(ui::scale::space::xs())
                .padding_all(ui::scale::space::xs())
                .width(SizeDimension::Percent(1.0)),
            move |_| RectStyle::filled(fill, ui::scale::corner::xs()),
            children,
        )?
        .hover_style(move |_| RectStyle::filled(theme.overlay, ui::scale::corner::xs()))
        .control(telar::Role::Button)
        .cursor(telar::Cursor::Pointer)
        .on_press(move || choose(&command, layer))
        .disabled(move || refused)
        .disabled_style(move |_| RectStyle::filled(fill, ui::scale::corner::xs())),
    ))
}
