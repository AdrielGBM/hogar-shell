//! A bar is rounded by its shape, not its style, so the editor and `layout set` both write its corners as `shape.radius`.

use std::convert::Infallible;

use crate::{Area, AreaKind, ResolvedArea, ResolvedAreaKind, Style};

pub fn bar_key(key: &str) -> &str {
    match key {
        "style.radius" => "shape.radius",
        _ => key,
    }
}

pub fn bar_style(area: &ResolvedArea) -> Style {
    let mut style = area.style.clone();
    if let ResolvedAreaKind::Bar { shape, .. } = &area.kind {
        style.radius = shape.radius;
    }
    style
}

pub fn restyle_bar(area: &mut Area, change: impl FnOnce(&mut Style)) {
    let _: Result<(), Infallible> = restyle(area, true, |style| {
        change(style);
        Ok(())
    });
}

/// A bar's kind is made a partial entry only when its corners change, so a change to the rest of its look writes nothing else.
pub fn restyle<E>(
    area: &mut Area,
    bar: bool,
    change: impl FnOnce(&mut Style) -> Result<(), E>,
) -> Result<(), E> {
    if !bar {
        let mut style = area.style.clone();
        change(&mut style)?;
        area.style = style;
        return Ok(());
    }
    let corners = match &area.kind {
        Some(AreaKind::Bar { shape, .. }) => shape.radius,
        _ => None,
    };
    let mut style = Style {
        radius: corners,
        ..area.style.clone()
    };
    change(&mut style)?;
    if style.radius != corners
        && let AreaKind::Bar { shape, .. } = area.kind.get_or_insert_with(unwritten_bar)
    {
        shape.radius = style.radius;
    }
    area.style = Style {
        radius: area.style.radius,
        ..style
    };
    Ok(())
}

fn unwritten_bar() -> AreaKind {
    AreaKind::Bar {
        edge: None,
        thickness: None,
        length: None,
        offset: None,
        shape: Default::default(),
        autohide: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AreaId, BarShape, Corners};

    fn bar(kind: Option<AreaKind>) -> Area {
        Area {
            id: AreaId::new("bar-top"),
            kind,
            ..Area::default()
        }
    }

    #[test]
    fn a_bar_s_corners_are_written_to_its_shape_and_the_rest_to_its_style() {
        let mut inherited = bar(None);
        restyle_bar(&mut inherited, |style| style.fill = Some("surface".into()));
        assert_eq!(inherited.kind, None, "nothing but the fill is written");
        assert_eq!(inherited.style.fill.as_deref(), Some("surface"));

        restyle_bar(&mut inherited, |style| {
            style.radius = Some(Corners::all(8.0))
        });
        let Some(AreaKind::Bar { shape, edge, .. }) = &inherited.kind else {
            panic!("a partial bar: {:?}", inherited.kind);
        };
        assert_eq!((shape.radius, *edge), (Some(Corners::all(8.0)), None));
        assert_eq!(inherited.style.radius, None);
        assert_eq!(bar_key("style.radius"), "shape.radius");
        assert_eq!(bar_key("style.fill"), "style.fill");
    }

    #[test]
    fn a_refused_change_writes_nothing() {
        let mut written = bar(Some(AreaKind::Bar {
            edge: None,
            thickness: Some(32.0),
            length: None,
            offset: None,
            shape: BarShape::default(),
            autohide: None,
        }));
        let before = written.clone();
        let refused = restyle(&mut written, true, |style| {
            style.radius = Some(Corners::all(4.0));
            Err("no")
        });
        assert_eq!((refused, written), (Err("no"), before));

        let mut plain = bar(None);
        restyle(&mut plain, false, |style| {
            style.radius = Some(Corners::all(4.0));
            Ok::<(), ()>(())
        })
        .expect("a change");
        assert_eq!(
            (plain.kind, plain.style.radius),
            (None, Some(Corners::all(4.0)))
        );
    }
}
