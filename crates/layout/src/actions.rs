//! One check of what a gesture may run, shared by the editor's Actions rows and `layout set … actions.<gesture>` so the two never disagree.

use util::report::Message;

use crate::{Action, AreaKind, LayerKind, ResolvedAreaKind, Trigger};

pub fn takes_actions(layer: LayerKind, kind: Option<&AreaKind>) -> Result<(), Message> {
    match (layer, kind) {
        (LayerKind::Lock, _) => Err(util::message!("gesture.on_lock")),
        (LayerKind::Background, _)
        | (_, Some(AreaKind::WallpaperRegion { .. } | AreaKind::Texture { .. })) => {
            Err(util::message!("gesture.behind"))
        }
        _ => Ok(()),
    }
}

/// A trust line is refused before resolution, since `layout trust` resolves: trust is the user's to give, never a gesture's.
pub fn action_from(
    trigger: &str,
    text: &str,
    resolves: impl Fn(&str) -> bool,
) -> Result<(Trigger, Action), Message> {
    let gesture = Trigger::from_name(trigger).ok_or_else(|| {
        let gestures: Vec<&str> = Trigger::ALL.iter().map(|it| it.as_str()).collect();
        util::message!(
            "gesture.unknown",
            trigger = trigger,
            gestures = gestures.join(", ")
        )
    })?;
    let chain = chain_of(text);
    for line in &chain {
        if crate::grants_trust(line) {
            return Err(util::message!("gesture.trust", line = line));
        }
        if !resolves(line) {
            return Err(util::message!("gesture.no_command", line = line));
        }
    }
    Ok((gesture, Action(chain)))
}

pub fn chain_of(text: &str) -> Vec<String> {
    text.split(';')
        .map(|line| line.trim().to_string())
        .filter(|line| !line.is_empty())
        .collect()
}

impl ResolvedAreaKind {
    pub fn unwritten(&self) -> AreaKind {
        match self {
            ResolvedAreaKind::Bar { .. } => AreaKind::Bar {
                edge: None,
                thickness: None,
                length: None,
                offset: None,
                shape: Default::default(),
                autohide: None,
            },
            ResolvedAreaKind::Grid { .. } => AreaKind::Grid {
                rect: None,
                cell: None,
                gap: None,
                anchor: None,
            },
            ResolvedAreaKind::Stack { .. } => AreaKind::Stack {
                anchor: None,
                offset: None,
                width: None,
                flow: None,
                output_policy: None,
                routes: Vec::new(),
                launcher: None,
            },
            ResolvedAreaKind::WallpaperRegion { .. } => AreaKind::WallpaperRegion {
                rect: None,
                source: None,
                fit: None,
                transition: None,
                focus: None,
                dim: None,
                blur: None,
                parallax: None,
            },
            ResolvedAreaKind::Texture { .. } => AreaKind::Texture {
                rect: None,
                image: None,
                gradient: None,
                tile: None,
                blend: None,
                opacity: None,
            },
            ResolvedAreaKind::Dock { .. } => AreaKind::Dock {
                edge: None,
                thickness: None,
            },
            ResolvedAreaKind::Free { .. } => AreaKind::Free {
                rect: None,
                anchor: None,
            },
            ResolvedAreaKind::Panel { .. } => AreaKind::Panel {
                owner: None,
                along: None,
                cols: None,
                rows: None,
                cell: None,
                gap: None,
            },
            ResolvedAreaKind::Prompt { .. } => AreaKind::Prompt { rect: None },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kind(name: &str) -> AreaKind {
        let mut table = toml::Table::new();
        table.insert("kind".to_string(), toml::Value::String(name.to_string()));
        toml::Value::Table(table).try_into().expect("a kind")
    }

    #[test]
    fn a_gesture_runs_only_where_something_can_be_pressed() {
        assert!(takes_actions(LayerKind::Top, Some(&kind("bar"))).is_ok());
        assert!(takes_actions(LayerKind::Desktop, Some(&kind("grid"))).is_ok());
        assert!(takes_actions(LayerKind::Desktop, None).is_ok());
        for (layer, kind) in [
            (LayerKind::Lock, Some(kind("grid"))),
            (LayerKind::Lock, None),
            (LayerKind::Background, Some(kind("free"))),
            (LayerKind::Desktop, Some(kind("wallpaper_region"))),
            (LayerKind::Desktop, Some(kind("texture"))),
        ] {
            assert!(
                takes_actions(layer, kind.as_ref()).is_err(),
                "{layer} {kind:?}"
            );
        }
        assert_eq!(
            takes_actions(LayerKind::Lock, None).map_err(|why| why.english()),
            Err(
                "the lock layer holds readings, never controls, so a gesture cannot run anything there"
                    .to_string()
            )
        );
    }

    #[test]
    fn a_resolved_kind_is_asked_as_the_kind_it_writes() {
        let resolved = ResolvedAreaKind::Free {
            rect: crate::Rect::default(),
            anchor: Default::default(),
        };
        assert_eq!(resolved.unwritten(), kind("free"));
    }

    #[test]
    fn a_chain_is_bound_only_when_every_line_of_it_may_run() {
        let resolves = |line: &str| line.starts_with("launcher") || line.starts_with("layout");
        assert_eq!(
            action_from("press", " launcher toggle ;; launcher close ; ", resolves),
            Ok((
                Trigger::Press,
                Action(vec![
                    "launcher toggle".to_string(),
                    "launcher close".to_string()
                ])
            ))
        );
        assert_eq!(
            action_from("press", "", resolves),
            Ok((Trigger::Press, Action(Vec::new())))
        );
        let refused = |trigger: &str, text: &str| {
            action_from(trigger, text, resolves)
                .expect_err(text)
                .english()
        };
        assert_eq!(
            refused("press", "launcher toggle; frobnicate"),
            "`frobnicate` is not a command this shell has"
        );
        assert!(
            refused("press", "layout   trust nord --all").contains("trust is yours to give"),
            "a trust line is refused even though it resolves"
        );
        assert!(
            refused("poke", "launcher toggle").starts_with("'poke' is not a gesture (press, "),
            "the gesture is checked before the chain"
        );
    }
}
