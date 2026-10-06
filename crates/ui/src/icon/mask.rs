//! An application's icon cut to the one silhouette `[icons] mask` names.

use std::sync::Arc;

use config::IconMask;
use telar::{
    Color, LayoutError, LayoutItem, LayoutStyle, Mask, Paint, PathData, PathStyle, Point,
    RectStyle, StyledContainer,
};

/// How square a squircle's sides are: `2` is a circle and the shape tends to a square as it grows.
const SQUIRCLE_EXPONENT: f32 = 5.0;
const SQUIRCLE_STEPS: usize = 96;

/// `icon`, `size` across, shown only inside `mask`.
pub(super) fn masked(
    icon: Box<dyn LayoutItem>,
    size: f32,
    mask: IconMask,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let shape: Box<dyn LayoutItem> = match mask {
        IconMask::None => return Ok(icon),
        IconMask::Circle => Box::new(StyledContainer::new(
            square(size),
            move |_| RectStyle::filled(Color::WHITE, size / 2.0),
            vec![],
        )?),
        IconMask::Squircle => Box::new(telar::Path::static_data(
            square(size),
            Arc::new(squircle(size)),
            || PathStyle {
                fill: Some(Paint::Solid(Color::WHITE)),
                ..PathStyle::default()
            },
        )?),
    };
    Ok(Box::new(Mask::new(
        square(size).flex_shrink(0.0),
        shape,
        icon,
    )?))
}

fn square(size: f32) -> LayoutStyle {
    LayoutStyle::new().width(size).height(size)
}

/// The superellipse `|x|^n + |y|^n = 1` filling a `size` square.
fn squircle(size: f32) -> PathData {
    let half = size / 2.0;
    let reach = |t: f32| t.signum() * t.abs().powf(2.0 / SQUIRCLE_EXPONENT);
    let points: Vec<Point> = (0..SQUIRCLE_STEPS)
        .map(|step| {
            let angle = step as f32 / SQUIRCLE_STEPS as f32 * std::f32::consts::TAU;
            let (sin, cos) = angle.sin_cos();
            Point::new(half + half * reach(cos), half + half * reach(sin))
        })
        .collect();
    PathData::polygon(&points)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_squircle_fills_its_square_edge_to_edge_and_rounds_its_corners() {
        let shape = squircle(40.0);
        let bounds = shape.bounds().expect("a shape with points");
        assert!((bounds.x).abs() < 0.01 && (bounds.width - 40.0).abs() < 0.01);
        assert!((bounds.y).abs() < 0.01 && (bounds.height - 40.0).abs() < 0.01);
        let farthest = shape
            .verbs()
            .iter()
            .filter_map(|verb| match verb {
                telar::PathVerb::MoveTo(p) | telar::PathVerb::LineTo(p) => {
                    Some(((p.x - 20.0).powi(2) + (p.y - 20.0).powi(2)).sqrt())
                }
                _ => None,
            })
            .fold(0.0f32, f32::max);
        assert!(farthest > 20.5, "fuller than a circle: {farthest}");
        assert!(
            farthest < 20.0 * std::f32::consts::SQRT_2 - 1.0,
            "with its corners rounded: {farthest}"
        );
    }
}
