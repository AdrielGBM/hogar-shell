//! The Id row of the area, group and instance popovers, typed into and pressed as a person would.

#[cfg(test)]
mod tests {
    use telar::{Key, ModifiersState};

    use layout::{Action, Area, AreaId, AreaKind, GroupId, InstanceId, LayerKind, Layout, Trigger};
    use surfaces::rects::Node;

    use crate::popover;
    use crate::rig::{Card, Rig, Scope, bar, rig_with, stored};
    use crate::session::{self, Selection};

    fn rename(card: &mut Card, shown: &str, typed: &str) {
        card.click_on(shown);
        let ctrl = ModifiersState {
            is_ctrl: true,
            ..ModifiersState::default()
        };
        card.key(Key::Char('a'), ctrl);
        for ch in typed.chars() {
            card.key(Key::Char(ch), ModifiersState::default());
        }
        card.click_on("Rename");
    }

    fn clock() -> Node {
        bar().instance(&GroupId::new("center"), &InstanceId::new("clock"))
    }

    fn with_clock_panel(mine: &mut Layout) {
        mine.outputs[0].layers.top.areas.push(Area {
            id: AreaId::new("panel-clock"),
            kind: Some(AreaKind::Panel {
                owner: Some(InstanceId::new("clock")),
                along: None,
                cols: None,
                rows: None,
                cell: None,
                gap: None,
            }),
            ..Area::default()
        });
    }

    fn bar_of(rig: &Rig) -> Area {
        stored(rig).outputs[0].layers.top.areas[0].clone()
    }

    #[test]
    fn an_instances_id_row_renames_it_and_the_panel_it_owns_as_one_undo_entry() {
        let rig = rig_with("rename-instance", with_clock_panel);
        let _scope = Scope::new();
        let _mode = crate::rig::enter(LayerKind::Top);
        let before = stored(&rig);
        popover::open_instance(clock()).expect("the clock's popover opens");
        let mut card = Card::open();

        rename(&mut card, "clock", "time");

        let center = &bar_of(&rig).groups[1];
        assert_eq!(center.children[0].id.as_str(), "time");
        let panel = stored(&rig).outputs[0].layers.top.areas[1].clone();
        assert_eq!(
            panel.kind.as_ref().and_then(AreaKind::owner),
            Some(&InstanceId::new("time")),
            "the panel opens from the renamed clock"
        );
        assert_eq!(rig.undo_label().as_deref(), Some("Rename clock to time"));
        assert_eq!(popover::current(), None, "the popover it renamed is closed");
        assert_eq!(
            session::selected(),
            Selection::Instance(bar().instance(&GroupId::new("center"), &InstanceId::new("time")))
        );
        crate::rig::undoes_to(&rig, &before);
    }

    #[test]
    fn an_action_line_naming_the_id_is_shown_under_the_row_and_nothing_changes() {
        let rig = rig_with("rename-refused", |mine| {
            mine.outputs[0].layers.top.areas[0].groups[2].children[0]
                .actions
                .insert(Trigger::Press, Action(vec!["panel toggle clock".into()]));
        });
        let _scope = Scope::new();
        let before = stored(&rig);
        popover::open_instance(clock()).expect("the clock's popover opens");
        let mut card = Card::open();

        rename(&mut card, "clock", "time");

        assert!(
            card.shows_part_of("`panel toggle clock`"),
            "the refusal names the line: {:?}",
            card.texts()
        );
        assert_eq!(stored(&rig), before);
        assert_eq!(rig.undo_label(), None);
        assert_eq!(popover::current(), Some(clock()), "the popover stays open");
    }

    /// A line typed into the same popover is weighed as one already written: the popover keeps it, so the rename would leave it naming nothing.
    #[test]
    fn an_action_line_typed_in_the_same_popover_that_names_the_id_refuses_the_rename() {
        let rig = rig_with("rename-refused-typed", |_| {});
        let _scope = Scope::new();
        services::command::set_runner(|_| "ok".to_string(), |_| true);
        let before = stored(&rig);
        popover::open_instance(clock()).expect("the clock's popover opens");
        let mut card = Card::open();
        let actions = popover::instance_draft()
            .expect("an instance's popover")
            .actions();
        let trigger = actions.add().expect("a free gesture");
        actions
            .set(trigger, "panel toggle clock")
            .expect("an ordinary line");
        card.lay_out();

        rename(&mut card, "clock", "time");

        assert!(
            card.shows_part_of("`panel toggle clock`"),
            "the refusal names the typed line: {:?}",
            card.texts()
        );
        assert_eq!(stored(&rig), before, "nothing is kept yet");
        assert_eq!(rig.undo_label(), None);
        assert_eq!(popover::current(), Some(clock()), "the popover stays open");
    }

    #[test]
    fn an_areas_and_a_groups_id_rows_rename_them() {
        let rig = rig_with("rename-area-group", |_| {});
        let _scope = Scope::new();
        popover::open_area(bar()).expect("the bar's popover opens");
        let mut card = Card::open();
        rename(&mut card, "bar-top", "bar-main");
        assert_eq!(bar_of(&rig).id.as_str(), "bar-main");
        assert_eq!(
            rig.undo_label().as_deref(),
            Some("Rename bar-top to bar-main")
        );

        let main = Node::area(
            Some(crate::rig::SCREEN),
            LayerKind::Top,
            &AreaId::new("bar-main"),
        );
        popover::open_group(main.group(&GroupId::new("end"))).expect("the group's popover opens");
        let mut card = Card::open();
        rename(&mut card, "end", "tail");
        assert_eq!(bar_of(&rig).groups[2].id.as_str(), "tail");
        assert_eq!(rig.undo_label().as_deref(), Some("Rename end to tail"));
    }

    #[test]
    fn an_id_that_cannot_be_typed_back_is_refused_under_the_row() {
        let rig = rig_with("rename-unreadable", |_| {});
        let _scope = Scope::new();
        let before = stored(&rig);
        popover::open_group(bar().group(&GroupId::new("end"))).expect("the group's popover opens");
        let mut card = Card::open();
        rename(&mut card, "end", "the end");
        assert!(
            card.shows_part_of("separator"),
            "the refusal says why: {:?}",
            card.texts()
        );
        rename(&mut card, "the end", "start");
        assert!(
            card.shows_part_of("already names a group `start`"),
            "{:?}",
            card.texts()
        );
        assert_eq!(stored(&rig), before);
    }

    /// A change made in the popover before the rename is kept as an entry of its own first, since its rows address the item by the id it is losing; the rename is the second entry.
    #[test]
    fn a_change_made_before_the_rename_is_kept_as_its_own_entry() {
        let rig = rig_with("rename-after-edit", |_| {});
        let _scope = Scope::new();
        let before = stored(&rig);
        popover::open_area(bar()).expect("the bar's popover opens");
        let mut card = Card::open();
        popover::shared::<String>("style.fill")
            .expect("the fill row")
            .set("overlay".to_string());

        rename(&mut card, "bar-top", "bar-main");

        let renamed = bar_of(&rig);
        assert_eq!(renamed.id.as_str(), "bar-main");
        assert_eq!(renamed.style.fill.as_deref(), Some("overlay"));
        assert_eq!(
            rig.undo_label().as_deref(),
            Some("Rename bar-top to bar-main")
        );
        session::undo().expect("the rename is one entry");
        assert_eq!(bar_of(&rig).id.as_str(), "bar-top");
        assert_eq!(bar_of(&rig).style.fill.as_deref(), Some("overlay"));
        session::undo().expect("the edit before it is another");
        assert_eq!(stored(&rig), before);
    }
}
