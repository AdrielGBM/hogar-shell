#[cfg(test)]
mod tests {
    use telar::{Key, ModifiersState};

    use layout::{
        Area, AreaId, AreaKind, Arrange, Group, GroupId, GroupKind, Instance, InstanceId,
        LayerKind, Layout, LayoutId, OutputMatch, OutputRule, Rect, Representation,
    };
    use surfaces::menu::Asked;
    use surfaces::rects::Node;
    use surfaces::transient;
    use ui::descriptor::{Category, ChipDef, Input, ModuleDescriptor, Representations, WidgetDef};
    use ui::host::WidgetSize;

    use crate::rig::{Rig, SCREEN, enter, face, rig_with, stored, tap};
    use crate::session::{self, Selection};
    use crate::{context, mode};

    static PROBES: &[ModuleDescriptor] = &[ModuleDescriptor {
        id: "clock",
        name: "Clock",
        icon: "clock",
        category: Category::Time,
        options: &[],
        representations: Representations {
            chip: Some(ChipDef::new(face, Input::ReadOnly)),
            widget: Some(WidgetDef {
                sizes: &WidgetSize::ALL,
                build: face,
                input: Input::ReadOnly,
            }),
            ..Representations::NONE
        },
        actions: &[],
        sources: &[],
    }];

    struct Owner(telar::OwnerGuard);

    impl Owner {
        fn new() -> Self {
            ui::descriptor::install(PROBES);
            Self(telar::owner_scope())
        }
    }

    impl Drop for Owner {
        fn drop(&mut self) {
            mode::leave();
            transient::close_all();
            telar::dispose_owner(self.0.id());
        }
    }

    const CTRL: ModifiersState = ModifiersState {
        is_shift: false,
        is_ctrl: true,
        is_alt: false,
        is_meta: false,
    };
    const CTRL_SHIFT: ModifiersState = ModifiersState {
        is_shift: true,
        ..CTRL
    };

    fn rect(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect { x, y, w, h }
    }

    fn free_area(id: &str, at: Rect) -> Area {
        Area {
            id: AreaId::new(id),
            kind: Some(AreaKind::Free {
                rect: Some(at),
                anchor: None,
            }),
            ..Area::default()
        }
    }

    /// The desktop with the grid, the clock face and a note area over it, and on the grid a free container of two clocks and a row of one.
    fn overlapping(layout: &mut Layout) {
        let desktop = &mut layout.outputs[0].layers.desktop.areas;
        desktop.push(free_area("note", rect(0.4, 0.4, 0.2, 0.2)));
        let widget = |id: &str, at: Option<Rect>| Instance {
            id: InstanceId::new(id),
            module: Some("clock".to_string()),
            representation: Some(Representation::WidgetM),
            rect: at,
            ..Instance::default()
        };
        let cell = |col: u32| {
            Some(GroupKind::Cell {
                col,
                row: 0,
                col_span: 4,
                row_span: 3,
            })
        };
        desktop[0].groups = vec![
            Group {
                id: GroupId::new("pad"),
                kind: cell(0),
                arrange: Some(Arrange::Free),
                children: vec![
                    widget("pad-a", Some(rect(0.0, 0.0, 0.6, 0.6))),
                    widget("pad-b", Some(rect(0.3, 0.3, 0.6, 0.6))),
                ],
                ..Group::default()
            },
            Group {
                id: GroupId::new("shelf"),
                kind: cell(5),
                arrange: Some(Arrange::Row),
                children: vec![widget("row-a", None), widget("row-b", None)],
                ..Group::default()
            },
        ];
    }

    fn area(layer: LayerKind, id: &str) -> Node {
        Node::area(Some(SCREEN), layer, &AreaId::new(id))
    }

    fn pad_child(id: &str) -> Node {
        area(LayerKind::Desktop, "widgets").instance(&GroupId::new("pad"), &InstanceId::new(id))
    }

    fn order_of(rig: &Rig, layer: LayerKind) -> Vec<String> {
        stored(rig).outputs[0]
            .layers
            .get(layer)
            .areas
            .iter()
            .map(|area| area.id.to_string())
            .collect()
    }

    fn drawn_order(layer: LayerKind) -> Vec<String> {
        crate::rig::desktop()
            .resolved
            .layer(layer)
            .map(|layer| layer.areas.iter().map(|area| area.id.to_string()).collect())
            .unwrap_or_default()
    }

    fn pad_order(rig: &Rig) -> Vec<String> {
        stored(rig).outputs[0]
            .layers
            .desktop
            .areas
            .iter()
            .find(|area| area.id.as_str() == "widgets")
            .and_then(|area| area.groups.iter().find(|group| group.id.as_str() == "pad"))
            .map(|group| {
                group
                    .children
                    .iter()
                    .map(|child| child.id.to_string())
                    .collect()
            })
            .unwrap_or_default()
    }

    fn refusal() -> String {
        mode::refusal().peek().unwrap_or_default()
    }

    /// Ctrl+[ and Ctrl+] move an area one step among the areas of its layer that overlap, and Ctrl+Shift+[ and Ctrl+Shift+] all the way, however the layout reports the bracket with Shift held; each is one undo entry, and one already at the end says so. A grid tiles its layer, so it has no order.
    #[test]
    fn the_keys_restack_an_area_among_its_layer() {
        let rig = rig_with("order-areas", overlapping);
        let _owner = Owner::new();
        let _mode = enter(LayerKind::Desktop);
        assert_eq!(
            drawn_order(LayerKind::Desktop),
            ["widgets", "centre", "note"]
        );
        assert!(session::select(Selection::Area(area(
            LayerKind::Desktop,
            "note"
        ))));

        assert!(tap(Key::Char('['), CTRL));
        assert_eq!(
            order_of(&rig, LayerKind::Desktop),
            ["widgets", "note", "centre"]
        );
        assert_eq!(
            drawn_order(LayerKind::Desktop),
            ["widgets", "note", "centre"]
        );
        assert_eq!(rig.undo_label().as_deref(), Some("Send backward: note"));
        assert!(tap(Key::Char('['), CTRL_SHIFT));
        assert!(
            refusal().contains("already behind everything"),
            "{}",
            refusal()
        );

        assert!(tap(Key::Char(']'), CTRL_SHIFT));
        assert_eq!(
            drawn_order(LayerKind::Desktop),
            ["widgets", "centre", "note"]
        );
        assert_eq!(rig.undo_label().as_deref(), Some("Bring to front: note"));
        assert!(tap(Key::Char('{'), CTRL_SHIFT));
        assert_eq!(
            drawn_order(LayerKind::Desktop),
            ["widgets", "note", "centre"]
        );
        assert_eq!(rig.undo_label().as_deref(), Some("Send to back: note"));
        assert!(tap(Key::Char('}'), CTRL_SHIFT));
        assert!(tap(Key::Char(']'), CTRL));
        assert!(
            refusal().contains("already in front of everything"),
            "{}",
            refusal()
        );

        assert_eq!(session::undo().as_deref(), Ok("Bring to front: note"));
        assert_eq!(
            drawn_order(LayerKind::Desktop),
            ["widgets", "note", "centre"]
        );

        assert!(session::select(Selection::Area(area(
            LayerKind::Desktop,
            "widgets"
        ))));
        let before = stored(&rig);
        assert!(tap(Key::Char('['), CTRL_SHIFT));
        assert!(refusal().contains("free areas"), "{}", refusal());
        assert_eq!(stored(&rig), before);
    }

    /// The children of a free container restack among themselves; a child of any other container has nothing it is drawn over.
    #[test]
    fn the_children_of_a_free_container_restack_and_no_other_child_does() {
        let rig = rig_with("order-children", overlapping);
        let _owner = Owner::new();
        let _mode = enter(LayerKind::Desktop);
        assert!(session::select(Selection::Instance(pad_child("pad-a"))));
        assert!(tap(Key::Char(']'), CTRL));
        assert_eq!(pad_order(&rig), ["pad-b", "pad-a"]);
        assert_eq!(rig.undo_label().as_deref(), Some("Bring forward: Clock"));
        assert_eq!(session::undo().as_deref(), Ok("Bring forward: Clock"));
        assert_eq!(pad_order(&rig), ["pad-a", "pad-b"]);

        let row = area(LayerKind::Desktop, "widgets")
            .instance(&GroupId::new("shelf"), &InstanceId::new("row-a"));
        assert!(session::select(Selection::Instance(row)));
        let before = stored(&rig);
        assert!(tap(Key::Char(']'), CTRL));
        assert!(refusal().contains("free container"), "{}", refusal());
        assert_eq!(stored(&rig), before);
    }

    /// The menu of what overlaps something has "Order ▸", its ends greyed, and each of its rows is one undo entry; a grid's menu has none.
    #[test]
    fn the_order_submenu_restacks_as_the_keys_do() {
        let rig = rig_with("order-menu", overlapping);
        let _owner = Owner::new();
        let _mode = enter(LayerKind::Desktop);
        context::open(Asked {
            node: area(LayerKind::Desktop, "note"),
            window: LayerKind::Desktop,
            at: Some((960.0, 540.0)),
        })
        .expect("the menu opens");
        assert_eq!(
            context::sub_rows("Order"),
            [
                "Bring forward",
                "Send backward",
                "Bring to front",
                "Send to back"
            ]
        );
        context::pick("Send to back");
        assert_eq!(
            drawn_order(LayerKind::Desktop),
            ["widgets", "note", "centre"]
        );
        assert_eq!(rig.undo_label().as_deref(), Some("Send to back: note"));

        context::open(Asked {
            node: pad_child("pad-b"),
            window: LayerKind::Desktop,
            at: Some((100.0, 100.0)),
        })
        .expect("the menu opens");
        context::pick("Send backward");
        assert_eq!(pad_order(&rig), ["pad-b", "pad-a"]);

        context::open(Asked {
            node: area(LayerKind::Desktop, "widgets"),
            window: LayerKind::Desktop,
            at: Some((960.0, 540.0)),
        })
        .expect("the menu opens");
        assert!(
            !context::rows().iter().any(|row| row == "Order"),
            "{:?}",
            context::rows()
        );
    }

    /// An area the extended layout places keeps its place: a level laid over another never restacks it.
    #[test]
    fn an_inherited_area_is_refused() {
        let rig = rig_with("order-inherited", |layout| {
            let mut rule = OutputRule {
                matches: OutputMatch("*".into()),
                ..OutputRule::default()
            };
            rule.layers
                .desktop
                .areas
                .push(free_area("note", rect(0.4, 0.4, 0.2, 0.2)));
            *layout = Layout {
                id: LayoutId::new("mine"),
                extends: Some(LayoutId::new(layout::BUILT_IN)),
                outputs: vec![rule],
                ..Layout::default()
            };
        });
        let _owner = Owner::new();
        let _mode = enter(LayerKind::Desktop);
        let before = stored(&rig);
        assert!(session::select(Selection::Area(area(
            LayerKind::Desktop,
            "centre"
        ))));
        assert!(tap(Key::Char(']'), CTRL));
        assert_eq!(
            refusal(),
            "'centre' is written by a layout this one extends, so its order cannot be changed here"
        );
        assert_eq!(stored(&rig), before);
    }

    fn texture(id: &str, at: Rect) -> Area {
        Area {
            id: AreaId::new(id),
            kind: Some(AreaKind::Texture {
                rect: Some(at),
                image: None,
                gradient: Some(crate::modes::texture::first_gradient()),
                tile: None,
                blend: None,
                opacity: None,
            }),
            ..Area::default()
        }
    }

    /// On the lock layer the prompt is always drawn last and the readings' grid tiles its place, so only the textures restack, around them.
    #[test]
    fn on_the_lock_the_textures_restack_around_the_prompt() {
        let rig = rig_with("order-lock", |layout| {
            let lock = &mut layout.outputs[0].layers.lock.areas;
            lock.push(texture("wash", rect(0.0, 0.0, 1.0, 1.0)));
            lock.push(texture("tint", rect(0.0, 0.5, 1.0, 0.5)));
        });
        let _owner = Owner::new();
        let _mode = enter(LayerKind::Lock);
        assert!(session::select(Selection::Area(area(
            LayerKind::Lock,
            "tint"
        ))));
        assert!(tap(Key::Char('['), CTRL_SHIFT), "{}", refusal());
        let order = order_of(&rig, LayerKind::Lock);
        assert_eq!(order, ["lock-readings", "prompt", "tint", "wash"]);
        assert_eq!(
            drawn_order(LayerKind::Lock).last().map(String::as_str),
            Some("prompt")
        );

        for id in ["prompt", "lock-readings"] {
            assert!(session::select(Selection::Area(area(LayerKind::Lock, id))));
            assert!(tap(Key::Char(']'), CTRL));
            assert!(refusal().contains("free container"), "{id}: {}", refusal());
        }
    }

    /// Stacks overlap each other on the overlay layer, so they restack by key and by menu as free areas do; a container holds its children in an order of its own and has none among the areas.
    #[test]
    fn stacks_restack_on_the_overlay_and_a_container_group_has_no_order() {
        let rig = rig_with("order-stacks", overlapping);
        let _owner = Owner::new();
        let _mode = enter(LayerKind::Overlay);
        let before = stored(&rig);
        assert!(tap(
            Key::Char('N'),
            ModifiersState {
                is_shift: true,
                ..ModifiersState::default()
            }
        ));
        assert_eq!(order_of(&rig, LayerKind::Overlay), ["stack", "stack-2"]);
        assert_eq!(
            session::selected(),
            Selection::Area(area(LayerKind::Overlay, "stack-2"))
        );

        assert!(tap(Key::Char('['), CTRL));
        assert_eq!(order_of(&rig, LayerKind::Overlay), ["stack-2", "stack"]);
        assert_eq!(rig.undo_label().as_deref(), Some("Send backward: stack-2"));
        assert!(tap(Key::Char('['), CTRL));
        assert!(
            refusal().contains("already behind everything"),
            "{}",
            refusal()
        );
        assert_eq!(session::undo().as_deref(), Ok("Send backward: stack-2"));
        assert_eq!(order_of(&rig, LayerKind::Overlay), ["stack", "stack-2"]);
        assert_eq!(session::undo().as_deref(), Ok("Make the stack stack-2"));
        assert_eq!(stored(&rig), before);
        mode::leave();

        let _mode = enter(LayerKind::Desktop);
        let shelf = area(LayerKind::Desktop, "widgets").group(&GroupId::new("shelf"));
        assert!(session::select(Selection::Group(shelf)));
        let before = stored(&rig);
        assert!(tap(Key::Char(']'), CTRL));
        assert!(!refusal().is_empty(), "a container is told why it has none");
        assert_eq!(stored(&rig), before);
    }
}
