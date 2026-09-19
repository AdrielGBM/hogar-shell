//! Parses `TELAR_PERF` window summaries; phases are matched by name so both the hardware and software renderer's differently-shaped lines parse the same way, and a window is only attributable to one piece of work if nothing else rendered while it filled.

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PhaseStat {
    pub avg_us: f64,
    pub max_us: f64,
    pub n: u64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PerfWindow {
    pub phases: Vec<(String, PhaseStat)>,
    pub damage_frames: u64,
    pub area_pct: Option<f64>,
}

impl PerfWindow {
    pub fn phase(&self, name: &str) -> Option<PhaseStat> {
        self.phases.iter().find(|(n, _)| n == name).map(|(_, s)| *s)
    }
}

/// Parses a summary, with or without its `perf[60f]` prefix. `None` if no phase in it parses.
pub fn parse(summary: &str) -> Option<PerfWindow> {
    let mut window = PerfWindow::default();
    for field in summary.split_whitespace() {
        let Some((name, value)) = field.split_once('=') else {
            continue;
        };
        match name {
            "damage" => window.damage_frames = value.parse().ok()?,
            "area" => window.area_pct = value.strip_suffix('%').and_then(|v| v.parse().ok()),
            _ => {
                if let Some(stat) = parse_phase(value) {
                    window.phases.push((name.to_owned(), stat));
                }
            }
        }
    }
    (!window.phases.is_empty()).then_some(window)
}

fn parse_phase(value: &str) -> Option<PhaseStat> {
    let (times, count) = value.split_once("us(n")?;
    let (avg, max) = times.split_once('/')?;
    Some(PhaseStat {
        avg_us: avg.parse().ok()?,
        max_us: max.parse().ok()?,
        n: count.strip_suffix(')')?.parse().ok()?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_software_window() {
        let window = parse(
            "perf[60f] build=41/95us(n60) clone=3/9us(n60) interpret=380/1210us(n59) frame=900/2400us(n59) present=410/980us(n59) plan=12/30us(n59) convert=90/200us(n59) mask=60/140us(n59) damage=59 area=0.4%",
        )
        .unwrap();
        assert_eq!(
            window.phase("interpret"),
            Some(PhaseStat {
                avg_us: 380.0,
                max_us: 1210.0,
                n: 59
            })
        );
        assert_eq!(window.phase("mask").unwrap().avg_us, 60.0);
        assert_eq!(window.phase("gpu"), None);
        assert_eq!(window.damage_frames, 59);
        assert_eq!(window.area_pct, Some(0.4));
    }

    #[test]
    fn a_software_window_drawn_straight_into_the_buffer_has_acquire_and_no_convert() {
        let window = parse(
            "perf[60f] build=40/90us(n60) clone=3/8us(n60) interpret=350/1100us(n59) frame=700/1900us(n59) present=120/300us(n59) plan=11/28us(n59) mask=55/130us(n59) acquire=45/400us(n59) damage=59 area=0.3%",
        )
        .unwrap();
        assert_eq!(
            window.phase("acquire"),
            Some(PhaseStat {
                avg_us: 45.0,
                max_us: 400.0,
                n: 59
            })
        );
        assert_eq!(window.phase("convert"), None);
        assert_eq!(window.phase("interpret").unwrap().max_us, 1100.0);
    }

    #[test]
    fn a_hardware_window_has_no_area_when_nothing_noted_damage() {
        let window = parse("build=50/80us(n60) interpret=200/400us(n60) gpu=700/1500us(n60) frame=950/1900us(n60) present=300/900us(n60) damage=0").unwrap();
        assert_eq!(window.phase("gpu").unwrap().max_us, 1500.0);
        assert_eq!(window.area_pct, None);
    }

    #[test]
    fn something_else_is_not_a_window() {
        assert_eq!(parse("hw init: format=Bgra8Unorm msaa=4"), None);
        assert_eq!(parse(""), None);
    }
}
