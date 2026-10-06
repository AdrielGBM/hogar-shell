//! Sample cards and a sample launcher, drawn where the real ones would land and never sent anywhere: the editor's way of showing what a stack's routes do. The cards are drawn by whatever draws them for the caller, so they look as the real ones do. They live in a store of their own, apart from the notification daemon, its history and the toaster, so nothing that listens to those ever hears of one.

use std::cell::Cell;
use std::time::{Duration, Instant};

use telar::{
    AlignItems, JustifyContent, LayoutStyle, ReactiveList, ReadSignal, Rect, RectStyle, RwSignal,
    StyledContainer, detached, signal,
};

use config::Config;
use layout::{Anchor, CardKind, RoutedCard, StackFlow, Urgency};
use ui::chrome::card_gap;
use ui::descriptor::Built;

use crate::pinned;

/// How long the real card `sample` stands for stays up under `config`: whatever the column holds goes after `[stack] timeout_ms`, but a critical notification under `[notifications] critical_sticky` waits to be dealt with, up to `critical_max_secs` (`None`, for as long as the mode is up, where that is `0`).
pub fn lifetime(sample: &Sample, config: &Config) -> Option<Duration> {
    let sticky = sample.kind == CardKind::Notification
        && sample.urgency == Some(Urgency::Critical)
        && config.notifications.critical_sticky;
    match sticky {
        true => config.notifications.critical_ceiling(),
        false => Some(config.stack.lifetime()),
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Sample {
    pub id: u64,
    pub kind: CardKind,
    pub app: Option<String>,
    pub urgency: Option<Urgency>,
    pub icon: String,
    pub title: String,
    pub body: String,
    pub level: Option<u8>,
    deadline: Option<Instant>,
}

impl Sample {
    fn bare(kind: CardKind, title: String) -> Self {
        Self {
            id: 0,
            kind,
            app: None,
            urgency: None,
            icon: String::new(),
            title,
            body: String::new(),
            level: None,
            deadline: None,
        }
    }

    pub fn notification(app: &str, urgency: Urgency, title: String, body: String) -> Self {
        Self {
            app: Some(app.to_string()),
            urgency: Some(urgency),
            body,
            ..Self::bare(CardKind::Notification, title)
        }
    }

    pub fn toast(icon: &str, title: String, body: String) -> Self {
        Self {
            icon: icon.to_string(),
            body,
            ..Self::bare(CardKind::Toast, title)
        }
    }

    pub fn osd(level: u8) -> Self {
        Self {
            level: Some(level.min(100)),
            ..Self::bare(CardKind::Osd, String::new())
        }
    }

    /// What a stack's routes are matched against, as for the real card of the same kind.
    pub fn routed(&self) -> RoutedCard<'_> {
        RoutedCard {
            kind: self.kind,
            app: self.app.as_deref(),
            urgency: self.urgency,
        }
    }
}

/// A sample launcher: its search line and the applications it lists.
#[derive(Clone, Debug, PartialEq)]
pub struct Launcher {
    pub search: String,
    pub apps: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Shown {
    Launcher(Launcher),
    Card(Sample),
}

impl Shown {
    fn key(&self) -> (bool, u64) {
        match self {
            Shown::Launcher(_) => (true, 0),
            Shown::Card(sample) => (false, sample.id),
        }
    }
}

thread_local! {
    static SAMPLES: RwSignal<Vec<Sample>> = detached(|| signal(Vec::new()));
    static LAUNCHER: RwSignal<Option<Launcher>> = detached(|| signal(None));
    static NEXT: Cell<u64> = const { Cell::new(0) };
}

/// The samples up now, oldest first.
pub fn samples() -> ReadSignal<Vec<Sample>> {
    SAMPLES.with(|samples| samples.read_only())
}

pub fn launcher() -> ReadSignal<Option<Launcher>> {
    LAUNCHER.with(|launcher| launcher.read_only())
}

/// Puts `sample` up, taking it away again after `lifetime` where there is one.
pub fn push(sample: Sample, lifetime: Option<Duration>) {
    push_at(sample, lifetime, Instant::now());
    if let Some(lifetime) = lifetime {
        platform_wayland::timeout(lifetime, || sweep(Instant::now()));
    }
}

/// [`push`], as of `now` and without the timer: what [`sweep`] is then asked about.
pub fn push_at(mut sample: Sample, lifetime: Option<Duration>, now: Instant) {
    sample.id = NEXT.with(|next| {
        next.set(next.get() + 1);
        next.get()
    });
    sample.deadline = lifetime.map(|lifetime| now + lifetime);
    SAMPLES.with(|samples| {
        samples.update(|all| {
            if sample.kind == CardKind::Osd {
                all.retain(|held| held.kind != CardKind::Osd);
            }
            all.push(sample);
        })
    });
}

/// Takes away every sample whose time is up at `now`.
pub fn sweep(now: Instant) {
    SAMPLES.with(|samples| {
        if samples.peek().iter().any(|sample| expired(sample, now)) {
            samples.update(|all| all.retain(|sample| !expired(sample, now)));
        }
    });
}

fn expired(sample: &Sample, now: Instant) -> bool {
    sample.deadline.is_some_and(|deadline| deadline <= now)
}

pub fn toggle_launcher(launcher: Launcher) {
    LAUNCHER.with(|open| {
        let closed = open.peek().is_some();
        open.set((!closed).then_some(launcher));
    });
}

pub fn clear() {
    SAMPLES.with(|samples| {
        if !samples.peek().is_empty() {
            samples.set(Vec::new());
        }
    });
    LAUNCHER.with(|launcher| {
        if launcher.peek().is_some() {
            launcher.set(None);
        }
    });
}

/// A stack's lane of samples, laid out as `flow` says where `place` says the stack is — its lane and the anchor it is pinned to, read again whenever they change — with what `shown` lists in it, in that order, each sample drawn by `draw` and the launcher by `draw_launcher`. It paints nothing of its own and takes nothing from the pointer.
pub fn lane(
    shown: impl Fn() -> Vec<Shown> + 'static,
    place: impl Fn() -> Option<(Rect, Anchor)> + 'static,
    flow: StackFlow,
    draw: impl Fn(&Sample) -> Built + 'static,
    draw_launcher: impl Fn(&Launcher) -> Built + 'static,
) -> Built {
    let list = ReactiveList::with_style(
        pinned::flowing(LayoutStyle::new(), flow).gap(card_gap()),
        shown,
        Shown::key,
        move |item: Shown| match item {
            Shown::Card(sample) => draw(&sample),
            Shown::Launcher(launcher) => draw_launcher(&launcher),
        },
    )?;
    Ok(Box::new(
        StyledContainer::new(
            LayoutStyle::new(),
            |_| RectStyle::default(),
            vec![Box::new(list)],
        )?
        .input_transparent()
        .styled_by(move || match place() {
            Some((lane, anchor)) => pinned::lane_style(crate::area::at(lane), anchor, flow),
            None => LayoutStyle::new().absolute().width(0.0).height(0.0),
        }),
    ))
}

/// The sample launcher in the middle of `bounds`, where no stack says it opens.
pub fn centred_launcher(
    launcher: Launcher,
    bounds: impl Fn() -> Rect + 'static,
    draw_launcher: impl Fn(&Launcher) -> Built,
) -> Built {
    Ok(Box::new(
        StyledContainer::new(
            LayoutStyle::new(),
            |_| RectStyle::default(),
            vec![draw_launcher(&launcher)?],
        )?
        .input_transparent()
        .styled_by(move || {
            crate::area::at(bounds())
                .flex_column()
                .justify_content(JustifyContent::CENTER)
                .align_items(AlignItems::CENTER)
        }),
    ))
}
