#[cfg(test)]
mod tests {
    use telar::{Key, NamedKey};

    use layout::{AreaId, GroupId, GroupKind, InstanceId, LayerKind, Layout};
    use surfaces::rects::Node;
    use surfaces::transient;
    use ui::descriptor::ModuleDescriptor;
    use ui::host::WidgetSize;

    use crate::command_palette::{self, Entry};
    use crate::keys;
    use crate::mode;
    use crate::rig::{
        CTRL, NONE, Owner, Page, Rig, SCREEN, cell_group, enter, module, rig_with, stored, tap,
        undoes_to, widget,
    };
    use crate::session::{self, Selection};

    static PROBES: &[ModuleDescriptor] = &[
        module("clock", "Clock", &WidgetSize::ALL),
        module("weather", "Weather", &WidgetSize::ALL),
    ];

    fn owner() -> Owner {
        Owner::installing(PROBES)
    }

    /// The desktop grid alone, holding one loose weather widget.
    fn one_widget(layout: &mut Layout) {
        let areas = &mut layout.outputs[0].layers.desktop.areas;
        areas.retain(|area| area.id.as_str() == "widgets");
        areas[0].groups = vec![layout::Group {
            children: vec![widget("weather", "weather")],
            ..cell_group("weather", (6, 5, 2, 2))
        }];
    }

    fn weather() -> Node {
        Node::area(Some(SCREEN), LayerKind::Desktop, &AreaId::new("widgets"))
            .instance(&GroupId::new("weather"), &InstanceId::new("weather"))
    }

    fn weather_column(rig: &Rig) -> u32 {
        let grid = &stored(rig).outputs[0].layers.desktop.areas[0];
        let group = grid
            .groups
            .iter()
            .find(|group| group.id.as_str() == "weather")
            .expect("the weather group");
        match group.kind {
            Some(GroupKind::Cell { col, .. }) => col,
            ref other => panic!("not on cells: {other:?}"),
        }
    }

    fn named<'a>(commands: &'a [Entry], name: &str) -> Vec<&'a Entry> {
        commands
            .iter()
            .filter(|command| command.name == name)
            .collect()
    }

    fn opened(layer: LayerKind) -> Page {
        assert!(tap(Key::Char('k'), CTRL), "Ctrl+K answers");
        assert!(transient::is_open(command_palette::ID));
        Page::of(command_palette::tree(SCREEN, layer))
    }

    fn held_or_own(label: &str) -> bool {
        [
            telar::t!("editor.keys.op.peek"),
            telar::t!("editor.keys.op.command-palette"),
        ]
        .contains(&label.to_string())
    }

    fn typed(page: &mut Page, text: &str) {
        for ch in text.chars() {
            page.key(Key::Char(ch));
        }
    }

    /// Every line of every mode's key list is an entry of the palette, and typing its label brings it to the top.
    #[test]
    fn every_line_of_the_key_list_is_an_entry() {
        let _rig = rig_with("command-every-key", |_| {});
        let _owner = owner();
        for layer in LayerKind::ALL {
            let _host = enter(layer);
            let commands = command_palette::commands(layer);
            let lines = keys::help_rows(layer);
            assert!(!lines.is_empty());
            for line in lines {
                if held_or_own(&line.what) {
                    continue;
                }
                assert!(
                    !named(&commands, &line.what).is_empty(),
                    "{layer}: {:?} is not an entry",
                    line.what
                );
                let first = command_palette::found(&commands, &line.what)
                    .into_iter()
                    .next()
                    .map(|command| command.name);
                assert_eq!(first.as_deref(), Some(line.what.as_str()), "{layer}");
            }
            mode::leave();
        }
    }

    /// The held Peek key and the key that opens the palette are not entries of it, in any mode.
    #[test]
    fn the_held_and_its_own_keys_are_not_entries() {
        let _rig = rig_with("command-not-offered", |_| {});
        let _owner = owner();
        for layer in LayerKind::ALL {
            let _host = enter(layer);
            let commands = command_palette::commands(layer);
            for label in [
                telar::t!("editor.keys.op.peek"),
                telar::t!("editor.keys.op.command-palette"),
            ] {
                assert!(named(&commands, &label).is_empty(), "{layer}: {label}");
            }
            mode::leave();
        }
    }

    /// Its chords are its own in every mode, with the vim keys on or off: under vim Ctrl+K stays what makes the selection taller.
    #[test]
    fn its_chords_take_no_other_rows_key() {
        let _owner = owner();
        let _rig = rig_with("command-unshadowed", |_| {});
        for vim in [false, true] {
            for layer in LayerKind::ALL {
                let table = keys::table_for(layer, vim);
                let answering = |chord: &keys::Chord| -> Vec<&'static str> {
                    table
                        .iter()
                        .filter(|row| {
                            row.keys
                                .iter()
                                .any(|held| held.matches(&chord.key, chord.modifiers))
                        })
                        .map(|row| row.name)
                        .collect()
                };
                for chord in keys::command_palette_chords(vim) {
                    assert_eq!(
                        answering(&chord),
                        vec!["command-palette"],
                        "{layer} (vim {vim}): {}",
                        chord.spelled()
                    );
                }
                let ctrl_k = keys::Chord::char('k').ctrl();
                let expected = match vim {
                    true => vec!["resize"],
                    false => vec!["command-palette"],
                };
                assert_eq!(answering(&ctrl_k), expected, "{layer} (vim {vim})");
            }
        }
    }

    /// Ctrl+K opens it over whatever was selected; while it is open the mode's keys are its, and Esc closes it alone, handing the keyboard back with the selection kept. The palette's window, which a headless shell never builds, is what puts it on the dismiss stack, so the test registers it as the window would.
    #[test]
    fn esc_closes_it_and_hands_the_keyboard_back() {
        let _rig = rig_with("command-esc", one_widget);
        let _owner = owner();
        let _host = enter(LayerKind::Desktop);
        assert!(session::select(Selection::of(weather())));
        let _page = opened(LayerKind::Desktop);
        let _window = telar::DismissRegistration::new(std::rc::Rc::new(|| {
            transient::close(command_palette::ID)
        }));
        tap(Key::Char('?'), NONE);
        assert!(!keys::help().peek(), "the palette has the keyboard");

        assert!(tap(Key::Named(NamedKey::Escape), NONE));
        assert!(!transient::is_open(command_palette::ID));
        assert!(mode::current().is_some());
        assert_eq!(session::selected(), Selection::of(weather()));
        assert!(tap(Key::Char('?'), NONE));
        assert!(keys::help().peek(), "the mode's keys answer again");
    }

    /// Typed and chosen with Enter, an entry runs against the selection, closing the palette, as one entry in the history.
    #[test]
    fn enter_runs_the_entry_against_the_selection_as_one_undo() {
        let rig = rig_with("command-enter", one_widget);
        let _owner = owner();
        let _host = enter(LayerKind::Desktop);
        assert!(session::select(Selection::of(weather())));
        let before = stored(&rig);
        let mut page = opened(LayerKind::Desktop);
        typed(&mut page, "dupl");
        page.key(Key::Named(NamedKey::Enter));
        assert!(
            !transient::is_open(command_palette::ID),
            "running closes it"
        );
        assert_eq!(rig.undo_label().as_deref(), Some("Duplicate Weather"));
        assert_eq!(
            stored(&rig).outputs[0].layers.desktop.areas[0].groups.len(),
            2
        );
        undoes_to(&rig, &before);
    }

    /// A move chosen by its chord goes one step that way and is committed at once, one undo taking it back.
    #[test]
    fn a_step_chosen_by_its_chord_is_one_undo() {
        let rig = rig_with("command-step", one_widget);
        let _owner = owner();
        let _host = enter(LayerKind::Desktop);
        assert!(session::select(Selection::of(weather())));
        let before = stored(&rig);
        let mut page = opened(LayerKind::Desktop);
        typed(&mut page, "Shift+←");
        page.key(Key::Named(NamedKey::Enter));
        assert_eq!(weather_column(&rig), 5);
        assert!(session::open().is_none(), "nothing is left previewing");
        undoes_to(&rig, &before);
    }

    /// The arrows walk the entries: Down as many times as the entry is below the first, then Enter, runs that one.
    #[test]
    fn the_arrows_walk_the_entries() {
        let rig = rig_with("command-arrows", one_widget);
        let _owner = owner();
        let _host = enter(LayerKind::Desktop);
        assert!(session::select(Selection::of(weather())));
        let found =
            command_palette::found(&command_palette::commands(LayerKind::Desktop), "shift+");
        let below = found
            .iter()
            .position(|command| command.hint == "Shift+→")
            .expect("moving right is found");
        assert!(below > 0);
        let mut page = opened(LayerKind::Desktop);
        typed(&mut page, "shift+");
        page.key(Key::Named(NamedKey::ArrowUp));
        for _ in 0..below {
            page.key(Key::Named(NamedKey::ArrowDown));
        }
        page.key(Key::Named(NamedKey::Enter));
        assert_eq!(weather_column(&rig), 7);
    }

    /// An entry the selection refuses is listed dimmed with the reason and does nothing when pressed; one it takes runs when pressed.
    #[test]
    fn a_press_runs_an_entry_and_a_refused_one_stays_put() {
        let rig = rig_with("command-press", one_widget);
        let _owner = owner();
        let _host = enter(LayerKind::Desktop);
        session::clear_selection();
        let commands = command_palette::commands(LayerKind::Desktop);
        let remove = named(&commands, "Remove it");
        assert_eq!(
            remove
                .first()
                .and_then(|command| command.refused.as_deref()),
            Some("There is nothing here to customize")
        );
        let stack = named(&commands, "Stack with the widget that way");
        assert_eq!(stack.len(), 4, "one entry each way");
        assert!(
            stack
                .iter()
                .all(|command| command.refused.as_deref() == Some("Only with a grid selected"))
        );
        let before = stored(&rig);
        let mut page = opened(LayerKind::Desktop);
        typed(&mut page, "remove");
        page.click(page.at("Remove it"));
        assert!(
            transient::is_open(command_palette::ID),
            "a refused entry stays put"
        );
        assert_eq!(stored(&rig), before);

        for _ in "remove".chars() {
            page.key(Key::Named(NamedKey::Backspace));
        }
        typed(&mut page, "add clock");
        page.click(page.at("Add Clock"));
        assert!(!transient::is_open(command_palette::ID));
        assert_eq!(rig.undo_label().as_deref(), Some("Add Clock"));
        undoes_to(&rig, &before);
    }

    /// Beside the keys it lists the strip's actions, the history, the other modes and Done, which leaves the mode.
    #[test]
    fn the_strip_the_history_and_the_other_modes_are_entries() {
        let _rig = rig_with("command-strip", one_widget);
        let _owner = owner();
        let _host = enter(LayerKind::Desktop);
        assert!(session::select(Selection::of(weather())));
        crate::duplicate::duplicate(&session::selected()).expect("a copy");
        let commands = command_palette::commands(LayerKind::Desktop);
        for name in [
            "Theme…",
            "History: At the start",
            "Edit Top",
            "Edit Lock screen",
            "Done",
        ] {
            assert!(!named(&commands, name).is_empty(), "{name:?}");
        }
        assert!(named(&commands, "Edit Desktop").is_empty());

        let mut page = opened(LayerKind::Desktop);
        typed(&mut page, "Done");
        page.key(Key::Named(NamedKey::Enter));
        assert!(mode::current().is_none());
    }
}
