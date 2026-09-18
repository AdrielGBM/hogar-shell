//! Judging a phase: the timeline and the protocol log of both runs, turned into a measured value and a verdict for each of the plan's criteria.
//!
//! Attribution is by time. The timeline stamps every scenario boundary and every discrete change from `CLOCK_REALTIME`; libwayland stamps every protocol message from the same clock, as a UTC time of day. A commit belongs to the scenario whose span holds it, and to the change whose window — from that change to the next — holds it. That is sound only because the director never runs two kinds of change at once; nothing here tries to untangle overlapping work.
//!
//! A verdict is only ever `Pass` when the thing it speaks for was actually observed. No data is `NotMeasured`, never a pass by default.

use std::path::Path;

use crate::geometry::{PxRect, PxRegion};
use crate::perf::{self, PerfWindow};
use crate::scene::NAMESPACE_PREFIX;
use crate::timeline::{self, Entry, LogicalRect, Record};
use crate::trace::{self, Commit, Damage, InputRegion, Trace};
use crate::wire::Stamp;

const DAY_US: u64 = 86_400_000_000;

/// How far outside the expected rects a damaged pixel may sit before it counts as excess, in buffer pixels: the renderer rounds a fractional box outward to whole pixels, and antialiasing can reach one pixel past a shape's edge.
pub const TOLERANCE_PX: i64 = 2;

/// A perf window has to start this long after its scenario did, so the frame that was already on the render thread when the scenario began cannot land in it.
const PERF_LEAD_IN_US: u64 = 50_000;

/// Damage rects at least this large on either side are the "everything" some WSIs send (`INT32_MAX`), not a real extent.
const EVERYTHING: i64 = 1 << 30;

pub const MEASURED_SCENARIOS: [&str; 6] = [
    "clock",
    "clock-rate",
    "notify",
    "notify-rate",
    "clip",
    "clip-rate",
];
pub const ALL_SCENARIOS: [&str; 9] = [
    "idle",
    "clock",
    "clock-rate",
    "notify",
    "notify-rate",
    "clip",
    "clip-rate",
    "drawer",
    "clicks",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub start: u64,
    pub end: u64,
}

impl Span {
    pub fn contains(&self, at: u64) -> bool {
        at >= self.start && at < self.end
    }
}

pub struct Event<'a> {
    pub at: u64,
    pub index: u32,
    pub role: &'a str,
    pub expected: &'a [LogicalRect],
}

pub struct TimedPerf {
    pub span: Span,
    pub window: PerfWindow,
}

/// One run: its timeline, and its protocol log replayed if there was one.
pub struct Run {
    pub timeline: Vec<Entry>,
    pub trace: Option<Trace>,
    day_start: u64,
    first: u64,
}

impl Run {
    pub fn new(timeline: Vec<Entry>, trace: Option<Trace>) -> Self {
        let first = timeline.first().map_or(0, |e| e.at_us);
        Self {
            timeline,
            trace,
            day_start: first - first % DAY_US,
            first,
        }
    }

    pub fn load(dir: &Path) -> Result<Self, String> {
        let path = dir.join("timeline.tsv");
        let text =
            std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let trace = std::fs::read_to_string(dir.join("wayland.log"))
            .ok()
            .map(|log| trace::replay(&log))
            .filter(|t| t.messages > 0);
        Ok(Self::new(timeline::read(&text), trace))
    }

    pub fn meta(&self, key: &str) -> Option<&str> {
        self.metas(key).last().copied()
    }

    pub fn metas(&self, key: &str) -> Vec<&str> {
        self.timeline
            .iter()
            .filter_map(|e| match &e.record {
                Record::Meta { key: k, value } if k == key => Some(value.as_str()),
                _ => None,
            })
            .collect()
    }

    pub fn scenario(&self, name: &str) -> Option<Span> {
        let edge = |start: bool| {
            self.timeline.iter().find_map(|e| match &e.record {
                Record::Scenario { name: n, start: s } if n == name && *s == start => Some(e.at_us),
                _ => None,
            })
        };
        Some(Span {
            start: edge(true)?,
            end: edge(false)?,
        })
    }

    pub fn events(&self, scenario: &str) -> Vec<Event<'_>> {
        let mut events: Vec<Event<'_>> = self
            .timeline
            .iter()
            .filter_map(|e| match &e.record {
                Record::Event {
                    scenario: s,
                    index,
                    role,
                    expected,
                } if s == scenario => Some(Event {
                    at: e.at_us,
                    index: *index,
                    role,
                    expected,
                }),
                _ => None,
            })
            .collect();
        events.sort_by_key(|e| e.at);
        events
    }

    /// A protocol stamp on the timeline's clock, or `None` for a stamp that is not wall-clock time.
    pub fn time(&self, stamp: Stamp) -> Option<u64> {
        let Stamp::TimeOfDay(of_day) = stamp else {
            return None;
        };
        let mut at = self.day_start + of_day;
        // A run that crosses midnight: the time of day wrapped, the absolute time did not.
        if at + DAY_US / 2 < self.first {
            at += DAY_US;
        }
        Some(at)
    }

    /// The spike's name for a surface, from its layer-shell namespace.
    pub fn role(&self, surface: usize) -> Option<&str> {
        self.trace
            .as_ref()?
            .namespace(surface)?
            .strip_prefix(NAMESPACE_PREFIX)
    }

    pub fn commits(&self, role: &str, span: Span) -> Vec<&Commit> {
        let Some(trace) = &self.trace else {
            return Vec::new();
        };
        trace
            .commits
            .iter()
            .filter(|c| self.role(c.surface) == Some(role))
            .filter(|c| self.time(c.stamp).is_some_and(|at| span.contains(at)))
            .collect()
    }

    /// Commits that attached a buffer: frames, as opposed to state changes such as a new input region.
    pub fn frames(&self, role: &str, span: Span) -> Vec<&Commit> {
        self.commits(role, span)
            .into_iter()
            .filter(|c| c.attached)
            .collect()
    }

    pub fn roles(&self) -> Vec<String> {
        let Some(trace) = &self.trace else {
            return Vec::new();
        };
        let mut roles: Vec<String> = (0..trace.surfaces.len())
            .filter_map(|s| self.role(s).map(str::to_owned))
            .collect();
        roles.sort();
        roles.dedup();
        roles
    }

    /// Every perf window with the span it covers: from the previous summary to its own.
    pub fn perf(&self) -> Vec<TimedPerf> {
        let mut previous = None;
        let mut windows = Vec::new();
        for entry in &self.timeline {
            let Record::Perf { summary } = &entry.record else {
                continue;
            };
            if let (Some(start), Some(window)) = (previous, perf::parse(summary)) {
                windows.push(TimedPerf {
                    span: Span {
                        start,
                        end: entry.at_us,
                    },
                    window,
                });
            }
            previous = Some(entry.at_us);
        }
        windows
    }

    /// The perf windows that filled entirely inside `scenario`, and so describe its frames and nothing else.
    pub fn clean_perf(&self, scenario: &str) -> Vec<PerfWindow> {
        let Some(span) = self.scenario(scenario) else {
            return Vec::new();
        };
        self.perf()
            .into_iter()
            .filter(|p| p.span.start >= span.start + PERF_LEAD_IN_US && p.span.end <= span.end)
            .map(|p| p.window)
            .collect()
    }

    pub fn rss(&self, label: &str) -> Option<&[(String, u64)]> {
        self.timeline.iter().rev().find_map(|e| match &e.record {
            Record::Rss { label: l, fields } if l == label => Some(fields.as_slice()),
            _ => None,
        })
    }

    pub fn rss_field(&self, label: &str, field: &str) -> Option<u64> {
        self.rss(label)?
            .iter()
            .find(|(k, _)| k == field)
            .map(|(_, v)| *v)
    }

    pub fn cpu(&self, label: &str) -> Option<(u64, Option<u64>)> {
        self.timeline.iter().find_map(|e| match &e.record {
            Record::Cpu {
                label: l,
                process_us,
                compositor_us,
            } if l == label => Some((*process_us, *compositor_us)),
            _ => None,
        })
    }

    /// CPU used during a scenario, as a share of one core: this process, and the compositor where it could be read.
    pub fn scenario_cpu(&self, scenario: &str) -> Option<(f64, Option<f64>)> {
        let span = self.scenario(scenario)?;
        let (p0, c0) = self.cpu(&format!("{scenario}:start"))?;
        let (p1, c1) = self.cpu(&format!("{scenario}:end"))?;
        let wall = span.end.saturating_sub(span.start).max(1) as f64;
        let compositor = c0.zip(c1).map(|(a, b)| b.saturating_sub(a) as f64 / wall);
        Some((p1.saturating_sub(p0) as f64 / wall, compositor))
    }

    pub fn logs(&self) -> Vec<(&str, &str, &str)> {
        self.timeline
            .iter()
            .filter_map(|e| match &e.record {
                Record::Log {
                    level,
                    target,
                    message,
                } => Some((level.as_str(), target.as_str(), message.as_str())),
                _ => None,
            })
            .collect()
    }

    pub fn targets(&self) -> Vec<(&str, &str, LogicalRect)> {
        self.timeline
            .iter()
            .filter_map(|e| match &e.record {
                Record::Target { class, id, rect } => Some((class.as_str(), id.as_str(), *rect)),
                _ => None,
            })
            .collect()
    }

    pub fn presses(&self, span: Span) -> Vec<(&str, f64, f64)> {
        self.timeline
            .iter()
            .filter(|e| span.contains(e.at_us))
            .filter_map(|e| match &e.record {
                Record::Press { role, x, y } => Some((role.as_str(), *x, *y)),
                _ => None,
            })
            .collect()
    }

    /// The renderer that actually drew, as the perf windows show it: only the software renderer has `plan`/`convert`, only the hardware one has `gpu`.
    pub fn observed_backend(&self) -> Option<&'static str> {
        let windows = self.perf();
        let has = |phase: &str| windows.iter().any(|p| p.window.phase(phase).is_some());
        match (has("gpu"), has("convert") || has("plan")) {
            (true, true) => Some("mixed"),
            (true, false) => Some("hardware"),
            (false, true) => Some("software"),
            (false, false) => None,
        }
    }

    pub fn fell_back(&self) -> bool {
        self.logs()
            .iter()
            .any(|(_, _, m)| m.contains("falling back to SW"))
    }
}

/// A frame's declared damage in buffer pixels, against the buffer it was declared on.
pub struct FrameDamage {
    pub damage: PxRegion,
    pub bounds: PxRect,
    /// Buffer pixels per logical pixel, from the buffer's size over the configured one.
    pub scale: f64,
}

impl FrameDamage {
    pub fn area(&self) -> i64 {
        self.damage.area_within(self.bounds)
    }

    pub fn fraction(&self) -> f64 {
        self.area() as f64 / self.bounds.area().max(1) as f64
    }

    pub fn full(&self) -> bool {
        self.area() >= self.bounds.area()
    }
}

/// `None` when the log never showed the buffer being created, so there is nothing to measure the damage against.
pub fn frame_damage(commit: &Commit) -> Option<FrameDamage> {
    let (w, h) = commit.buffer?;
    let bounds = PxRect::new(0, 0, w, h);
    let scale = match commit.logical {
        Some((lw, _)) if lw > 0 => w as f64 / lw as f64,
        _ => 1.0,
    };
    let mut damage = PxRegion::default();
    for declared in &commit.damage {
        let rect = match *declared {
            Damage::Buffer(r) => Some(r),
            Damage::Surface(r) if r.w >= EVERYTHING || r.h >= EVERYTHING => Some(bounds),
            Damage::Surface(r) => Some(PxRect::enclosing(
                r.x as f64, r.y as f64, r.w as f64, r.h as f64, scale,
            )),
        };
        if let Some(clipped) = rect.and_then(|r| r.intersect(bounds)) {
            damage.add(clipped);
        }
    }
    Some(FrameDamage {
        damage,
        bounds,
        scale,
    })
}

fn scaled(rects: &[LogicalRect], scale: f64, inflate: i64) -> PxRegion {
    PxRegion::from_rects(
        rects
            .iter()
            .map(|r| PxRect::enclosing(r.x, r.y, r.w, r.h, scale).inflate(inflate)),
    )
}

/// What one discrete change was sent to the compositor as, against what the layout said it should repaint.
#[derive(Clone, Debug, PartialEq)]
pub struct EventCheck {
    pub index: u32,
    pub frames: usize,
    /// Pixels damaged across the change's frames, counted once.
    pub damaged: i64,
    /// Pixels the expected rects cover.
    pub expected: i64,
    /// Damaged pixels outside the expected rects even after [`TOLERANCE_PX`].
    pub excess: i64,
    /// Expected pixels the damage covered.
    pub covered: i64,
    /// The largest share of the buffer any one of the change's frames damaged.
    pub worst_fraction: f64,
    pub full_frames: usize,
    pub buffer: Option<(i64, i64)>,
}

/// Checks each change of `scenario`: its frames are the ones committed on its surface between it and the next change (or the scenario's end). `None` when there is no protocol log or no such scenario.
pub fn check_events(run: &Run, scenario: &str) -> Option<Vec<EventCheck>> {
    run.trace.as_ref()?;
    let span = run.scenario(scenario)?;
    let events = run.events(scenario);
    let mut checks = Vec::new();
    for (i, event) in events.iter().enumerate() {
        let end = events.get(i + 1).map_or(span.end, |next| next.at);
        let frames: Vec<FrameDamage> = run
            .frames(
                event.role,
                Span {
                    start: event.at,
                    end,
                },
            )
            .into_iter()
            .filter_map(frame_damage)
            .collect();
        let scale = frames.first().map_or(1.0, |f| f.scale);
        let mut damaged = PxRegion::default();
        for frame in &frames {
            damaged.extend(&frame.damage);
        }
        let exact = scaled(event.expected, scale, 0);
        let allowed = scaled(event.expected, scale, TOLERANCE_PX);
        checks.push(EventCheck {
            index: event.index,
            frames: frames.len(),
            damaged: damaged.area(),
            expected: exact.area(),
            excess: damaged.area_outside(&allowed),
            covered: damaged.area_covering(&exact),
            worst_fraction: frames.iter().map(FrameDamage::fraction).fold(0.0, f64::max),
            full_frames: frames.iter().filter(|f| f.full()).count(),
            buffer: frames.first().map(|f| (f.bounds.w, f.bounds.h)),
        });
    }
    Some(checks)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Pass,
    Fail,
    /// Some but not all of what the criterion needs was observed.
    Incomplete,
    NotMeasured,
    OutOfScope,
}

impl Status {
    pub fn label(self) -> &'static str {
        match self {
            Status::Pass => "PASS",
            Status::Fail => "FAIL",
            Status::Incomplete => "INCOMPLETE",
            Status::NotMeasured => "NOT MEASURED",
            Status::OutOfScope => "OUT OF SCOPE",
        }
    }
}

pub struct Verdict {
    pub id: &'static str,
    pub title: &'static str,
    pub status: Status,
    pub measured: String,
    pub notes: Vec<String>,
}

impl Verdict {
    fn new(
        id: &'static str,
        title: &'static str,
        status: Status,
        measured: impl Into<String>,
    ) -> Self {
        Self {
            id,
            title,
            status,
            measured: measured.into(),
            notes: Vec::new(),
        }
    }

    fn note(mut self, note: impl Into<String>) -> Self {
        self.notes.push(note.into());
        self
    }
}

fn percent(fraction: f64) -> String {
    format!("{:.2}%", fraction * 100.0)
}

fn no_log(id: &'static str, title: &'static str, run: &Run) -> Option<Verdict> {
    if run.trace.is_some() {
        return None;
    }
    Some(Verdict::new(
        id,
        title,
        Status::NotMeasured,
        "no WAYLAND_DEBUG protocol log for the merged run",
    ))
}

/// Criterion 1: idle clock-tick frames damage < 5 % of the window.
pub fn clock_ticks(merged: &Run) -> Verdict {
    const ID: &str = "1";
    const TITLE: &str = "clock-tick frames damage < 5% of the window";
    if let Some(v) = no_log(ID, TITLE, merged) {
        return v;
    }
    let Some(checks) = check_events(merged, "clock").filter(|c| !c.is_empty()) else {
        return Verdict::new(
            ID,
            TITLE,
            Status::NotMeasured,
            "the clock scenario is missing from the timeline",
        );
    };
    let frames: usize = checks.iter().map(|c| c.frames).sum();
    if frames == 0 {
        return Verdict::new(
            ID,
            TITLE,
            Status::NotMeasured,
            format!("{} ticks, but no frame was committed for any", checks.len()),
        );
    }
    let worst = checks.iter().map(|c| c.worst_fraction).fold(0.0, f64::max);
    let ticked: Vec<&EventCheck> = checks.iter().filter(|c| c.frames > 0).collect();
    let mean = ticked.iter().map(|c| c.damaged as f64).sum::<f64>()
        / ticked
            .iter()
            .map(|c| c.buffer.map_or(1.0, |(w, h)| (w * h) as f64))
            .sum::<f64>()
            .max(1.0);
    let buffer = checks.iter().find_map(|c| c.buffer).unwrap_or((0, 0));
    let mut verdict = Verdict::new(
        ID,
        TITLE,
        if worst < 0.05 {
            Status::Pass
        } else {
            Status::Fail
        },
        format!(
            "worst frame {} · mean per tick {} of the {}×{} buffer · {} ticks, {} frames",
            percent(worst),
            percent(mean),
            buffer.0,
            buffer.1,
            checks.len(),
            frames
        ),
    );
    let silent = checks.iter().filter(|c| c.frames == 0).count();
    if silent > 0 {
        verdict = verdict.note(format!("{silent} tick(s) produced no frame at all"));
    }
    let excess = checks.iter().map(|c| c.excess).max().unwrap_or(0);
    verdict.note(if excess == 0 {
        "every tick's damage stayed inside the clock chip".to_owned()
    } else {
        format!("damage reached outside the clock chip by up to {excess} px")
    })
}

fn damage_containment(
    merged: &Run,
    id: &'static str,
    title: &'static str,
    scenario: &str,
    noun: &str,
) -> Verdict {
    if let Some(v) = no_log(id, title, merged) {
        return v;
    }
    let Some(checks) = check_events(merged, scenario).filter(|c| !c.is_empty()) else {
        return Verdict::new(
            id,
            title,
            Status::NotMeasured,
            format!("the {scenario} scenario is missing from the timeline"),
        );
    };
    let observed: Vec<&EventCheck> = checks.iter().filter(|c| c.frames > 0).collect();
    if observed.is_empty() {
        return Verdict::new(
            id,
            title,
            Status::NotMeasured,
            format!(
                "{} {noun}, but no frame was committed for any",
                checks.len()
            ),
        );
    }
    let clean = observed.iter().filter(|c| c.excess == 0).count();
    let worst_excess = checks.iter().map(|c| c.excess).max().unwrap_or(0);
    let worst_cover = observed
        .iter()
        .map(|c| c.covered as f64 / c.expected.max(1) as f64)
        .fold(1.0, f64::min);
    let status = if worst_excess > 0 {
        Status::Fail
    } else if observed.len() < checks.len() {
        Status::Incomplete
    } else {
        Status::Pass
    };
    let mut verdict = Verdict::new(
        id,
        title,
        status,
        format!(
            "{clean}/{} {noun} inside the expected rects (±{TOLERANCE_PX} px) · worst excess {worst_excess} px · least coverage {}",
            checks.len(),
            percent(worst_cover)
        ),
    );
    if observed.len() < checks.len() {
        verdict = verdict.note(format!(
            "{} of them produced no frame",
            checks.len() - observed.len()
        ));
    }
    if worst_cover < 0.999 {
        verdict = verdict.note(
            "coverage under 100% means the declared damage missed pixels the layout moved — not this criterion, but a correctness question for the renderer",
        );
    }
    verdict
}

/// Criterion 2: an arriving notification damages only its card and the siblings it moves.
pub fn arrivals(merged: &Run) -> Verdict {
    damage_containment(
        merged,
        "2",
        "an arrival damages only its card ∪ the siblings it moves",
        "notify",
        "arrivals",
    )
}

/// Criterion 3: a clip resize damages only the old and new clip bounds.
pub fn clip_resizes(merged: &Run) -> Verdict {
    damage_containment(
        merged,
        "3",
        "a clip resize damages only old ∪ new clip bounds",
        "clip",
        "resizes",
    )
}

/// The average of one phase across windows, each window weighted by its sample count.
pub fn weighted(windows: &[PerfWindow], phase: &str) -> Option<f64> {
    let (sum, n) = windows
        .iter()
        .filter_map(|w| w.phase(phase))
        .fold((0.0, 0u64), |(sum, n), s| {
            (sum + s.avg_us * s.n as f64, n + s.n)
        });
    (n > 0).then(|| sum / n as f64)
}

pub fn slowest(windows: &[PerfWindow], phase: &str) -> Option<f64> {
    windows
        .iter()
        .filter_map(|w| w.phase(phase))
        .map(|s| s.max_us)
        .reduce(f64::max)
}

/// Criterion 4: frame interpret < 2 ms, judged on the slowest frame of every clean window the merged run produced.
pub fn interpret(merged: &Run) -> Verdict {
    const ID: &str = "4";
    const TITLE: &str = "frame interpret < 2 ms";
    let mut per_scenario = Vec::new();
    let mut all = Vec::new();
    for scenario in ALL_SCENARIOS {
        let windows = merged.clean_perf(scenario);
        if let (Some(avg), Some(max)) = (
            weighted(&windows, "interpret"),
            slowest(&windows, "interpret"),
        ) {
            per_scenario.push(format!(
                "{scenario}: avg {avg:.0} µs, slowest {max:.0} µs ({} windows)",
                windows.len()
            ));
        }
        all.extend(windows);
    }
    let (Some(avg), Some(max)) = (weighted(&all, "interpret"), slowest(&all, "interpret")) else {
        return Verdict::new(
            ID,
            TITLE,
            Status::NotMeasured,
            "no TELAR_PERF window filled entirely inside a scenario (was TELAR_PERF set?)",
        );
    };
    let mut verdict = Verdict::new(
        ID,
        TITLE,
        if max < 2000.0 {
            Status::Pass
        } else {
            Status::Fail
        },
        format!(
            "slowest frame {max:.0} µs · average {avg:.0} µs over {} clean windows",
            all.len()
        ),
    );
    if let Some(mask) = weighted(&all, "mask") {
        verdict = verdict.note(format!(
            "of which masking averages {mask:.0} µs — interpret − mask ≈ {:.0} µs of rasterisation",
            avg - mask
        ));
    }
    for line in per_scenario {
        verdict = verdict.note(line);
    }
    verdict
}

/// Criterion 5: no full-surface frame on clock ticks, arrivals or clip resizes.
pub fn full_frames(merged: &Run) -> Verdict {
    const ID: &str = "5";
    const TITLE: &str = "no full-surface frame on ticks, arrivals or clip resizes";
    if let Some(v) = no_log(ID, TITLE, merged) {
        return v;
    }
    let mut frames = 0;
    let mut full = 0;
    let mut unmeasured = 0;
    let mut breakdown = Vec::new();
    for scenario in MEASURED_SCENARIOS {
        let Some(span) = merged.scenario(scenario) else {
            continue;
        };
        let commits = merged.frames("top", span);
        let damage: Vec<FrameDamage> = commits.iter().filter_map(|c| frame_damage(c)).collect();
        let whole = damage.iter().filter(|d| d.full()).count();
        unmeasured += commits.len() - damage.len();
        frames += damage.len();
        full += whole;
        breakdown.push(format!("{scenario}: {whole} of {}", damage.len()));
    }
    if frames == 0 {
        return Verdict::new(
            ID,
            TITLE,
            Status::NotMeasured,
            "no frame of the merged window was found in those scenarios",
        );
    }
    let mut verdict = Verdict::new(
        ID,
        TITLE,
        if full == 0 {
            Status::Pass
        } else {
            Status::Fail
        },
        format!(
            "{full} full-surface frames of {frames} ({})",
            breakdown.join(", ")
        ),
    );
    if unmeasured > 0 {
        verdict = verdict.note(format!(
            "{unmeasured} frame(s) left out: their buffer's creation was not in the log"
        ));
    }
    if let Some(idle) = merged.scenario("idle") {
        verdict = verdict.note(format!(
            "idle (nothing changing) committed {} frame(s)",
            merged.frames("top", idle).len()
        ));
    }
    verdict
}

/// What one frame of a scenario cost the renderer, from its clean perf windows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrameCost {
    /// Time the renderer spent working, with the wait for the compositor taken out: `plan + interpret + convert` on software, `interpret + gpu − present` on hardware.
    pub work_us: f64,
    /// The renderer's whole `render_frame`, the wait for a free buffer or a vsync included.
    pub frame_us: f64,
    pub frames: u64,
}

pub fn frame_cost(windows: &[PerfWindow]) -> Option<FrameCost> {
    let frames = windows
        .iter()
        .filter_map(|w| w.phase("interpret"))
        .map(|s| s.n)
        .sum();
    let interpret = weighted(windows, "interpret")?;
    let work = if windows.iter().any(|w| w.phase("gpu").is_some()) {
        interpret + weighted(windows, "gpu").unwrap_or(0.0)
            - weighted(windows, "present").unwrap_or(0.0)
    } else {
        interpret
            + weighted(windows, "plan").unwrap_or(0.0)
            + weighted(windows, "convert").unwrap_or(0.0)
    };
    Some(FrameCost {
        work_us: work,
        frame_us: weighted(windows, "frame").unwrap_or(0.0),
        frames,
    })
}

/// Criterion 6: the drawer animation costs at most twice as much per frame merged as it does on a surface of its own.
pub fn drawer(per_surface: &Run, merged: &Run) -> Verdict {
    const ID: &str = "6";
    const TITLE: &str = "drawer animation ≤ 2× the per-surface frame cost";
    let today = frame_cost(&per_surface.clean_perf("drawer"));
    let dec1 = frame_cost(&merged.clean_perf("drawer"));
    let (Some(today), Some(dec1)) = (today, dec1) else {
        return Verdict::new(
            ID,
            TITLE,
            Status::NotMeasured,
            "one of the runs has no perf window that filled inside its drawer scenario",
        );
    };
    let ratio = dec1.work_us / today.work_us.max(1.0);
    let mut verdict = Verdict::new(
        ID,
        TITLE,
        if ratio <= 2.0 { Status::Pass } else { Status::Fail },
        format!(
            "{ratio:.2}× — {:.0} µs merged vs {:.0} µs per-surface of renderer work per frame ({} and {} frames)",
            dec1.work_us, today.work_us, dec1.frames, today.frames
        ),
    )
    .note(format!(
        "whole render_frame including the wait for the compositor: {:.0} µs merged vs {:.0} µs per-surface ({:.2}×)",
        dec1.frame_us,
        today.frame_us,
        dec1.frame_us / today.frame_us.max(1.0)
    ));
    if let (Some((p, c)), Some((m, mc))) = (
        per_surface.scenario_cpu("drawer"),
        merged.scenario_cpu("drawer"),
    ) {
        verdict = verdict.note(format!(
            "process CPU over the scenario: {} merged vs {} per-surface of one core{}",
            percent(m),
            percent(p),
            match (mc, c) {
                (Some(mc), Some(c)) => format!("; compositor {} vs {}", percent(mc), percent(c)),
                _ => String::new(),
            }
        ));
    }
    verdict
}

fn mib(kb: u64) -> String {
    format!("{:.1} MiB", kb as f64 / 1024.0)
}

/// Criterion 7: the merged window's RSS stays under 150 % of today's, at rest and at its peak.
pub fn memory(per_surface: &Run, merged: &Run) -> Verdict {
    const ID: &str = "7";
    const TITLE: &str = "RSS < 150% of the per-surface model";
    let pair = |label: &str, field: &str| {
        Some((
            per_surface.rss_field(label, field)?,
            merged.rss_field(label, field)?,
        ))
    };
    let (Some(steady), Some(peak)) = (pair("steady", "VmRSS"), pair("end", "VmHWM")) else {
        return Verdict::new(
            ID,
            TITLE,
            Status::NotMeasured,
            "a run is missing its memory readings",
        );
    };
    let ratio = |(today, dec1): (u64, u64)| dec1 as f64 / today.max(1) as f64;
    let (at_rest, at_peak) = (ratio(steady), ratio(peak));
    let mut verdict = Verdict::new(
        ID,
        TITLE,
        if at_rest < 1.5 && at_peak < 1.5 {
            Status::Pass
        } else {
            Status::Fail
        },
        format!(
            "at rest {:.0}% ({} vs {}) · peak {:.0}% ({} vs {})",
            at_rest * 100.0,
            mib(steady.1),
            mib(steady.0),
            at_peak * 100.0,
            mib(peak.1),
            mib(peak.0)
        ),
    );
    if let Some(open) = pair("drawer", "VmRSS") {
        verdict = verdict.note(format!(
            "with the drawer open {:.0}% ({} vs {})",
            ratio(open) * 100.0,
            mib(open.1),
            mib(open.0)
        ));
    }
    for field in ["RssAnon", "RssFile", "RssShmem"] {
        if let Some((today, dec1)) = pair("steady", field) {
            verdict = verdict.note(format!(
                "at rest {field}: {} merged vs {} per-surface",
                mib(dec1),
                mib(today)
            ));
        }
    }
    verdict
}

fn logical(r: LogicalRect) -> PxRect {
    PxRect::enclosing(r.x, r.y, r.w, r.h, 1.0)
}

/// The input region the merged window had in effect at `at`: the last one a commit carried before it.
pub fn region_at(run: &Run, at: u64) -> Option<InputRegion> {
    let trace = run.trace.as_ref()?;
    trace
        .commits
        .iter()
        .rev()
        .filter(|c| run.role(c.surface) == Some("top"))
        .filter(|c| run.time(c.stamp).is_some_and(|t| t <= at))
        .find_map(|c| c.input_region.clone())
}

pub struct RegionCheck {
    pub bars_covered: usize,
    pub bars: usize,
    pub empty_clear: usize,
    pub empty: usize,
    pub rects: usize,
}

pub fn check_region(region: &InputRegion, targets: &[(&str, &str, LogicalRect)]) -> RegionCheck {
    let takes = |rect: PxRect, whole: bool| match region {
        InputRegion::Everything => true,
        InputRegion::Rects(r) if whole => r.covers(rect),
        InputRegion::Rects(r) => r.touches(rect),
    };
    let bars: Vec<PxRect> = targets
        .iter()
        .filter(|t| t.0 == "bar")
        .map(|t| logical(t.2))
        .collect();
    let empty: Vec<PxRect> = targets
        .iter()
        .filter(|t| t.0 == "empty")
        .map(|t| logical(t.2))
        .collect();
    RegionCheck {
        bars_covered: bars.iter().filter(|r| takes(**r, true)).count(),
        bars: bars.len(),
        empty_clear: empty.iter().filter(|r| !takes(**r, false)).count(),
        empty: empty.len(),
        rects: match region {
            InputRegion::Everything => 0,
            InputRegion::Rects(r) => r.rect_count(),
        },
    }
}

#[derive(Default, Debug, PartialEq)]
pub struct ClickTally {
    pub bar_on_top: u32,
    pub bar_elsewhere: u32,
    pub empty_on_catcher: u32,
    pub empty_elsewhere: u32,
    pub outside_targets: u32,
}

pub fn tally<'a>(
    presses: impl IntoIterator<Item = (Option<&'a str>, f64, f64)>,
    targets: &[(&str, &str, LogicalRect)],
) -> ClickTally {
    let mut tally = ClickTally::default();
    for (role, x, y) in presses {
        match targets.iter().find(|t| t.2.contains(x, y)).map(|t| t.0) {
            Some("bar") if role == Some("top") => tally.bar_on_top += 1,
            Some("bar") => tally.bar_elsewhere += 1,
            Some("empty") if role == Some("catcher") => tally.empty_on_catcher += 1,
            Some("empty") => tally.empty_elsewhere += 1,
            _ => tally.outside_targets += 1,
        }
    }
    tally
}

/// Criterion 8, in two halves: the clicks a user made and where the compositor delivered them, and — deterministic, and available where nobody can click — the input region the merged window declared.
pub fn clicks(merged: &Run) -> Vec<Verdict> {
    const WANTED: u32 = 20;
    let targets = merged.targets();
    let span = merged.scenario("clicks");
    let mut verdicts = Vec::new();

    let clicked = match span {
        None => Verdict::new(
            "8a",
            "20/20 bar clicks stay on the Top window, 20/20 empty-space clicks go through",
            Status::NotMeasured,
            "no click test in this phase",
        ),
        Some(span) => {
            let protocol: Option<ClickTally> = merged.trace.as_ref().map(|trace| {
                let presses = trace
                    .presses
                    .iter()
                    .filter(|p| merged.time(p.stamp).is_some_and(|t| span.contains(t)))
                    .map(|p| (p.surface.and_then(|s| merged.role(s)), p.x, p.y));
                tally(presses, &targets)
            });
            let in_tree = tally(
                merged
                    .presses(span)
                    .into_iter()
                    .map(|(role, x, y)| (Some(role), x, y)),
                &targets,
            );
            let judged = protocol.as_ref().unwrap_or(&in_tree);
            let bar = judged.bar_on_top + judged.bar_elsewhere;
            let empty = judged.empty_on_catcher + judged.empty_elsewhere;
            let status = if judged.bar_elsewhere > 0 || judged.empty_elsewhere > 0 {
                Status::Fail
            } else if bar >= WANTED && empty >= WANTED {
                Status::Pass
            } else {
                Status::Incomplete
            };
            let mut verdict = Verdict::new(
                "8a",
                "20/20 bar clicks stay on the Top window, 20/20 empty-space clicks go through",
                status,
                format!(
                    "bar background: {}/{bar} delivered to the Top window · empty space: {}/{empty} delivered to the Bottom catcher",
                    judged.bar_on_top, judged.empty_on_catcher
                ),
            )
            .note(format!(
                "counted from {}; {} press(es) landed outside every target and are not counted",
                if protocol.is_some() {
                    "the wl_pointer.button events the compositor sent, by the surface the pointer had entered"
                } else {
                    "what each surface's tree received (no protocol log)"
                },
                judged.outside_targets
            ));
            if let Some(protocol) = &protocol
                && *protocol != in_tree
            {
                verdict = verdict.note(format!(
                    "the trees' own count differs: bar {}/{}, empty {}/{}",
                    in_tree.bar_on_top,
                    in_tree.bar_on_top + in_tree.bar_elsewhere,
                    in_tree.empty_on_catcher,
                    in_tree.empty_on_catcher + in_tree.empty_elsewhere
                ));
            }
            verdict
        }
    };
    verdicts.push(clicked);

    const REGION: &str = "declared input region covers the bars and not empty space";
    let reference = span
        .map(|s| s.start)
        .or_else(|| merged.timeline.last().map(|e| e.at_us));
    let region = reference.and_then(|at| region_at(merged, at));
    let declared = match (region, targets.is_empty()) {
        (_, true) => Verdict::new(
            "8b",
            REGION,
            Status::NotMeasured,
            "no click targets were recorded",
        ),
        (None, false) => Verdict::new(
            "8b",
            REGION,
            Status::NotMeasured,
            "the merged window never committed an input region the log shows",
        ),
        (Some(region), false) => {
            let check = check_region(&region, &targets);
            let pass = check.bars_covered == check.bars && check.empty_clear == check.empty;
            let mut verdict = Verdict::new(
                "8b",
                REGION,
                if pass { Status::Pass } else { Status::Fail },
                format!(
                    "{}/{} bar targets inside, {}/{} empty targets outside the region in effect {} ({})",
                    check.bars_covered,
                    check.bars,
                    check.empty_clear,
                    check.empty,
                    if span.is_some() {
                        "when the click test began"
                    } else {
                        "at the end of the run"
                    },
                    match region {
                        InputRegion::Everything => "the whole surface".to_owned(),
                        InputRegion::Rects(_) => format!("{} rects", check.rects),
                    }
                ),
            );
            if let Some(idle) = merged.scenario("idle") {
                let during = region_at(merged, (idle.start + idle.end) / 2);
                verdict = verdict.note(match during {
                    None => "during idle no input region had been committed yet — the surface still had the empty region it was created with, so the bars were click-through until a later frame carried one".to_owned(),
                    Some(r) => {
                        let c = check_region(&r, &targets);
                        format!(
                            "during idle: {}/{} bar targets inside, {}/{} empty targets outside",
                            c.bars_covered, c.bars, c.empty_clear, c.empty
                        )
                    }
                });
            }
            verdict
        }
    };
    verdicts.push(declared);
    verdicts
}

pub fn exactness() -> Verdict {
    Verdict::new(
        "–",
        "software exactness A/B (masked vs unmasked draws)",
        Status::OutOfScope,
        "the software renderer has no unmasked fast path to switch to, so there is no B to measure",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timeline::Record;

    const DAY_START: u64 = 1_789_689_600_000_000;

    fn at(h: u64, m: u64, s: u64, us: u64) -> u64 {
        DAY_START + ((h * 60 + m) * 60 + s) * 1_000_000 + us
    }

    fn entry(at_us: u64, record: Record) -> Entry {
        Entry { at_us, record }
    }

    fn scenario(name: &str, start: u64, end: u64) -> [Entry; 2] {
        [
            entry(
                start,
                Record::Scenario {
                    name: name.into(),
                    start: true,
                },
            ),
            entry(
                end,
                Record::Scenario {
                    name: name.into(),
                    start: false,
                },
            ),
        ]
    }

    /// The merged window, a 1920×1080 buffer, and one frame per line of damage given.
    fn log(frames: &[(&str, &str)]) -> String {
        let mut log = String::from(
            "[10:00:00.000100]  -> wl_compositor#4.create_surface(new id wl_surface#25)
[10:00:00.000200]  -> zwlr_layer_shell_v1#9.get_layer_surface(new id zwlr_layer_surface_v1#26, wl_surface#25, nil, 2, \"hogar-shell-spike-top\")
[10:00:00.010000] {Default Queue} zwlr_layer_surface_v1#26.configure(1, 1920, 1080)
[10:00:00.020100]  -> wl_shm_pool#40.create_buffer(new id wl_buffer#41, 0, 1920, 1080, 7680, 0)
",
        );
        for (stamp, damage) in frames {
            log.push_str(&format!(
                "[{stamp}]  -> wl_surface#25.attach(wl_buffer#41, 0, 0)\n[{stamp}]  -> wl_surface#25.damage_buffer({damage})\n[{stamp}]  -> wl_surface#25.commit()\n"
            ));
        }
        log
    }

    fn run(timeline: Vec<Entry>, frames: &[(&str, &str)]) -> Run {
        Run::new(timeline, Some(trace::replay(&log(frames))))
    }

    fn clock_event(at_us: u64, index: u32) -> Entry {
        entry(
            at_us,
            Record::Event {
                scenario: "clock".into(),
                index,
                role: "top".into(),
                expected: vec![LogicalRect::new(900.0, 4.0, 104.0, 28.0)],
            },
        )
    }

    #[test]
    fn a_commit_is_placed_on_the_timeline_by_its_time_of_day() {
        let run = Run::new(
            vec![entry(
                at(10, 0, 0, 0),
                Record::Meta {
                    key: "mode".into(),
                    value: "merged".into(),
                },
            )],
            None,
        );
        assert_eq!(
            run.time(Stamp::TimeOfDay(36_000_000_000)),
            Some(at(10, 0, 0, 0))
        );
        assert_eq!(run.time(Stamp::Relative(5)), None);
        let late = Run::new(
            vec![entry(
                at(23, 59, 59, 0),
                Record::Meta {
                    key: "mode".into(),
                    value: "merged".into(),
                },
            )],
            None,
        );
        assert_eq!(
            late.time(Stamp::TimeOfDay(1_000_000)),
            Some(DAY_START + DAY_US + 1_000_000),
            "a line printed just after midnight belongs to the next day"
        );
    }

    #[test]
    fn a_clock_tick_inside_its_chip_passes() {
        let mut timeline = scenario("clock", at(10, 0, 1, 0), at(10, 0, 3, 0)).to_vec();
        timeline.push(clock_event(at(10, 0, 1, 500_000), 0));
        timeline.push(clock_event(at(10, 0, 2, 500_000), 1));
        timeline.sort_by_key(|e| e.at_us);
        let run = run(
            timeline,
            &[
                ("10:00:01.510000", "920, 8, 60, 20"),
                ("10:00:02.510000", "920, 8, 60, 20"),
            ],
        );
        let verdict = clock_ticks(&run);
        assert_eq!(verdict.status, Status::Pass, "{}", verdict.measured);
        assert!(verdict.notes.iter().any(|n| n.contains("stayed inside")));
    }

    #[test]
    fn a_whole_frame_on_a_tick_fails_both_the_area_and_the_full_frame_criteria() {
        let mut timeline = scenario("clock", at(10, 0, 1, 0), at(10, 0, 3, 0)).to_vec();
        timeline.push(clock_event(at(10, 0, 1, 500_000), 0));
        timeline.sort_by_key(|e| e.at_us);
        let run = run(timeline, &[("10:00:01.510000", "0, 0, 1920, 1080")]);
        assert_eq!(clock_ticks(&run).status, Status::Fail);
        let verdict = full_frames(&run);
        assert_eq!(verdict.status, Status::Fail);
        assert!(verdict.measured.starts_with("1 full-surface frames of 1"));
    }

    #[test]
    fn damage_beyond_the_tolerance_is_excess() {
        let mut timeline = scenario("clip", at(10, 0, 1, 0), at(10, 0, 3, 0)).to_vec();
        timeline.push(entry(
            at(10, 0, 1, 500_000),
            Record::Event {
                scenario: "clip".into(),
                index: 0,
                role: "top".into(),
                expected: vec![
                    LogicalRect::new(100.0, 1048.0, 40.0, 28.0),
                    LogicalRect::new(100.0, 1048.0, 300.0, 28.0),
                ],
            },
        ));
        timeline.sort_by_key(|e| e.at_us);
        let inside = run(
            timeline.clone(),
            &[("10:00:01.510000", "99, 1047, 303, 30")],
        );
        let verdict = clip_resizes(&inside);
        assert_eq!(verdict.status, Status::Pass, "{}", verdict.measured);

        let spilling = run(timeline, &[("10:00:01.510000", "100, 1048, 400, 28")]);
        let checks = check_events(&spilling, "clip").unwrap();
        assert_eq!(checks[0].excess, (400 - 302) * 28);
        assert_eq!(clip_resizes(&spilling).status, Status::Fail);
    }

    #[test]
    fn a_change_that_produced_no_frame_is_not_a_pass() {
        let mut timeline = scenario("notify", at(10, 0, 1, 0), at(10, 0, 3, 0)).to_vec();
        timeline.push(entry(
            at(10, 0, 1, 500_000),
            Record::Event {
                scenario: "notify".into(),
                index: 0,
                role: "top".into(),
                expected: vec![LogicalRect::new(1532.0, 44.0, 380.0, 60.0)],
            },
        ));
        timeline.sort_by_key(|e| e.at_us);
        let run = run(timeline, &[]);
        assert_eq!(arrivals(&run).status, Status::NotMeasured);
        let without_log = Run::new(Vec::new(), None);
        assert_eq!(arrivals(&without_log).status, Status::NotMeasured);
    }

    fn perf(at_us: u64, summary: &str) -> Entry {
        entry(
            at_us,
            Record::Perf {
                summary: summary.into(),
            },
        )
    }

    #[test]
    fn only_windows_filled_inside_a_scenario_count() {
        let mut timeline = scenario("drawer", at(10, 0, 10, 0), at(10, 0, 20, 0)).to_vec();
        timeline.extend([
            perf(at(10, 0, 9, 500_000), "interpret=9000/9000us(n60)"),
            perf(at(10, 0, 11, 0), "interpret=5000/5000us(n60)"),
            perf(at(10, 0, 12, 0), "interpret=400/900us(n60) plan=10/20us(n60) convert=90/100us(n60) frame=900/1500us(n60)"),
            perf(at(10, 0, 13, 0), "interpret=600/1100us(n60) plan=10/20us(n60) convert=110/120us(n60) frame=1100/1700us(n60)"),
            perf(at(10, 0, 21, 0), "interpret=7000/7000us(n60)"),
        ]);
        timeline.sort_by_key(|e| e.at_us);
        let run = Run::new(timeline, None);
        let windows = run.clean_perf("drawer");
        assert_eq!(
            windows.len(),
            2,
            "the window that began before the scenario and the one that ended after it are out"
        );
        let cost = frame_cost(&windows).unwrap();
        assert_eq!(cost.work_us, 500.0 + 10.0 + 100.0);
        assert_eq!(cost.frames, 120);
        assert_eq!(run.observed_backend(), Some("software"));
    }

    #[test]
    fn the_drawer_ratio_compares_renderer_work() {
        let build = |interpret: u32| {
            let mut timeline = scenario("drawer", at(10, 0, 10, 0), at(10, 0, 20, 0)).to_vec();
            timeline.extend([
                perf(at(10, 0, 11, 0), "interpret=1/1us(n1)"),
                perf(
                    at(10, 0, 12, 0),
                    &format!("interpret={interpret}/{interpret}us(n60) frame=2000/2000us(n60)"),
                ),
            ]);
            timeline.sort_by_key(|e| e.at_us);
            Run::new(timeline, None)
        };
        assert_eq!(drawer(&build(400), &build(700)).status, Status::Pass);
        assert_eq!(drawer(&build(400), &build(900)).status, Status::Fail);
        assert_eq!(
            drawer(&build(400), &Run::new(Vec::new(), None)).status,
            Status::NotMeasured
        );
    }

    fn rss(at_us: u64, label: &str, rss: u64, hwm: u64) -> Entry {
        entry(
            at_us,
            Record::Rss {
                label: label.into(),
                fields: vec![("VmRSS".into(), rss), ("VmHWM".into(), hwm)],
            },
        )
    }

    #[test]
    fn memory_must_hold_at_rest_and_at_the_peak() {
        let today = Run::new(
            vec![
                rss(1, "steady", 100_000, 100_000),
                rss(2, "end", 110_000, 120_000),
            ],
            None,
        );
        let fine = Run::new(
            vec![
                rss(1, "steady", 140_000, 140_000),
                rss(2, "end", 150_000, 170_000),
            ],
            None,
        );
        assert_eq!(memory(&today, &fine).status, Status::Pass);
        let spiky = Run::new(
            vec![
                rss(1, "steady", 140_000, 140_000),
                rss(2, "end", 150_000, 190_000),
            ],
            None,
        );
        assert_eq!(
            memory(&today, &spiky).status,
            Status::Fail,
            "a peak of 158% fails even though rest is 140%"
        );
    }

    #[test]
    fn clicks_are_tallied_by_target_and_by_the_surface_that_took_them() {
        let targets = [
            ("bar", "B1", LogicalRect::new(700.0, 4.0, 96.0, 28.0)),
            ("empty", "E1", LogicalRect::new(708.0, 450.0, 120.0, 72.0)),
        ];
        let presses = [
            (Some("top"), 710.0, 10.0),
            (Some("catcher"), 710.0, 10.0),
            (Some("catcher"), 720.0, 460.0),
            (Some("top"), 720.0, 460.0),
            (Some("catcher"), 5.0, 500.0),
        ];
        assert_eq!(
            tally(presses, &targets),
            ClickTally {
                bar_on_top: 1,
                bar_elsewhere: 1,
                empty_on_catcher: 1,
                empty_elsewhere: 1,
                outside_targets: 1,
            }
        );
    }

    #[test]
    fn the_declared_region_must_cover_bar_targets_and_miss_empty_ones() {
        let targets = [
            ("bar", "B1", LogicalRect::new(700.0, 4.0, 96.0, 28.0)),
            ("empty", "E1", LogicalRect::new(708.0, 450.0, 120.0, 72.0)),
        ];
        let bars = InputRegion::Rects(PxRegion::from_rects([
            PxRect::new(0, 0, 1920, 36),
            PxRect::new(0, 1044, 1920, 36),
        ]));
        let check = check_region(&bars, &targets);
        assert_eq!(
            (check.bars_covered, check.empty_clear, check.rects),
            (1, 1, 2)
        );
        let check = check_region(&InputRegion::Everything, &targets);
        assert_eq!(
            (check.bars_covered, check.empty_clear),
            (1, 0),
            "a surface taking input everywhere swallows empty-space clicks"
        );
        let check = check_region(&InputRegion::Rects(PxRegion::default()), &targets);
        assert_eq!(
            (check.bars_covered, check.empty_clear),
            (0, 1),
            "an empty region lets the bar clicks through too"
        );
    }

    #[test]
    fn a_frame_damaged_everywhere_in_surface_space_is_full() {
        let log = "\
[10:00:00.000100]  -> wl_compositor#4.create_surface(new id wl_surface#25)
[10:00:00.000300]  -> zwp_linux_buffer_params_v1#60.create_immed(new id wl_buffer#61, 3840, 2160, 875713089, 0)
[10:00:00.000400]  -> wl_surface#25.attach(wl_buffer#61, 0, 0)
[10:00:00.000500]  -> wl_surface#25.damage(0, 0, 2147483647, 2147483647)
[10:00:00.000600]  -> wl_surface#25.commit()
";
        let trace = trace::replay(log);
        let damage = frame_damage(&trace.commits[0]).unwrap();
        assert!(damage.full());
        assert_eq!(damage.area(), 3840 * 2160);
    }
}
