//! The edit mode's host as its window builds it: each mode's tools under its strip, a refused lock mode saying why, and a press on an area reaching its selection target through every tool.

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use telar::{
        AvailableSpace, ComponentList, Container, DrawCommand, LayoutStyle, Text, box_item,
        compute_layout, new_container, use_theme,
    };

    use config::Config;
    use config::theme::{FontRole, NordTheme};
    use layout::LayerKind;
    use surfaces::rects;
    use ui::descriptor::Built;

    use crate::host::{Under, add_tool, tools, tree};
    use crate::mode::Mode;

    const SCREEN: (f32, f32) = (1920.0, 1080.0);

    fn marker(said: &'static str) -> Built {
        let theme = use_theme::<NordTheme>();
        Ok(box_item(Text::declaring(
            move || said.to_string(),
            LayoutStyle::new(),
            move |inherited| theme.text_over(inherited, FontRole::Body, theme.text),
        )?))
    }

    fn desktop_tool(_: &Mode) -> Built {
        marker("a desktop tool")
    }

    fn top_tool(_: &Mode) -> Built {
        marker("a top tool")
    }

    fn lock_tool(_: &Mode) -> Built {
        marker("a lock tool")
    }

    /// Everything the host's tree says, laid out over a whole screen.
    fn said(mode: &Mode, under: Under) -> Vec<String> {
        telar::reset_layout_runtime();
        telar::set_locale("en");
        telar::set_theme(Arc::new(Config::default()).resolve_theme());
        let item = tree(mode, under).expect("the host builds");
        let page = || LayoutStyle::new().width(SCREEN.0).height(SCREEN.1);
        let root = new_container(page(), &[item.layout_node()]).expect("a root");
        let tree = ComponentList::new(Container::new(page(), vec![item]).expect("a page"));
        compute_layout(
            root,
            AvailableSpace::Definite(SCREEN.0),
            AvailableSpace::Definite(SCREEN.1),
        )
        .expect("the host lays out");
        tree.commands()
            .iter()
            .filter_map(|command| match command {
                DrawCommand::Text { text, .. } => Some(text.to_string()),
                _ => None,
            })
            .collect()
    }

    /// A mode's tools mount in its host and only there: the strip names the mode, the screen and the way out, and a tool added for another layer is nowhere to be seen.
    #[test]
    fn a_mode_mounts_its_own_tools_under_its_strip() {
        add_tool(LayerKind::Desktop, desktop_tool);
        add_tool(LayerKind::Top, top_tool);
        let mode = Mode {
            layer: LayerKind::Desktop,
            output: "DP-1".to_string(),
            refused: None,
        };
        let said = said(&mode, Under::Nothing);
        for expected in ["Desktop ▾", "DP-1", "Done", "a desktop tool"] {
            assert!(
                said.iter().any(|text| text == expected),
                "{expected}: {said:?}"
            );
        }
        assert!(!said.iter().any(|text| text == "a top tool"), "{said:?}");
    }

    /// TA-8: lock mode on a machine that cannot lock opens, and says why where its tools would be, instead of offering tools for a lock screen it could never show.
    #[test]
    fn a_refused_lock_mode_says_why_instead_of_mounting_tools() {
        add_tool(LayerKind::Lock, lock_tool);
        let mode = Mode {
            layer: LayerKind::Lock,
            output: "DP-1".to_string(),
            refused: Some(util::message!(
                "editor.refused.cannot_lock",
                reason = "no PAM"
            )),
        };
        let said = said(&mode, Under::Nothing);
        assert!(said.iter().any(|text| text.contains("no PAM")), "{said:?}");
        assert!(!said.iter().any(|text| text == "a lock tool"), "{said:?}");
        assert!(
            said.iter().any(|text| text == "Done"),
            "the way out is still there"
        );
    }

    /// In every mode a press on an area selects it, through every layer the mode's tools lay over the screen: a tool takes the pointer only where it has a control, so none of them stands between the pointer and the selection targets under it.
    #[test]
    fn a_press_on_an_area_selects_it_in_every_mode() {
        use telar::{Event, PointerButton, PointerSource, Rect, signal};

        use layout::AreaId;

        let _rig = crate::rig::rig_with("host-press-selects", |_| {});
        let scope = telar::owner_scope();
        let areas = [
            (
                LayerKind::Background,
                "background",
                Rect::new(0.0, 0.0, 1920.0, 1080.0),
                (300.0, 700.0),
            ),
            (
                LayerKind::Desktop,
                "widgets",
                Rect::new(480.0, 270.0, 960.0, 540.0),
                (600.0, 700.0),
            ),
            (
                LayerKind::Top,
                "bar-top",
                Rect::new(0.0, 0.0, 1920.0, 34.0),
                (300.0, 17.0),
            ),
            (
                LayerKind::Overlay,
                "stack",
                Rect::new(1500.0, 50.0, 380.0, 300.0),
                (1600.0, 300.0),
            ),
        ];
        for (layer, id, rect, _) in areas {
            let node = rects::Node::area(Some(crate::rig::SCREEN), layer, &AreaId::new(id));
            rects::track_spanning(node, vec![signal(rect)]);
        }
        for (layer, id, _, at) in areas {
            crate::rig::open_mode(layer);
            let mode = crate::mode::current().expect("the mode is up");
            let page = || LayoutStyle::new().width(SCREEN.0).height(SCREEN.1);
            let built = tools(&mode).expect("the tools build");
            let root = new_container(page(), &[built.layout_node()]).expect("a root");
            let mut tree = ComponentList::new(surfaces::menu::Pointed::new(Box::new(
                Container::new(page(), vec![built]).expect("a page"),
            )));
            compute_layout(
                root,
                AvailableSpace::Definite(SCREEN.0),
                AvailableSpace::Definite(SCREEN.1),
            )
            .expect("the tools lay out");
            let (x, y) = at;
            for event in [
                Event::PointerMoved {
                    x,
                    y,
                    source: PointerSource::Mouse,
                },
                Event::PointerPressed {
                    x,
                    y,
                    button: PointerButton::Primary,
                    source: PointerSource::Mouse,
                },
                Event::PointerReleased {
                    x,
                    y,
                    button: PointerButton::Primary,
                    source: PointerSource::Mouse,
                },
            ] {
                tree.on_event(&event);
            }
            assert_eq!(
                crate::session::selected()
                    .node()
                    .map(|node| node.area.to_string()),
                Some(id.to_string()),
                "{layer}: a press on the area never reached its selection target"
            );
            crate::mode::leave();
        }
        telar::dispose_owner(scope.id());
    }
}
