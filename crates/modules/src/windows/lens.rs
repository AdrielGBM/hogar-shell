use std::f32::consts::PI;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Lens {
    pub scale: f32,
    pub shift: f32,
}

impl Lens {
    pub const REST: Lens = Lens {
        scale: 1.0,
        shift: 0.0,
    };
}

/// How the entries spanning `spans` along a strip, in order, grow towards `peak` around `pointer`: each by how near its centre is to the pointer, falling to nothing at `reach`, and moved apart so they keep the gaps they had while the point under the pointer stays where it is.
pub fn lenses(spans: &[(f32, f32)], pointer: Option<f32>, peak: f32, reach: f32) -> Vec<Lens> {
    let Some(pointer) = pointer.filter(|_| peak > 1.0 && reach > 0.0) else {
        return vec![Lens::REST; spans.len()];
    };
    let scales: Vec<f32> = spans
        .iter()
        .map(|&(start, end)| {
            let distance = ((start + end) / 2.0 - pointer).abs() / reach;
            1.0 + (peak - 1.0) * falloff(distance)
        })
        .collect();
    let mut starts = Vec::with_capacity(spans.len());
    let mut next = spans.first().map_or(0.0, |span| span.0);
    for (index, (&(start, end), scale)) in spans.iter().zip(&scales).enumerate() {
        starts.push(next);
        let gap = spans
            .get(index + 1)
            .map_or(0.0, |following| following.0 - end);
        next += (end - start) * scale + gap;
    }
    let held = pointer - grown_at(spans, &scales, &starts, pointer);
    spans
        .iter()
        .zip(&scales)
        .zip(&starts)
        .map(|((&(start, end), &scale), &grown)| Lens {
            scale,
            shift: grown + (end - start) * scale / 2.0 + held - (start + end) / 2.0,
        })
        .collect()
}

fn falloff(distance: f32) -> f32 {
    match distance < 1.0 {
        true => (1.0 + (PI * distance).cos()) / 2.0,
        false => 0.0,
    }
}

fn grown_at(spans: &[(f32, f32)], scales: &[f32], grown: &[f32], point: f32) -> f32 {
    let mut at = point;
    for (index, &(start, end)) in spans.iter().enumerate() {
        if point < start {
            break;
        }
        at = grown[index] + (point.min(end) - start) * scales[index] + (point - end).max(0.0);
    }
    at
}

#[cfg(test)]
mod tests {
    use super::*;

    const PITCH: f32 = 48.0;
    const SIDE: f32 = 44.0;

    fn row(count: usize) -> Vec<(f32, f32)> {
        (0..count)
            .map(|index| {
                let start = index as f32 * PITCH;
                (start, start + SIDE)
            })
            .collect()
    }

    fn centre(span: (f32, f32)) -> f32 {
        (span.0 + span.1) / 2.0
    }

    fn drawn(spans: &[(f32, f32)], lenses: &[Lens]) -> Vec<(f32, f32)> {
        spans
            .iter()
            .zip(lenses)
            .map(|(&(start, end), lens)| {
                let half = (end - start) * lens.scale / 2.0;
                let middle = centre((start, end)) + lens.shift;
                (middle - half, middle + half)
            })
            .collect()
    }

    #[test]
    fn with_no_pointer_or_no_peak_nothing_grows_or_moves() {
        let spans = row(5);
        assert!(
            lenses(&spans, None, 1.6, 150.0)
                .iter()
                .all(|lens| *lens == Lens::REST)
        );
        assert!(
            lenses(&spans, Some(100.0), 1.0, 150.0)
                .iter()
                .all(|lens| *lens == Lens::REST)
        );
        assert!(lenses(&[], Some(10.0), 1.6, 150.0).is_empty());
    }

    #[test]
    fn the_entry_under_the_pointer_grows_to_the_peak_and_the_rest_fall_off_with_distance() {
        let spans = row(7);
        let under = centre(spans[3]);
        let grown = lenses(&spans, Some(under), 1.6, 3.0 * PITCH);
        assert!((grown[3].scale - 1.6).abs() < 1e-4, "{grown:?}");
        assert!(grown[2].scale < grown[3].scale && grown[1].scale < grown[2].scale);
        assert!(
            (grown[2].scale - grown[4].scale).abs() < 1e-4,
            "symmetric about the pointer"
        );
        assert_eq!(grown[0].scale, 1.0, "past the reach nothing grows");
        assert!(
            grown[3].shift.abs() < 1e-3,
            "the entry under the pointer stays put"
        );
        assert!(
            grown[2].shift < 0.0 && grown[4].shift > 0.0,
            "its neighbours make room"
        );
    }

    #[test]
    fn grown_entries_keep_their_gaps_and_never_overlap() {
        let spans = row(6);
        for pointer in [-30.0, 0.0, 10.0, 46.0, 100.0, 170.0, 290.0, 400.0] {
            let drawn = drawn(&spans, &lenses(&spans, Some(pointer), 2.0, 3.0 * PITCH));
            for pair in drawn.windows(2) {
                let gap = pair[1].0 - pair[0].1;
                assert!((gap - (PITCH - SIDE)).abs() < 1e-3, "{pointer}: {drawn:?}");
            }
        }
    }

    #[test]
    fn the_point_under_the_pointer_stays_under_it() {
        let spans = row(5);
        for pointer in [5.0, 22.0, 45.5, 60.0, 130.0, 200.0] {
            let lenses = lenses(&spans, Some(pointer), 1.8, 3.0 * PITCH);
            let Some(index) = spans
                .iter()
                .position(|&(start, end)| pointer >= start && pointer < end)
            else {
                continue;
            };
            let (start, end) = spans[index];
            let lens = lenses[index];
            let moved =
                centre((start, end)) + lens.shift + (pointer - centre((start, end))) * lens.scale;
            assert!((moved - pointer).abs() < 1e-3, "{pointer} drew at {moved}");
        }
    }

    #[test]
    fn growth_is_continuous_as_the_pointer_crosses_an_entry() {
        let spans = row(5);
        let mut last: Option<Vec<Lens>> = None;
        let mut pointer = 0.0;
        while pointer < 240.0 {
            let now = lenses(&spans, Some(pointer), 1.6, 3.0 * PITCH);
            if let Some(before) = &last {
                for (a, b) in before.iter().zip(&now) {
                    assert!((a.scale - b.scale).abs() < 0.05, "{pointer}");
                    assert!((a.shift - b.shift).abs() < 1.5, "{pointer}");
                }
            }
            last = Some(now);
            pointer += 0.5;
        }
    }
}
