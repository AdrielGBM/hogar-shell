//! What an edit must leave behind: exactly what was there before when it is reverted, however it was reverted, and exactly one entry in the one history when it is committed, whichever part of the layout it changed.
//!
//! Driven the way a tool drives it — a box with a transacted drag fed real pointer events, a popover registered on the dismiss stack — against a store installed as the running shell's, redrawn through a real reconcile.

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use telar::{
        AvailableSpace, Component, Event, Key, LayoutItem, LayoutStyle, ModifiersState, NamedKey,
        PointerButton, PointerSource, RectStyle, StyledContainer, compute_layout, signal,
    };

    use layout::{
        AreaId, GroupId, Instance, InstanceId, LayerKind, Layout, LayoutOp, LayoutStore, Site, Spot,
    };
    use surfaces::reconcile;
    use surfaces::rects::Node;

    use crate::mode::{self, Compositor};
    use crate::rig::{SCREEN, rig};
    use crate::session::{self, Edit, EditError, Selection, Way};

    const MOVE: &str = "Move the clock";

    fn zone(group: &str) -> Spot {
        Spot {
            site: Site::everywhere(LayerKind::Top),
            area: AreaId::new("bar-top"),
            group: GroupId::new(group),
        }
    }

    fn move_clock(to: &str) -> LayoutOp {
        LayoutOp::MoveInstance {
            from: zone("center"),
            to: zone(to),
            id: InstanceId::new("clock"),
            index: 0,
        }
    }

    /// The zone of the top bar the clock is drawn in on screen right now.
    fn clock_zone() -> Option<String> {
        reconcile::desktops()[0]
            .resolved
            .layer(LayerKind::Top)?
            .areas
            .iter()
            .flat_map(|area| area.groups.iter())
            .find(|group| {
                group
                    .children
                    .iter()
                    .any(|child| child.id.as_str() == "clock")
            })
            .map(|group| group.id.to_string())
    }

    /// A 100 px box whose drag moves the clock to the start zone on the left half and the end zone on the right half, the way a tool previews into its edit on every move.
    fn clock_handle(edit: &Edit) -> StyledContainer {
        let previewing = edit.clone();
        let handle = StyledContainer::new(
            LayoutStyle::new().width(100.0).height(100.0),
            |_| RectStyle::default(),
            Vec::new(),
        )
        .expect("a handle")
        .on_drag(move |x, _| {
            let to = if x < 50.0 { "start" } else { "end" };
            previewing
                .preview(vec![move_clock(to)])
                .expect("it previews");
        })
        .drag_transaction(edit.transaction());
        compute_layout(
            handle.layout_node(),
            AvailableSpace::Definite(100.0),
            AvailableSpace::Definite(100.0),
        )
        .expect("the handle lays out");
        handle
    }

    fn press(x: f64, y: f64) -> Event {
        Event::PointerPressed {
            x,
            y,
            button: PointerButton::Primary,
            source: PointerSource::Mouse,
        }
    }

    fn to(x: f64, y: f64) -> Event {
        Event::PointerMoved {
            x,
            y,
            source: PointerSource::Mouse,
        }
    }

    fn release(x: f64, y: f64) -> Event {
        Event::PointerReleased {
            x,
            y,
            button: PointerButton::Primary,
            source: PointerSource::Mouse,
        }
    }

    fn escape() -> Event {
        Event::KeyPressed {
            key: Key::Named(NamedKey::Escape),
            modifiers: ModifiersState::default(),
        }
    }

    /// F-7's modal-operator rule: Esc mid-drag puts back exactly what was on screen — the very arrangement the last reconcile published, not an equal copy — and records nothing.
    #[test]
    fn escape_during_a_drag_restores_exactly_what_was_on_screen() {
        let rig = rig("session-escape");
        let before = reconcile::desktops();
        let edit = Edit::new(MOVE);
        let mut handle = clock_handle(&edit);

        handle.on_event(&press(10.0, 10.0));
        handle.on_event(&to(80.0, 10.0));
        assert_eq!(
            clock_zone().as_deref(),
            Some("end"),
            "the drag previews live"
        );
        assert!(edit.is_open());

        assert!(telar::dispatch_overlays(&escape()), "Esc cancels the drag");
        assert!(Rc::ptr_eq(&reconcile::desktops(), &before));
        assert!(reconcile::previewing().is_none());
        assert!(!edit.is_open());
        assert_eq!(rig.undo_label(), None, "nothing was recorded");

        handle.on_event(&release(90.0, 10.0));
        assert_eq!(
            rig.undo_label(),
            None,
            "the release after it commits nothing"
        );
        assert_eq!(clock_zone().as_deref(), Some("center"));
    }

    /// A release commits what the drag last previewed as one transaction and one undo entry, and undoing it puts the arrangement back as it was.
    #[test]
    fn a_released_drag_is_one_undo_entry_and_undoing_it_restores_the_layout() {
        let rig = rig("session-release");
        let before = reconcile::desktops();
        let edit = Edit::new(MOVE);
        let mut handle = clock_handle(&edit);

        handle.on_event(&press(10.0, 10.0));
        handle.on_event(&to(30.0, 10.0));
        handle.on_event(&to(80.0, 10.0));
        handle.on_event(&release(80.0, 10.0));

        assert_eq!(rig.undo_label().as_deref(), Some(MOVE));
        assert!(reconcile::previewing().is_none(), "the store draws it now");
        assert_eq!(clock_zone().as_deref(), Some("end"));

        assert_eq!(session::undo().as_deref(), Ok(MOVE));
        assert_eq!(rig.undo_label(), None, "it was one entry");
        assert_eq!(reconcile::desktops()[0].resolved, before[0].resolved);
    }

    /// The clock as the layout holds it, with `options` for its own.
    fn clock_with(layout: &Layout, options: toml::Table) -> Instance {
        let group = layout::ops::layer(layout, &zone("center").site)
            .expect("the top layer")
            .areas
            .iter()
            .find(|area| area.id.as_str() == "bar-top")
            .and_then(|area| {
                area.groups
                    .iter()
                    .find(|group| group.id.as_str() == "center")
            })
            .expect("the centre zone");
        Instance {
            options,
            ..group
                .children
                .iter()
                .find(|child| child.id.as_str() == "clock")
                .expect("the clock")
                .clone()
        }
    }

    fn clock_options() -> Option<toml::Table> {
        reconcile::desktops()[0]
            .resolved
            .instances()
            .find(|instance| instance.id.as_str() == "clock")
            .map(|instance| instance.options.clone())
    }

    /// TA-4: an instance's options and where it is placed are undone from one history, in the order they were made.
    #[test]
    fn an_option_change_and_a_move_undo_from_one_stack() {
        let rig = rig("session-one-stack");
        let before = reconcile::desktops();
        let format = toml::Table::from_iter([("format".to_string(), "%H".into())]);

        let restyled = session::begin("Set the clock's format").expect("it opens");
        let clock = clock_with(rig.store.borrow().active(), format.clone());
        restyled
            .preview(vec![LayoutOp::SetInstance {
                spot: zone("center"),
                id: InstanceId::new("clock"),
                instance: Box::new(clock),
            }])
            .expect("it previews");
        restyled.commit().expect("it commits");
        let moved = session::begin(MOVE).expect("it opens");
        moved.preview(vec![move_clock("end")]).expect("it previews");
        moved.commit().expect("it commits");
        assert_eq!(
            (clock_zone().as_deref(), clock_options()),
            (Some("end"), Some(format.clone()))
        );

        assert_eq!(session::undo().as_deref(), Ok(MOVE));
        assert_eq!(
            (clock_zone().as_deref(), clock_options()),
            (Some("center"), Some(format))
        );
        assert_eq!(session::undo().as_deref(), Ok("Set the clock's format"));
        assert_eq!(reconcile::desktops()[0].resolved, before[0].resolved);
        assert_eq!(rig.undo_label(), None);

        assert!(session::redo().is_ok() && session::redo().is_ok());
        assert_eq!(clock_zone().as_deref(), Some("end"));
    }

    /// F-7's popover convention through the dismiss stack: Esc reverts to what was there when it opened, and closing it any other way commits.
    #[test]
    fn a_popover_reverts_on_escape_and_commits_on_any_other_close() {
        let rig = rig("session-popover");
        let before = reconcile::desktops();
        let open = signal(false);
        let edit = Edit::new(MOVE);
        let _registered = telar::register_transaction(open, edit.transaction());

        open.set(true);
        edit.preview(vec![move_clock("start")])
            .expect("it previews");
        assert_eq!(clock_zone().as_deref(), Some("start"));
        assert!(telar::dismiss_top(), "Esc closes the popover");
        assert!(Rc::ptr_eq(&reconcile::desktops(), &before));
        assert_eq!(rig.undo_label(), None);

        open.set(true);
        edit.preview(vec![move_clock("end")]).expect("it previews");
        open.set(false);
        assert_eq!(rig.undo_label().as_deref(), Some(MOVE));
        assert_eq!(clock_zone().as_deref(), Some("end"));
    }

    /// No savepoints: while one edit is open a second is refused rather than nested, and the first is untouched by the attempt.
    #[test]
    fn a_second_edit_is_refused_while_one_is_open() {
        let _rig = rig("session-nested");
        let first = session::begin(MOVE).expect("it opens");
        first.preview(vec![move_clock("end")]).expect("it previews");
        assert_eq!(
            session::begin("Anything else").err(),
            Some(EditError::Nested)
        );
        assert_eq!(clock_zone().as_deref(), Some("end"));
        first.revert().expect("it reverts");
        assert!(session::begin("Anything else").is_ok());
    }

    /// Undo with an edit still open takes back that edit, which was never recorded, and leaves the history alone.
    #[test]
    fn undo_while_an_edit_is_open_reverts_that_edit_first() {
        let rig = rig("session-undo-open");
        let committed = session::begin("Move the clock to the start").expect("it opens");
        committed
            .preview(vec![move_clock("start")])
            .expect("it previews");
        committed.commit().expect("it commits");
        let after_commit = reconcile::desktops();

        let open = session::begin(MOVE).expect("it opens");
        let moved = Spot {
            group: GroupId::new("start"),
            ..zone("center")
        };
        open.preview(vec![LayoutOp::MoveInstance {
            from: moved,
            to: zone("end"),
            id: InstanceId::new("clock"),
            index: 0,
        }])
        .expect("it previews");
        assert_eq!(clock_zone().as_deref(), Some("end"));

        assert_eq!(session::undo().as_deref(), Ok(MOVE));
        assert!(!open.is_open());
        assert!(Rc::ptr_eq(&reconcile::desktops(), &after_commit));
        assert_eq!(
            rig.undo_label().as_deref(),
            Some("Move the clock to the start")
        );
    }

    fn compositor() -> Compositor {
        Compositor {
            restack: true,
            locked: false,
            lockable: Err("no lock here".to_string()),
        }
    }

    fn on_the_bar() -> Node {
        Node::area(Some(SCREEN), LayerKind::Top, &AreaId::new("bar-top"))
    }

    /// The selection names what it selected through an edit that moves it, clears when the edit takes it away, and belongs to the mode it was made in.
    #[test]
    fn the_selection_follows_a_move_and_clears_when_its_instance_is_removed() {
        let _rig = rig("session-selection");
        let clock = InstanceId::new("clock");
        assert!(
            !session::select(Selection::Area(on_the_bar())),
            "nothing is selected while no mode is up"
        );
        mode::enter_as(LayerKind::Top, Some(SCREEN), &compositor()).expect("top mode");
        assert!(
            !session::select(Selection::Area(Node::area(
                Some(SCREEN),
                LayerKind::Background,
                &AreaId::new("background"),
            ))),
            "only what is on the layer being edited"
        );
        assert!(session::select(Selection::Instance(
            on_the_bar().instance(&GroupId::new("center"), &clock)
        )));

        let moved = session::begin(MOVE).expect("it opens");
        moved.preview(vec![move_clock("end")]).expect("it previews");
        moved.commit().expect("it commits");
        assert_eq!(
            session::selected(),
            Selection::Instance(on_the_bar().instance(&GroupId::new("end"), &clock))
        );

        let removed = session::begin("Remove the clock").expect("it opens");
        removed
            .preview(vec![LayoutOp::DeleteInstance {
                spot: zone("end"),
                id: clock.clone(),
            }])
            .expect("it previews");
        assert_ne!(
            session::selected(),
            Selection::None,
            "a preview may yet be reverted"
        );
        removed.commit().expect("it commits");
        assert_eq!(session::selected(), Selection::None);

        assert!(session::select(Selection::Area(on_the_bar())));
        mode::switch(LayerKind::Overlay);
        assert_eq!(
            session::selected(),
            Selection::None,
            "a mode change clears it"
        );
        assert!(session::select(Selection::Area(Node::area(
            Some(SCREEN),
            LayerKind::Overlay,
            &AreaId::new("stack"),
        ))));
        mode::leave();
        assert_eq!(session::selected(), Selection::None, "so does leaving");
    }

    static REMEMBERED: ui::host::InstanceStore<u32> = ui::host::InstanceStore::new(|| 0);

    /// F-3.4 from the editor's side: a committed removal, and the redo that repeats it, forget the instance in every store kept by id.
    #[test]
    fn a_committed_removal_forgets_the_instance() {
        let _rig = rig("session-forget");
        let clock = ui::host::InstanceId::new("clock");
        REMEMBERED.set(&clock, 12);
        let removed = session::begin("Remove the clock").expect("it opens");
        removed
            .preview(vec![LayoutOp::DeleteInstance {
                spot: zone("center"),
                id: InstanceId::new("clock"),
            }])
            .expect("it previews");
        assert_eq!(REMEMBERED.get(&clock), 12, "a preview forgets nothing");
        removed.commit().expect("it commits");
        assert_eq!(REMEMBERED.get(&clock), 0);

        session::undo().expect("it undoes");
        REMEMBERED.set(&clock, 5);
        session::redo().expect("it redoes");
        assert_eq!(REMEMBERED.get(&clock), 0);
    }

    /// F-10.35: under `--safe-layout` no edit opens, none previews and the history does not move.
    #[test]
    fn the_safe_layout_refuses_every_edit_and_the_history() {
        let _rig = rig("session-safe");
        let before = reconcile::desktops();
        let dragged = Edit::new(MOVE);
        dragged.begin().expect("an edit opens on an ordinary store");
        dragged.revert().expect("and reverts");
        surfaces::layouts::install(
            Rc::new(RefCell::new(LayoutStore::safe(
                util::paths::isolated_root()
                    .expect("a scratch root")
                    .join("editor-session-safe-store"),
            ))),
            Rc::new(|| {}),
        );

        assert_eq!(session::begin(MOVE).err(), Some(EditError::Safe));
        assert!(
            dragged.transaction().begin().is_ok(),
            "a drag opens it itself"
        );
        assert_eq!(
            dragged.preview(vec![move_clock("end")]),
            Err(EditError::Safe)
        );
        assert!(Rc::ptr_eq(&reconcile::desktops(), &before));
        dragged.revert().expect("it reverts");
        for walked in [session::undo(), session::redo()] {
            let refused = walked.expect_err("refused");
            assert!(refused.contains("--safe-layout"), "{refused}");
        }
    }

    #[test]
    fn ctrl_z_undoes_and_ctrl_shift_z_and_ctrl_y_redo() {
        let with = |ctrl: bool, shift: bool| ModifiersState {
            is_ctrl: ctrl,
            is_shift: shift,
            ..ModifiersState::default()
        };
        let key = |ch: char| Key::Char(ch);
        let cases = [
            (key('z'), with(true, false), Some(Way::Undo)),
            (key('Z'), with(true, true), Some(Way::Redo)),
            (key('z'), with(true, true), Some(Way::Redo)),
            (key('y'), with(true, false), Some(Way::Redo)),
            (key('z'), with(false, false), None),
            (key('y'), with(true, true), None),
            (
                key('z'),
                ModifiersState {
                    is_alt: true,
                    ..with(true, false)
                },
                None,
            ),
        ];
        for (key, modifiers, expected) in cases {
            assert_eq!(
                session::history_key(&key, modifiers),
                expected,
                "{key:?} {modifiers:?}"
            );
        }
    }
}
