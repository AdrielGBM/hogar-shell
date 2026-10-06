#[cfg(test)]
mod tests {
    use telar::MenuEntry;

    use layout::{LayerKind, LayoutId};
    use surfaces::transient;

    use crate::mode;
    use crate::rig::{enter, rig};
    use crate::templates;

    fn picked(entry: &MenuEntry) {
        match entry {
            MenuEntry::Custom { act: Some(act), .. } => act(),
            _ => panic!("a template is a row that picks"),
        }
    }

    #[test]
    fn the_gallery_is_a_strip_action_that_opens_a_menu_in_a_mode() {
        let _rig = rig("templates-strip");
        let _scope = telar::owner_scope();
        assert!(templates::open().is_err(), "no mode, no gallery");
        let _host = enter(LayerKind::Desktop);
        let (_, press) = crate::host::strip_actions()
            .into_iter()
            .find(|(label, _)| label() == "New layout from template…")
            .expect("the strip offers the gallery");
        press();
        assert!(transient::is_open(crate::context::ID));
        transient::close(crate::context::ID);
        mode::leave();
    }

    #[test]
    fn the_gallery_shows_every_template_and_picking_one_draws_a_new_layout() {
        let rig = rig("templates-pick");
        let _scope = telar::owner_scope();
        let _host = enter(LayerKind::Desktop);
        let mine = rig.store.borrow().get(&LayoutId::new("mine")).cloned();

        let gallery = templates::gallery();
        assert_eq!(gallery.len(), layout::templates::all().len());
        for entry in &gallery {
            if let MenuEntry::Custom { widget, .. } = entry {
                assert!(
                    widget().is_ok(),
                    "each row draws its name and what it holds"
                );
            }
        }

        picked(&gallery[0]);
        assert_eq!(rig.store.borrow().active_id().as_str(), "showcase");
        assert_eq!(
            rig.store.borrow().get(&LayoutId::new("mine")).cloned(),
            mine
        );
        assert_eq!(
            mode::confirmation().get().as_deref(),
            Some("Made the layout `showcase` from Showcase and switched to it")
        );

        picked(&gallery[0]);
        assert_eq!(
            rig.store.borrow().active_id().as_str(),
            "showcase-2",
            "a second pick makes a second layout"
        );
        assert_eq!(
            rig.store.borrow().history().undo.len(),
            0,
            "making a layout is not an edit of one"
        );
        assert!(templates::use_template("nothing-like-it").is_err());
        mode::leave();
    }
}
