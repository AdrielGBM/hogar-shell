//! What the app's tests share to put a screen in front of the commands under test.

#![cfg(test)]

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use layout::{LayerKind, Layout, LayoutId, LayoutStore, Resolved, ResolvedArea};
use surfaces::area::Surround;
use surfaces::reconcile::{self, Desktop};
use ui::host::Audience;

pub const SCREEN_SIZE: (f32, f32) = (1920.0, 1080.0);

/// The screen `output` as a shell with the default config and nothing reserved would show it, `resolved` drawn on it.
pub fn desktop(output: &str, resolved: Resolved) -> Desktop {
    Desktop {
        output: Some(output.to_string()),
        config: Arc::new(config::Config::default()),
        resolved,
        reserved: Default::default(),
        size: SCREEN_SIZE,
    }
}

/// [`desktop`] as the only screen the shell shows.
pub fn publish(output: &str, resolved: Resolved) {
    reconcile::publish(&[desktop(output, resolved)]);
}

/// The screen `output` of `size` under `config`, with the edges `resolved` reserves taken off it as a running shell takes them.
pub fn measured(
    output: &str,
    config: Arc<config::Config>,
    resolved: Resolved,
    size: (f32, f32),
) -> Desktop {
    Desktop {
        reserved: surfaces::layer_window::Reserved::of(&resolved, &config),
        config,
        size,
        ..desktop(output, resolved)
    }
}

/// `area`, written on `layer`, placed on `desktop` as a running shell places it.
pub fn placed<'a>(desktop: &'a Desktop, area: &ResolvedArea, layer: LayerKind) -> Surround<'a> {
    let audience = match layer {
        LayerKind::Lock => Audience::Anyone,
        _ => Audience::Owner,
    };
    Surround::placed(area.within, layer, desktop.on_screen(), audience)
}

/// A layouts directory of the test's own holding `mine` as `mine.toml` and each of `parents` under its id, loaded with `active` drawn and installed as the store the shell owns, and a handle on it to read the result back. Its own directory rather than the user's, so these run beside the commands that read the real one without either seeing the other's files.
pub fn shell_holding(
    test: &str,
    active: &str,
    mine: &Layout,
    parents: &[&Layout],
) -> Rc<RefCell<LayoutStore>> {
    ui::descriptor::install(crate::core::modules::MODULES);
    let dir = util::paths::isolated_root()
        .expect("a test process resolves under its scratch root")
        .join(format!("layout-verbs-{test}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a layouts directory");
    std::fs::write(
        dir.join("mine.toml"),
        toml::to_string_pretty(mine).expect("the layout serializes"),
    )
    .expect("a layout to edit");
    for parent in parents {
        std::fs::write(
            dir.join(format!("{}.toml", parent.id)),
            toml::to_string_pretty(parent).expect("the layout serializes"),
        )
        .expect("a layout it extends");
    }
    let (mut store, report) = LayoutStore::load(&dir);
    assert!(report.is_clean(), "{}", report.render());
    store
        .use_layout(&LayoutId::new(active))
        .expect("the store holds it");
    let store = Rc::new(RefCell::new(store));
    surfaces::layouts::install(Rc::clone(&store), Rc::new(|| {}));
    store
}
