//! Runs the script of scenarios; each scenario is a span with exactly one kind of change happening, which is what lets the report attribute a protocol commit or perf window to it by time alone (`simultaneous` is the deliberate exception, recorded as one change covering all three).

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use platform_wayland::{Layer, LayerShellPlatform, LayerWindowHandle};
use telar::{App, AppPathsProvider};

use crate::probe;
use crate::scene::{
    Card, LONG_LABEL, Role, SHORT_LABEL, Scene, SurfaceApp, Target, TargetClass, Watch,
};
use crate::timeline::{Expected, LogicalRect, Record, Recorder, now_us};

pub struct RunArgs {
    pub out: PathBuf,
    /// The output to map on, by its `wl_output` name; the compositor's choice when absent.
    pub output: Option<String>,
    /// Whether to end the run by asking the user to click (only meaningful on an output they can see and reach).
    pub clicks: bool,
    pub note: Option<String>,
}

const WARMUP: u64 = 2500;
const IDLE: u64 = 3000;
const CLOCK_TICKS: u32 = 10;
const RATE_SPAN: u64 = 5000;
/// One change per display frame at 60 Hz, which is the most telar will present for a surface.
const RATE_PERIOD: u64 = 16;
const NOTIFY_COUNT: u32 = 6;
const CHURN_PERIOD: u64 = 20;
const CHURN_DEPTH: usize = 5;
const CLIP_COUNT: u32 = 6;
const SIMULTANEOUS_COUNT: u32 = 6;
const EVENT_GAP: u64 = 900;
/// How long after a change the layout is read back for the rects it should have repainted — several frames, so a slow frame cannot race the read.
const SETTLE: u64 = 250;
/// Long enough after mapping a surface for its first, necessarily whole, frame to be out of the way.
const MAP_SETTLE: u64 = 1500;
const DRAWER_SPAN: u64 = 8000;
const DRAWER_CYCLE: u64 = 700;
const DRAWER_HOLD: u64 = 350;
const CLICKS_WANTED: u32 = 20;
const CLICK_TIMEOUT: Duration = Duration::from_secs(300);

pub fn run(args: RunArgs) -> Result<(), String> {
    std::fs::create_dir_all(&args.out)
        .map_err(|e| format!("cannot create {}: {e}", args.out.display()))?;
    let path = args.out.join("timeline.tsv");
    let recorder =
        Recorder::create(&path).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    probe::install_tracing(recorder.clone());
    let compositor = probe::compositor_pid();
    record_context(&recorder, &args, compositor);

    let shutdown = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&shutdown);
    let started = recorder.clone();
    platform_wayland::run_on_start(move || Director::start(args, compositor, started, flag));
    let outcome = telar::run_multi_with_platform(
        LayerShellPlatform::new().with_shutdown(shutdown),
        Vec::new(),
        |_| Arc::new(telar::NoPaths) as Arc<dyn AppPathsProvider>,
        |_id| -> Box<dyn App> {
            unreachable!("the spike opens every surface at runtime, through platform_wayland")
        },
        "hogar-shell-spike",
    );
    #[cfg(feature = "hardware")]
    if let Some(gpu) = telar::gpu::shared() {
        let info = gpu.adapter.get_info();
        recorder.meta(
            "gpu-adapter",
            format!(
                "{} ({:?}, {:?}, driver {} {})",
                info.name, info.backend, info.device_type, info.driver, info.driver_info
            ),
        );
    }
    recorder.meta("finished", now_us());
    outcome.map_err(|e| format!("the platform stopped with an error: {e}"))
}

fn record_context(recorder: &Recorder, args: &RunArgs, compositor: Option<u32>) {
    recorder.meta(
        "telar-renderers",
        if cfg!(feature = "hardware") {
            "software+hardware"
        } else {
            "software"
        },
    );
    recorder.meta(
        "telar-renderer-backend-at-build",
        option_env!("TELAR_RENDERER_BACKEND").unwrap_or("unset"),
    );
    recorder.meta(
        "profile",
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        },
    );
    for key in [
        "WAYLAND_DISPLAY",
        "WAYLAND_DEBUG",
        "TELAR_PERF",
        "WGPU_POWER_PREF",
    ] {
        recorder.meta(
            &format!("env:{key}"),
            std::env::var(key).unwrap_or_else(|_| "unset".into()),
        );
    }
    recorder.meta(
        "output-requested",
        args.output.as_deref().unwrap_or("compositor's choice"),
    );
    if let Some(note) = &args.note {
        recorder.meta("note", note);
    }
    recorder.meta("pid", std::process::id());
    if let Some(pid) = compositor {
        recorder.meta("compositor-pid", pid);
        recorder.meta(
            "compositor-name",
            probe::process_name(pid).unwrap_or_default(),
        );
    }
    recorder.meta("cpu", probe::cpu_model().unwrap_or_default());
    recorder.meta("logical-cpus", probe::logical_cpus());
    if let Ok(release) = std::fs::read_to_string("/proc/sys/kernel/osrelease") {
        recorder.meta("kernel", release.trim());
    }
}

type Action = Box<dyn FnOnce(&Rc<Director>)>;
type Tick = dyn Fn(&Rc<Director>);

/// The scenario script: each step waits, then acts.
#[derive(Default)]
struct Script {
    steps: VecDeque<(u64, Action)>,
}

impl Script {
    fn then(&mut self, wait_ms: u64, action: impl FnOnce(&Rc<Director>) + 'static) {
        self.steps.push_back((wait_ms, Box::new(action)));
    }
}

fn play(director: Rc<Director>, mut steps: VecDeque<(u64, Action)>) {
    let Some((wait, action)) = steps.pop_front() else {
        return;
    };
    platform_wayland::timeout(Duration::from_millis(wait), move || {
        action(&director);
        play(director, steps);
    });
}

#[derive(Default)]
struct Tally {
    bar: u32,
    empty: u32,
}

struct Director {
    output: Option<String>,
    clicks: bool,
    recorder: Recorder,
    shutdown: Arc<AtomicBool>,
    compositor: Option<u32>,
    scene: Rc<Scene>,
    // Held because dropping a handle closes its window.
    window: RefCell<Option<LayerWindowHandle>>,
    catcher: RefCell<Option<LayerWindowHandle>>,
    targets: RefCell<Vec<Target>>,
    clicking: Cell<bool>,
    tally: RefCell<Tally>,
    seconds: Cell<u32>,
    next_card: Cell<u64>,
}

impl Director {
    fn start(
        args: RunArgs,
        compositor: Option<u32>,
        recorder: Recorder,
        shutdown: Arc<AtomicBool>,
    ) {
        let director = Rc::new(Director {
            output: args.output,
            clicks: args.clicks,
            compositor,
            recorder,
            shutdown,
            scene: Scene::new(clock_text(0)),
            window: RefCell::default(),
            catcher: RefCell::default(),
            targets: RefCell::default(),
            clicking: Cell::new(false),
            tally: RefCell::default(),
            seconds: Cell::new(0),
            next_card: Cell::new(1),
        });
        let outputs = platform_wayland::outputs();
        for output in &outputs {
            director.recorder.meta(
                "output",
                format!(
                    "{} logical {:?} scale {} at {:?}",
                    output.name.as_deref().unwrap_or("?"),
                    output.logical_size,
                    output.scale,
                    output.position
                ),
            );
        }
        if let Some(wanted) = &director.output
            && !outputs
                .iter()
                .any(|o| o.name.as_deref() == Some(wanted.as_str()))
        {
            director.recorder.meta(
                "error",
                format!("output {wanted} is not among the compositor's outputs"),
            );
            eprintln!("hogar-shell-spike: output {wanted} not found");
            director.stop();
            return;
        }
        let presses = Rc::downgrade(&director);
        director.scene.on_press(move |role, x, y| {
            if let Some(director) = presses.upgrade() {
                director.pressed(role, x, y);
            }
        });
        *director.window.borrow_mut() = Some(director.open_window(Layer::Top, Role::Top));
        director.recorder.meta("surfaces-opened", now_us());
        println!("hogar-shell-spike: run started");
        play(Rc::clone(&director), script().steps);
    }

    /// A window of the scene, opened the way the shell's layer windows are.
    fn open_window(&self, layer: Layer, role: Role) -> LayerWindowHandle {
        platform_wayland::open_layer_window(
            self.output.clone(),
            layer,
            role.namespace(),
            SurfaceApp {
                scene: Rc::clone(&self.scene),
                role,
            },
        )
    }

    fn stop(&self) {
        self.recorder.meta("stopped", now_us());
        self.shutdown.store(true, Ordering::Relaxed);
    }

    fn scenario(&self, name: &str, start: bool) {
        self.recorder.record(Record::Scenario {
            name: name.to_owned(),
            start,
        });
        self.sample_cpu(&format!("{name}:{}", if start { "start" } else { "end" }));
        if start {
            println!("hogar-shell-spike: {name}");
        }
    }

    fn sample_cpu(&self, label: &str) {
        self.recorder.record(Record::Cpu {
            label: label.to_owned(),
            process_us: probe::process_cpu_us(),
            compositor_us: self.compositor.and_then(probe::task_cpu_us),
        });
    }

    fn sample_memory(&self, label: &str) {
        self.recorder.record(Record::Rss {
            label: label.to_owned(),
            fields: probe::memory(),
        });
    }

    /// Makes one discrete change to every piece in `watches`, in the same turn so it lands in one frame, and once the layout has run records it with the rects it should have repainted.
    fn change(
        self: &Rc<Self>,
        scenario: &'static str,
        index: u32,
        watches: &'static [Watch],
        apply: impl FnOnce(&Scene),
    ) {
        let at = now_us();
        let before: Vec<_> = watches.iter().map(|w| self.scene.snapshot(*w)).collect();
        apply(&self.scene);
        let director = Rc::clone(self);
        platform_wayland::timeout(Duration::from_millis(SETTLE), move || {
            let mut expected = Expected::default();
            for (watch, before) in watches.iter().zip(&before) {
                let after = director.scene.snapshot(*watch);
                expected.extend(expected_repaint(*watch, before, &after));
            }
            director.recorder.record_at(
                at,
                Record::Event {
                    scenario: scenario.to_owned(),
                    index,
                    role: Role::Top.name().to_owned(),
                    expected,
                },
            );
        });
    }

    /// Repeats `tick` every `period` for `span`, one timer at a time so nothing is left armed once it is over.
    fn repeat(self: &Rc<Self>, period: u64, span: u64, tick: impl Fn(&Rc<Director>) + 'static) {
        fn next(director: Rc<Director>, period: u64, deadline: Instant, tick: Rc<Tick>) {
            platform_wayland::timeout(Duration::from_millis(period), move || {
                if Instant::now() >= deadline {
                    return;
                }
                tick(&director);
                next(director, period, deadline, tick);
            });
        }
        let deadline = Instant::now() + Duration::from_millis(span);
        next(Rc::clone(self), period, deadline, Rc::new(tick));
    }

    fn advance_clock(&self) {
        self.scene.clock.set(self.next_second());
    }

    fn next_second(&self) -> String {
        self.seconds.set(self.seconds.get() + 1);
        clock_text(self.seconds.get())
    }

    fn tick_clock(self: &Rc<Self>, index: u32) {
        let text = self.next_second();
        self.change("clock", index, &[Watch::Clock], move |scene| {
            scene.clock.set(text)
        });
    }

    fn take_card(&self) -> Card {
        let id = self.next_card.get();
        self.next_card.set(id + 1);
        Card { id }
    }

    fn notify(self: &Rc<Self>, index: u32) {
        let card = self.take_card();
        self.change("notify", index, &[Watch::Cards], move |scene| {
            scene.cards.update(|cards| cards.insert(0, card));
        });
    }

    fn churn(&self) {
        let card = self.take_card();
        self.scene.cards.update(|cards| {
            cards.insert(0, card);
            cards.truncate(CHURN_DEPTH);
        });
    }

    fn close_stack(&self) {
        self.scene.cards.set(Vec::new());
        self.scene.forget_cards();
    }

    fn toggle_wide(self: &Rc<Self>, index: u32) {
        let text = wide_label(index);
        self.change("clip", index, &[Watch::Wide], move |scene| {
            scene.wide.set(text.to_owned())
        });
    }

    /// A clock tick, a card arrival and a clip resize in one turn: three changes far apart on the screen, which one window has to damage as three regions rather than one rect spanning them.
    fn simultaneous(self: &Rc<Self>, index: u32) {
        let text = self.next_second();
        let card = self.take_card();
        let label = wide_label(index);
        self.change(
            "simultaneous",
            index,
            &[Watch::Clock, Watch::Cards, Watch::Wide],
            move |scene| {
                scene.clock.set(text);
                scene.cards.update(|cards| cards.insert(0, card));
                scene.wide.set(label.to_owned());
            },
        );
    }

    fn flip_wide(&self) {
        let next = if self.scene.wide.peek() == SHORT_LABEL {
            LONG_LABEL
        } else {
            SHORT_LABEL
        };
        self.scene.wide.set(next.to_owned());
    }

    fn open_drawer(&self) {
        self.scene.drawer_slot.set(vec![0]);
    }

    fn close_drawer(&self) {
        self.scene.drawer_slot.set(Vec::new());
    }

    /// Opens and closes the drawer on a fixed cycle: 200 ms in, a hold, 200 ms out, a pause, the same change every frame so its frame cost can be read.
    fn animate_drawer(self: &Rc<Self>) {
        self.repeat(DRAWER_CYCLE, DRAWER_SPAN - DRAWER_CYCLE, |director| {
            director.scene.drawer.retarget(1.0);
            let closing = Rc::clone(director);
            platform_wayland::timeout(Duration::from_millis(DRAWER_HOLD), move || {
                closing.scene.drawer.retarget(0.0);
            });
        });
    }

    fn record_targets(&self) {
        let targets = self.scene.click_targets();
        for target in &targets {
            self.recorder.record(Record::Target {
                class: target.class.name().to_owned(),
                id: target.id.clone(),
                rect: target.rect,
            });
        }
        *self.targets.borrow_mut() = targets;
    }

    fn after_measurement(self: &Rc<Self>) {
        if self.clicks {
            self.ask_for_clicks();
        } else {
            self.stop();
        }
    }

    fn ask_for_clicks(self: &Rc<Self>) {
        self.scenario("clicks", true);
        *self.catcher.borrow_mut() = Some(self.open_window(Layer::Bottom, Role::Catcher));
        self.scene.targets.set(self.targets.borrow().clone());
        self.clicking.set(true);
        self.show_tally();
        println!(
            "hogar-shell-spike: click test — on an empty workspace, click the yellow B targets (bar background) {CLICKS_WANTED} times in total and the green E targets (empty space) {CLICKS_WANTED} times in total; it ends by itself once both counts are reached, or after {} s",
            CLICK_TIMEOUT.as_secs()
        );
        let director = Rc::clone(self);
        platform_wayland::timeout(CLICK_TIMEOUT, move || director.finish_clicks());
    }

    fn show_tally(&self) {
        let tally = self.tally.borrow();
        self.scene.status.set(format!(
            "click B (bar background) {}/{CLICKS_WANTED}   ·   click E (empty space) {}/{CLICKS_WANTED}",
            tally.bar.min(CLICKS_WANTED),
            tally.empty.min(CLICKS_WANTED)
        ));
    }

    fn pressed(self: &Rc<Self>, role: Role, x: f64, y: f64) {
        self.recorder.record(Record::Press {
            role: role.name().to_owned(),
            x,
            y,
        });
        if !self.clicking.get() {
            return;
        }
        let class = self
            .targets
            .borrow()
            .iter()
            .find(|t| t.rect.contains(x, y))
            .map(|t| t.class);
        {
            let mut tally = self.tally.borrow_mut();
            match class {
                Some(TargetClass::Bar) => tally.bar += 1,
                Some(TargetClass::Empty) => tally.empty += 1,
                None => {}
            }
        }
        self.show_tally();
        let tally = self.tally.borrow();
        if tally.bar >= CLICKS_WANTED && tally.empty >= CLICKS_WANTED {
            let director = Rc::clone(self);
            platform_wayland::timeout(Duration::from_millis(500), move || director.finish_clicks());
        }
    }

    fn finish_clicks(&self) {
        if !self.clicking.replace(false) {
            return;
        }
        self.scene.targets.set(Vec::new());
        self.scene.status.set(String::new());
        if let Some(catcher) = self.catcher.borrow_mut().take() {
            catcher.close();
        }
        self.scenario("clicks", false);
        self.stop();
    }
}

fn clock_text(seconds: u32) -> String {
    let total = 9 * 3600 + 41 * 60 + seconds;
    format!(
        "{:02}:{:02}:{:02}",
        total / 3600 % 24,
        total / 60 % 60,
        total % 60
    )
}

/// What a keyed change should repaint: every rect that appeared or went away, and both the old and the new place of every one that moved. One that stayed exactly where it was is left out — repainting it would be the excess the check is looking for.
fn moved(before: &[(u64, LogicalRect)], after: &[(u64, LogicalRect)]) -> Vec<LogicalRect> {
    let mut rects = Vec::new();
    for (key, now) in after {
        match before.iter().find(|(k, _)| k == key) {
            Some((_, then)) if then == now => {}
            Some((_, then)) => rects.extend([*then, *now]),
            None => rects.push(*now),
        }
    }
    for (key, then) in before {
        if !after.iter().any(|(k, _)| k == key) {
            rects.push(*then);
        }
    }
    rects
}

/// The label the widening chip shows after the `index`th clip change: long on even ones, short on odd ones, so each change resizes its clip.
fn wide_label(index: u32) -> &'static str {
    if index.is_multiple_of(2) {
        LONG_LABEL
    } else {
        SHORT_LABEL
    }
}

/// What a change to `watch` should repaint: whatever the layout moved or added, before and after, in full — or, for the clock, whose box never moves, somewhere inside the chip whose digits changed.
fn expected_repaint(
    watch: Watch,
    before: &[(u64, LogicalRect)],
    after: &[(u64, LogicalRect)],
) -> Expected {
    match watch {
        Watch::Clock => Expected::bound(after.iter().map(|(_, r)| *r).collect()),
        Watch::Wide | Watch::Cards => Expected::cover(moved(before, after)),
    }
}

fn script() -> Script {
    let mut s = Script::default();
    s.then(WARMUP, |d| {
        d.record_targets();
        d.scenario("idle", true);
    });
    s.then(IDLE, |d| {
        d.scenario("idle", false);
        d.sample_memory("steady");
        d.scenario("clock", true);
    });
    for i in 0..CLOCK_TICKS {
        s.then(1000, move |d| d.tick_clock(i));
    }
    s.then(1000, |d| {
        d.scenario("clock", false);
        d.scenario("clock-rate", true);
        d.repeat(RATE_PERIOD, RATE_SPAN, |d| d.advance_clock());
    });
    s.then(RATE_SPAN + 200, |d| {
        d.scenario("clock-rate", false);
    });
    s.then(MAP_SETTLE, |d| d.scenario("notify", true));
    for i in 0..NOTIFY_COUNT {
        s.then(if i == 0 { 300 } else { EVENT_GAP }, move |d| d.notify(i));
    }
    s.then(EVENT_GAP, |d| {
        d.scenario("notify", false);
        d.scenario("notify-rate", true);
        d.repeat(CHURN_PERIOD, RATE_SPAN, |d| d.churn());
    });
    s.then(RATE_SPAN + 200, |d| {
        d.scenario("notify-rate", false);
        d.close_stack();
    });
    s.then(1000, |d| d.scenario("clip", true));
    for i in 0..CLIP_COUNT {
        s.then(if i == 0 { 300 } else { EVENT_GAP }, move |d| {
            d.toggle_wide(i)
        });
    }
    s.then(EVENT_GAP, |d| {
        d.scenario("clip", false);
        d.scenario("clip-rate", true);
        d.repeat(RATE_PERIOD, RATE_SPAN, |d| d.flip_wide());
    });
    s.then(RATE_SPAN + 200, |d| {
        d.scenario("clip-rate", false);
        d.scene.wide.set(SHORT_LABEL.to_owned());
    });
    s.then(MAP_SETTLE, |d| d.scenario("simultaneous", true));
    for i in 0..SIMULTANEOUS_COUNT {
        s.then(if i == 0 { 300 } else { EVENT_GAP }, move |d| {
            d.simultaneous(i)
        });
    }
    s.then(EVENT_GAP, |d| {
        d.scenario("simultaneous", false);
        d.close_stack();
        d.scene.wide.set(SHORT_LABEL.to_owned());
    });
    s.then(1000, |d| d.open_drawer());
    s.then(MAP_SETTLE, |d| {
        d.scenario("drawer", true);
        d.animate_drawer();
    });
    s.then(DRAWER_SPAN / 2, |d| d.sample_memory("drawer"));
    s.then(DRAWER_SPAN / 2 + 300, |d| {
        d.scenario("drawer", false);
        d.close_drawer();
    });
    s.then(1000, |d| {
        d.sample_memory("end");
        d.after_measurement();
    });
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(x: f64, y: f64) -> LogicalRect {
        LogicalRect::new(x, y, 380.0, 60.0)
    }

    #[test]
    fn an_arrival_repaints_the_new_card_and_both_places_of_every_card_it_moves() {
        let before = vec![(1, r(0.0, 44.0)), (2, r(0.0, 112.0))];
        let after = vec![(1, r(0.0, 112.0)), (2, r(0.0, 180.0)), (3, r(0.0, 44.0))];
        let rects = moved(&before, &after);
        assert_eq!(rects.len(), 5);
        assert!(rects.contains(&r(0.0, 44.0)));
        assert!(rects.contains(&r(0.0, 180.0)));
    }

    #[test]
    fn a_card_that_did_not_move_is_not_expected_to_repaint() {
        let before = vec![(1, r(0.0, 44.0))];
        let after = vec![(1, r(0.0, 44.0)), (2, r(0.0, 112.0))];
        assert_eq!(moved(&before, &after), vec![r(0.0, 112.0)]);
    }

    #[test]
    fn a_clock_tick_is_bounded_by_its_chip_though_the_chip_did_not_move() {
        let chip = vec![(0, r(900.0, 4.0))];
        assert_eq!(
            expected_repaint(Watch::Clock, &chip, &chip),
            Expected::bound(vec![r(900.0, 4.0)]),
            "the chip bounds the tick's damage, which need not fill it"
        );
        assert!(
            expected_repaint(Watch::Wide, &chip, &chip) == Expected::default(),
            "a clip that kept its size and place repaints nothing"
        );
    }

    #[test]
    fn every_clip_change_resizes_the_clip() {
        assert_eq!(wide_label(0), LONG_LABEL);
        assert_eq!(wide_label(1), SHORT_LABEL);
        assert_eq!(wide_label(2), LONG_LABEL);
    }

    #[test]
    fn the_clock_reads_like_a_clock() {
        assert_eq!(clock_text(0), "09:41:00");
        assert_eq!(clock_text(61), "09:42:01");
    }
}
