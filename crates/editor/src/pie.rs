//! The mode pie: the other edit modes round a circle, a press or a direction away (F-7 — at most eight petals, and never the only way to another mode).
//!
//! Whether it is open is kept here rather than in the strip that draws it, because the keyboard opens it too: the switcher key opens it, an arrow then picks the petal that lies that way.

use telar::{RwSignal, detached, signal};

use layout::LayerKind;

use crate::keys::Direction;

/// How wide the pie is.
pub const PIE: f32 = 112.0;
/// How wide each of its petals is.
pub const PETAL: f32 = 32.0;

/// One other mode on the pie, placed within the pie's own box.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Petal {
    pub layer: LayerKind,
    pub x: f32,
    pub y: f32,
    /// Where it lies from the centre, clockwise from the right.
    angle: f32,
}

thread_local! {
    static SHOWN: RwSignal<bool> = detached(|| signal(false));
}

/// Whether the pie is open, as a signal both the strip and the keyboard write.
pub fn shown() -> RwSignal<bool> {
    SHOWN.with(|shown| *shown)
}

/// The other modes round the pie's centre, evenly spaced and starting at the top.
pub fn petals(current: LayerKind) -> Vec<Petal> {
    let others: Vec<LayerKind> = LayerKind::ALL
        .into_iter()
        .filter(|layer| *layer != current)
        .collect();
    let reach = (PIE - PETAL) / 2.0;
    let step = std::f32::consts::TAU / others.len().max(1) as f32;
    others
        .into_iter()
        .enumerate()
        .map(|(at, layer)| {
            let angle = at as f32 * step - std::f32::consts::FRAC_PI_2;
            Petal {
                layer,
                x: reach + reach * angle.cos(),
                y: reach + reach * angle.sin(),
                angle,
            }
        })
        .collect()
}

/// The mode whose petal lies nearest `direction` from the centre of the pie `current`'s strip shows.
pub fn toward(current: LayerKind, direction: Direction) -> Option<LayerKind> {
    let wanted = match direction {
        Direction::Right => 0.0,
        Direction::Down => std::f32::consts::FRAC_PI_2,
        Direction::Left => std::f32::consts::PI,
        Direction::Up => -std::f32::consts::FRAC_PI_2,
    };
    let apart = |angle: f32| {
        let turn = (angle - wanted).rem_euclid(std::f32::consts::TAU);
        turn.min(std::f32::consts::TAU - turn)
    };
    petals(current)
        .into_iter()
        .min_by(|a, b| apart(a.angle).total_cmp(&apart(b.angle)))
        .map(|petal| petal.layer)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Four other modes sit at the four arrows, so each arrow reaches exactly one of them and every one is reached.
    #[test]
    fn every_arrow_reaches_its_own_petal() {
        for current in LayerKind::ALL {
            let reached: Vec<LayerKind> = [
                Direction::Up,
                Direction::Right,
                Direction::Down,
                Direction::Left,
            ]
            .into_iter()
            .filter_map(|direction| toward(current, direction))
            .collect();
            let petals: Vec<LayerKind> = petals(current).iter().map(|petal| petal.layer).collect();
            assert_eq!(reached, petals, "from {current}");
        }
    }
}
