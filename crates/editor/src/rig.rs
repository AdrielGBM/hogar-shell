//! The running shell an editor test edits: a store holding the shipped layout under a name of its own, drawn on one screen (or several) through a real reconcile — reservation strips included — and installed as the running shell's, with the editor started over it.
#![cfg(test)]

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use config::Config;
use layout::{
    ActiveWorkspace, AreaId, Group, GroupId, GroupKind, Instance, InstanceId, LayerKind, Layout,
    LayoutId, LayoutStore, Representation,
};
use platform_wayland::OutputDescriptor;
use surfaces::layer_window::{Content, Demands, LayerWindowContext};
use surfaces::menu::Pointed;
use surfaces::reconcile::{Shell, plan};
use surfaces::rects::Node;
use surfaces::transient;
use telar::{DismissRegistration, LayoutItem};
use ui::descriptor::Built;
use ui::host::Host;

use crate::keys::{self, Press};
use crate::mode::{self, Compositor, Mode};
use crate::modes::container::{self, Frame};
use crate::modes::gesture::{self, Hint};
use crate::modes::widgets;

/// The one screen a rig draws on.
pub(crate) const SCREEN: &str = "DP-1";

pub(crate) struct Rig {
    pub(crate) store: Rc<RefCell<LayoutStore>>,
    /// How many times the screen has been brought in line with the store, which is the only thing that renegotiates what an edge reserves.
    pub(crate) reconciles: Rc<Cell<usize>>,
    _shell: Rc<RefCell<Shell>>,
}

impl Rig {
    pub(crate) fn undo_label(&self) -> Option<String> {
        self.store.borrow().undo_label().map(str::to_string)
    }
}

/// A store holding `mine` and drawing it, installed as the running shell's, with the editor started over it; `test` names its directory.
pub(crate) fn rig(test: &str) -> Rig {
    rig_with(test, |_| {})
}

/// [`rig`], with `mine` changed by `edit` before it is stored.
pub(crate) fn rig_with(test: &str, edit: impl FnOnce(&mut Layout)) -> Rig {
    rig_on(test, None, edit)
}

/// [`rig_with`], its screen showing the workspace called `workspace`.
pub(crate) fn rig_on(test: &str, workspace: Option<&str>, edit: impl FnOnce(&mut Layout)) -> Rig {
    rig_across(test, &[SCREEN], workspace, edit, |_| {})
}

/// [`rig_with`], its store given what `prepare` adds — komponents, the trust a bundle's files are held under — before the screen is first drawn from it.
pub(crate) fn rig_prepared(
    test: &str,
    edit: impl FnOnce(&mut Layout),
    prepare: impl FnOnce(&mut LayoutStore),
) -> Rig {
    rig_across(test, &[SCREEN], None, edit, prepare)
}

/// [`rig_with`], drawn on every screen `screens` names, side by side and each 1920 by 1080.
pub(crate) fn rig_screens(test: &str, screens: &[&str], edit: impl FnOnce(&mut Layout)) -> Rig {
    rig_across(test, screens, None, edit, |_| {})
}

fn rig_across(
    test: &str,
    screens: &[&str],
    workspace: Option<&str>,
    edit: impl FnOnce(&mut Layout),
    prepare: impl FnOnce(&mut LayoutStore),
) -> Rig {
    let workspace = workspace.map(|name| ActiveWorkspace {
        name: name.to_string(),
        id: None,
        special: None,
    });
    telar::reset_layout_runtime();
    telar::set_locale("en");
    telar::set_theme(Config::default().resolve_theme());
    let dir = util::paths::isolated_root()
        .expect("a test process resolves under its scratch root")
        .join(format!("editor-{test}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a layouts directory");
    let mut mine = layout::built_in();
    mine.id = LayoutId::new("mine");
    edit(&mut mine);
    std::fs::write(
        dir.join("mine.toml"),
        toml::to_string_pretty(&mine).expect("the layout serializes"),
    )
    .expect("a layout to edit");
    let (mut store, report) = LayoutStore::load(&dir);
    assert!(report.is_clean(), "{}", report.render());
    store.use_layout(&LayoutId::new("mine")).expect("mine");
    prepare(&mut store);
    let store = Rc::new(RefCell::new(store));
    let shell = Rc::new(RefCell::new(Shell::new()));
    let outputs: Vec<OutputDescriptor> = screens
        .iter()
        .enumerate()
        .map(|(at, name)| OutputDescriptor {
            name: Some(name.to_string()),
            logical_size: Some((1920, 1080)),
            position: (1920 * at as i32, 0),
            scale: 1,
        })
        .collect();
    let reconciles = Rc::new(Cell::new(0));
    let redraw: Rc<dyn Fn()> = {
        let reconciles = Rc::clone(&reconciles);
        let (store, shell) = (Rc::clone(&store), Rc::clone(&shell));
        Rc::new(move || {
            let desktops = {
                let store = store.borrow();
                plan(
                    &util::paths::config_dir().join("config.toml"),
                    &Arc::new(Config::default()),
                    store.active(),
                    store.all(),
                    &outputs,
                    &|_| workspace.clone(),
                )
                .0
            };
            shell.borrow_mut().reconcile(&desktops, Content::Changed);
            reconciles.set(reconciles.get() + 1);
        })
    };
    redraw();
    transient::install(shell.borrow().windows().holder());
    surfaces::layouts::install(Rc::clone(&store), redraw);
    crate::install();
    Rig {
        store,
        reconciles,
        _shell: shell,
    }
}

/// The card of the popover drawn in `tree`: the largest rectangle painted in the surface colour.
pub(crate) fn card_of(tree: &telar::ComponentList) -> telar::Rect {
    let surface = telar::use_theme::<config::theme::NordTheme>().surface;
    let mut found = Vec::new();
    telar::for_each_with_matrix(&tree.commands(), |command, [a, b, c, d, e, f]| {
        if let telar::DrawCommand::Rect { rect, style } = command
            && style.fill == Some(telar::Paint::Solid(surface))
        {
            found.push(telar::Rect::new(
                a * rect.x + c * rect.y + e,
                b * rect.x + d * rect.y + f,
                rect.width,
                rect.height,
            ));
        }
    });
    found
        .into_iter()
        .max_by(|a, b| a.height.total_cmp(&b.height))
        .expect("the card is drawn")
}

/// A turn of the wheel over `at` that takes the rows `pixels` further down, or up when negative.
pub(crate) fn wheel_at((x, y): (f32, f32), pixels: f32) -> telar::Event {
    telar::Event::Scrolled {
        delta: telar::ScrollDelta::Pixels { x: 0.0, y: -pixels },
        x: x.into(),
        y: y.into(),
    }
}

/// How much of the card, below a row's top, a row's own controls need to be in view.
pub(crate) const ROW_ROOM: f32 = 80.0;

/// Where to turn the wheel to bring a row at `row` into the card's visible rows, or nothing when it is already in them.
pub(crate) fn wheel_toward(card: telar::Rect, row: telar::Rect) -> Option<f32> {
    let header = 60.0;
    if row.y + row.height + ROW_ROOM > card.y + card.height {
        Some(60.0)
    } else if row.y < card.y + header {
        Some(-60.0)
    } else {
        None
    }
}

pub(crate) fn compositor() -> Compositor {
    Compositor {
        restack: true,
        locked: false,
        lockable: Ok(()),
    }
}

pub(crate) fn open_mode(layer: LayerKind) -> Mode {
    mode::enter_as(layer, Some(SCREEN), &compositor()).expect("the mode opens")
}

/// The edit mode of `layer` entered on the rig's screen, whatever it draws over the screen closed as the registration is dropped.
pub(crate) fn enter(layer: LayerKind) -> DismissRegistration {
    enter_on(layer, SCREEN)
}

pub(crate) fn enter_on(layer: LayerKind, output: &str) -> DismissRegistration {
    mode::enter_as(layer, Some(output), &compositor()).expect("the mode opens");
    let id = crate::host::transient_id(output);
    DismissRegistration::new(Rc::new(move || transient::close(&id)))
}

/// Lays `node` out over a window of `size` as the runner does each frame, until it settles: a card takes the height its rows were laid out at, which is known only once they are.
pub(crate) fn lay_out(node: telar::NodeId, (width, height): (f32, f32)) {
    telar::compute_layout(
        node,
        telar::AvailableSpace::Definite(width),
        telar::AvailableSpace::Definite(height),
    )
    .expect("it lays out");
    for _ in 0..3 {
        telar::relayout_if_dirty();
    }
}

pub(crate) fn pointer_at((x, y): (f32, f32)) -> telar::Event {
    telar::Event::PointerMoved {
        x: x.into(),
        y: y.into(),
        source: telar::PointerSource::Mouse,
    }
}

pub(crate) fn press_at((x, y): (f32, f32)) -> telar::Event {
    telar::Event::PointerPressed {
        x: x.into(),
        y: y.into(),
        button: telar::PointerButton::Primary,
        source: telar::PointerSource::Mouse,
    }
}

pub(crate) fn release_at((x, y): (f32, f32)) -> telar::Event {
    telar::Event::PointerReleased {
        x: x.into(),
        y: y.into(),
        button: telar::PointerButton::Primary,
        source: telar::PointerSource::Mouse,
    }
}

pub(crate) fn click_at(at: (f32, f32)) -> [telar::Event; 2] {
    [press_at(at), release_at(at)]
}

pub(crate) fn move_and_click(at: (f32, f32)) -> [telar::Event; 3] {
    let [press, release] = click_at(at);
    [pointer_at(at), press, release]
}

pub(crate) fn stored(rig: &Rig) -> Layout {
    rig.store.borrow().active().clone()
}

/// The area every rig's top bar is.
pub(crate) fn bar() -> Node {
    Node::area(Some(SCREEN), LayerKind::Top, &AreaId::new("bar-top"))
}

/// A module's chip or widget that takes room and shows nothing.
pub(crate) fn face(_: &Host) -> Built {
    Ok(Box::new(telar::StyledContainer::new(
        telar::LayoutStyle::new().width(40.0).height(20.0),
        |_| telar::RectStyle::default(),
        Vec::new(),
    )?))
}

/// `key` pressed and let go as the runner routes it: to the overlays first, and to the editor's keys when none takes it.
pub(crate) fn tap(key: telar::Key, modifiers: telar::ModifiersState) -> bool {
    let event = telar::Event::KeyPressed {
        key: key.clone(),
        modifiers,
    };
    telar::observe_keyboard(&event);
    let taken = telar::dispatch_overlays(&event) || keys::press_as(&key, modifiers, Press::First);
    telar::observe_keyboard(&telar::Event::KeyReleased { key, modifiers });
    keys::settle_released();
    taken
}

pub(crate) fn holding_alt(alt: bool) -> telar::Event {
    telar::Event::ModifiersChanged {
        modifiers: telar::ModifiersState {
            is_alt: alt,
            ..telar::ModifiersState::default()
        },
    }
}

pub(crate) fn hold_alt(held: bool) {
    telar::observe_keyboard(&holding_alt(held));
}

pub(crate) fn close(a: f32, b: f32) -> bool {
    close_within(a, b, 1e-5)
}

pub(crate) fn close_within(a: f32, b: f32, eps: f32) -> bool {
    (a - b).abs() < eps
}

thread_local! {
    static DRAWN: RefCell<Option<(telar::OwnerId, telar::ComponentList)>> = const { RefCell::new(None) };
}

/// The rig's screen as the windows show it now.
pub(crate) fn desktop() -> surfaces::reconcile::Desktop {
    surfaces::reconcile::desktop(Some(SCREEN)).expect("the rig's screen")
}

/// Takes down what [`draw`] drew.
pub(crate) fn undraw() {
    if let Some((owner, tree)) = DRAWN.with(|drawn| drawn.borrow_mut().take()) {
        drop(tree);
        telar::dispose_owner(owner);
    }
}

/// Draws the rig's screen's `layer` as its window would, from the arrangement the windows show now, so what the editor reads from the rect registry is there: the rig runs no compositor, so its windows build nothing. What was drawn before is taken down first.
pub(crate) fn draw(layer: LayerKind) {
    undraw();
    let scope = telar::owner_scope();
    telar::set_context(LayerWindowContext {
        layer,
        output: Some(SCREEN.to_string()),
        demands: Rc::new(Demands::new(platform_wayland::Layer::Top)),
        mapped: telar::signal(true).read_only(),
    });
    let drawn = surfaces::area::stand_in(&desktop(), layer).expect("the layer builds");
    let page = telar::Container::new(
        telar::LayoutStyle::new().width(1920.0).height(1080.0),
        vec![drawn],
    )
    .expect("a screen");
    let root = page.layout_node();
    let tree = telar::ComponentList::new(page);
    telar::compute_layout(
        root,
        telar::AvailableSpace::Definite(1920.0),
        telar::AvailableSpace::Definite(1080.0),
    )
    .expect("the layer lays out");
    DRAWN.with(|drawn| *drawn.borrow_mut() = Some((scope.id(), tree)));
}

/// No modifier held.
pub(crate) const NONE: telar::ModifiersState = telar::ModifiersState {
    is_shift: false,
    is_ctrl: false,
    is_alt: false,
    is_meta: false,
};

/// The modifiers `modifiers` holds down.
pub(crate) fn with(modifiers: fn(&mut telar::ModifiersState)) -> telar::ModifiersState {
    let mut held = NONE;
    modifiers(&mut held);
    held
}

/// A widget of `module` at size M.
pub(crate) fn widget(id: &str, module: &str) -> Instance {
    Instance {
        id: InstanceId::new(id),
        module: Some(module.to_string()),
        representation: Some(Representation::WidgetM),
        ..Instance::default()
    }
}

/// An empty group on the cells `cells` of a grid, as its column, row, columns and rows.
pub(crate) fn cell_group(id: &str, (col, row, cols, rows): (u32, u32, u32, u32)) -> Group {
    Group {
        id: GroupId::new(id),
        kind: Some(GroupKind::Cell {
            col,
            row,
            col_span: cols,
            row_span: rows,
        }),
        ..Group::default()
    }
}

pub(crate) fn centre(rect: telar::Rect) -> (f32, f32) {
    (rect.x + rect.width / 2.0, rect.y + rect.height / 2.0)
}

/// Checks one gesture is one undo entry: undoing it once puts the layout back as it was.
pub(crate) fn undoes_to(rig: &Rig, before: &Layout) {
    crate::session::undo().expect("one undo takes it back");
    assert_eq!(
        &stored(rig),
        before,
        "one undo puts back everything the gesture did"
    );
}

/// The container `group` of the desktop grid as it is drawn now.
pub(crate) fn frame(group: &str) -> Frame {
    container::frame_in(
        &widgets::grids(SCREEN, LayerKind::Desktop),
        &AreaId::new("widgets"),
        &GroupId::new(group),
    )
    .expect("a container on the grid")
}

/// Lays out what was relaid since the last frame, as the runner does each frame.
pub(crate) fn settle() {
    for _ in 0..3 {
        telar::relayout_if_dirty();
    }
}

/// Something built and laid out over the whole screen, which the pointer and the keys reach as the runner routes them.
pub(crate) struct Page(pub(crate) telar::ComponentList);

impl Page {
    pub(crate) fn of(built: Built) -> Self {
        Self::over(vec![built.expect("it builds")])
    }

    pub(crate) fn over(children: Vec<Box<dyn LayoutItem>>) -> Self {
        let (width, height) = (1920.0, 1080.0);
        let root = Pointed::new(Box::new(
            telar::Container::new(
                telar::LayoutStyle::new().width(width).height(height),
                children,
            )
            .expect("a page"),
        ));
        let node = root.layout_node();
        let tree = telar::ComponentList::new(root);
        telar::compute_layout(
            node,
            telar::AvailableSpace::Definite(width),
            telar::AvailableSpace::Definite(height),
        )
        .expect("it lays out");
        settle();
        Self(tree)
    }

    /// `event` routed to the overlays first, and to the page where none takes it.
    pub(crate) fn send(&mut self, event: telar::Event) {
        telar::observe_keyboard(&event);
        if !telar::dispatch_overlays(&event) {
            self.0.on_event(&event);
        }
        settle();
    }

    pub(crate) fn key(&mut self, key: telar::Key) {
        self.send(telar::Event::KeyPressed {
            key,
            modifiers: NONE,
        });
    }

    pub(crate) fn move_to(&mut self, at: (f32, f32)) {
        self.send(pointer_at(at));
    }

    pub(crate) fn button(&mut self, at: (f32, f32), pressed: bool) {
        self.send(match pressed {
            true => press_at(at),
            false => release_at(at),
        });
    }

    pub(crate) fn click(&mut self, at: (f32, f32)) {
        self.move_to(at);
        self.button(at, true);
        self.button(at, false);
    }

    /// A press at `from`, a move a little way off and then to `to`, and a release there; answers what the drag showed at the pointer just before it was let go.
    pub(crate) fn drag(&mut self, from: (f32, f32), to: (f32, f32)) -> Option<Hint> {
        self.move_to(from);
        self.button(from, true);
        self.move_to((from.0 + 8.0, from.1 + 8.0));
        self.move_to(to);
        let hint = gesture::hint().peek();
        self.button(to, false);
        hint
    }

    /// Where the text `wanted` is drawn, a little inside its start.
    pub(crate) fn at(&self, wanted: &str) -> (f32, f32) {
        let mut found = None;
        telar::for_each_with_matrix(&self.0.commands(), |command, [a, b, c, d, e, f]| {
            if let telar::DrawCommand::Text { text, rect, .. } = command
                && text.to_string() == wanted
            {
                found = Some((
                    a * (rect.x + 2.0) + c * (rect.y + rect.height / 2.0) + e,
                    b * (rect.x + 2.0) + d * (rect.y + rect.height / 2.0) + f,
                ));
            }
        });
        found.unwrap_or_else(|| panic!("{wanted:?} is drawn"))
    }
}
