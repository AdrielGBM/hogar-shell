//! The running shell an editor test edits: a store holding the shipped layout under a name of its own, drawn on one screen through a real reconcile and installed as the running shell's, with the editor started over it.
//!
//! Nothing reserves an edge, because a headless reconcile that opens a reservation strip aborts at thread exit (F-9).
#![cfg(test)]

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use config::Config;
use layout::{LayerKind, Layout, LayoutId, LayoutStore};
use platform_wayland::OutputDescriptor;
use surfaces::layer_window::Content;
use surfaces::reconcile::{Shell, plan};
use surfaces::transient;

/// The one screen a rig draws on.
pub(crate) const SCREEN: &str = "DP-1";

/// The shipped layout with nothing reserving an edge.
pub(crate) fn unreserved() -> Layout {
    let mut layout = layout::built_in();
    for rule in &mut layout.outputs {
        for layer in LayerKind::ALL {
            for area in &mut rule.layers.get_mut(layer).areas {
                area.reserve = Some(false);
            }
        }
    }
    layout
}

pub(crate) struct Rig {
    pub(crate) store: Rc<RefCell<LayoutStore>>,
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
    telar::reset_layout_runtime();
    telar::set_locale("en");
    telar::set_theme(Config::default().resolve_theme());
    let dir = util::paths::isolated_root()
        .expect("a test process resolves under its scratch root")
        .join(format!("editor-{test}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a layouts directory");
    let mut mine = unreserved();
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
    let store = Rc::new(RefCell::new(store));
    let shell = Rc::new(RefCell::new(Shell::new()));
    let redraw: Rc<dyn Fn()> = {
        let (store, shell) = (Rc::clone(&store), Rc::clone(&shell));
        Rc::new(move || {
            let desktops = {
                let store = store.borrow();
                plan(
                    &util::paths::config_dir().join("config.toml"),
                    &Arc::new(Config::default()),
                    store.active(),
                    store.all(),
                    &[OutputDescriptor {
                        name: Some(SCREEN.to_string()),
                        logical_size: Some((1920, 1080)),
                        position: (0, 0),
                        scale: 1,
                    }],
                    &|_| None,
                )
                .0
            };
            shell.borrow_mut().reconcile(&desktops, Content::Rebuild);
        })
    };
    redraw();
    transient::install(shell.borrow().windows().holder());
    surfaces::layouts::install(Rc::clone(&store), redraw);
    crate::install();
    Rig {
        store,
        _shell: shell,
    }
}
