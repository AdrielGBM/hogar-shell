//! An area whose `visible` reads false while its layer's edit mode is up: drawn dim, selected by a press, with the selection saying why; outside the mode it paints nothing and is not there to select.

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use telar::{
        AvailableSpace, ComponentList, Container, DrawCommand, Event, LayoutItem, LayoutStyle,
        PointerButton, PointerSource, Rect, compute_layout,
    };

    use layout::{AreaId, Expr, LayerKind, Layout};
    use surfaces::area::Surround;
    use surfaces::expressions;
    use surfaces::menu::Pointed;
    use surfaces::reconcile;
    use surfaces::rects::{self, Node};
    use surfaces::transient;

    use crate::mode::{self, Compositor};
    use crate::rig::{SCREEN, rig_with};
    use crate::select;
    use crate::session;

    const SIZE: (f32, f32) = (1920.0, 1080.0);

    fn bar() -> Node {
        Node::area(Some(SCREEN), LayerKind::Top, &AreaId::new("bar-top"))
    }

    fn hide_the_bar(layout: &mut Layout) {
        layout.outputs[0]
            .layers
            .top
            .areas
            .iter_mut()
            .find(|area| area.id.as_str() == "bar-top")
            .expect("the bar")
            .visible = Some(Expr("false".to_string()));
    }

    struct Scope(telar::OwnerGuard);

    impl Drop for Scope {
        fn drop(&mut self) {
            mode::leave();
            transient::close_all();
            telar::dispose_owner(self.0.id());
        }
    }

    fn enter() {
        mode::enter_as(
            LayerKind::Top,
            Some(SCREEN),
            &Compositor {
                restack: true,
                locked: false,
                lockable: Ok(()),
            },
        )
        .expect("the top layer's mode opens");
    }

    fn lay_out(root: telar::NodeId) {
        compute_layout(
            root,
            AvailableSpace::Definite(SIZE.0),
            AvailableSpace::Definite(SIZE.1),
        )
        .expect("the screen lays out");
    }

    fn painted(tree: &ComponentList) -> bool {
        tree.commands().iter().any(|command| {
            matches!(command, DrawCommand::Rect { rect, .. } if rect.width > 0.0 && rect.height > 0.0)
        })
    }

    fn press(tree: &mut ComponentList, (x, y): (f32, f32)) {
        let (x, y) = (f64::from(x), f64::from(y));
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
    }

    #[test]
    fn a_hidden_area_is_drawn_dim_and_selected_by_pointer_in_its_mode_and_is_nothing_outside_it() {
        let _rig = rig_with("hidden-area", hide_the_bar);
        let _scope = Scope(telar::owner_scope());
        ui::descriptor::install(&[]);

        let desktop = reconcile::desktops()[0].clone();
        let config = Arc::clone(&desktop.config);
        let area = desktop
            .resolved
            .layer(LayerKind::Top)
            .expect("a top layer")
            .areas
            .iter()
            .find(|area| area.id.as_str() == "bar-top")
            .expect("the bar")
            .clone();
        let built = surfaces::area::build(
            &area,
            Surround {
                config: &config,
                theme: config.resolve_theme(),
                output: Some(SCREEN),
                layer: LayerKind::Top,
                bounds: Rect::new(0.0, 0.0, SIZE.0, SIZE.1),
                reserved: desktop.reserved,
                audience: ui::host::Audience::Owner,
            },
        )
        .expect("a bar has a builder")
        .expect("the bar builds");
        let drawn = Container::new(LayoutStyle::new().width(SIZE.0).height(SIZE.1), vec![built])
            .expect("a page");
        let drawn_root = drawn.layout_node();
        let drawn_tree = ComponentList::new(drawn);

        enter();
        let mode = mode::current().expect("the mode is up");
        let tool = select::tool(&mode).expect("the selection builds");
        let tools = Container::new(
            LayoutStyle::new().width(SIZE.0).height(SIZE.1),
            vec![Box::new(Pointed::new(tool)) as Box<dyn LayoutItem>],
        )
        .expect("a page");
        let tools_root = tools.layout_node();
        let mut tools_tree = ComponentList::new(tools);
        for _ in 0..2 {
            lay_out(drawn_root);
            lay_out(tools_root);
        }

        let at = rects::rect(&bar()).expect("the bar is placed");
        assert!(at.width > 0.0 && at.height > 0.0, "{at:?}");
        assert!(painted(&drawn_tree), "drawn in its mode");
        assert!(
            drawn_tree.commands().iter().any(|command| matches!(
                command,
                DrawCommand::PushLayer { opacity, .. } if *opacity == expressions::HIDDEN_OPACITY
            )),
            "at 30 %"
        );

        press(&mut tools_tree, (at.x + 2.0, at.y + at.height / 2.0));
        let selected = session::selected();
        let node = selected.node().expect("the press selected something");
        assert!(node.is_in(Some(SCREEN), LayerKind::Top, &AreaId::new("bar-top")));
        assert_eq!(
            expressions::hidden_by(node).as_deref(),
            Some("false"),
            "the selection can say why it is dim"
        );
        assert_eq!(
            telar::t!("editor.select.hidden", expr = "false"),
            "Hidden: visible = false is false"
        );

        mode::leave();
        drop(tools_tree);
        lay_out(drawn_root);
        assert!(!painted(&drawn_tree), "outside its mode it paints nothing");
        assert!(
            rects::rect(&bar()).is_none_or(|rect| rect.width == 0.0 || rect.height == 0.0),
            "and is nowhere to select"
        );
    }
}
