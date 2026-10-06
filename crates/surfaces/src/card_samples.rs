//! Sample cards and a sample launcher, drawn where the real ones would land and never sent anywhere: the editor's way of showing what a stack's routes do. The cards are drawn by whatever draws them for the caller, so they look as the real ones do. They live in a store of their own, apart from the notification daemon, its history and the toaster, so nothing that listens to those ever hears of one.

use std::cell::Cell;
use std::time::{Duration, Instant};

use telar::{
    AlignItems, Border, Container, JustifyContent, LayoutError, LayoutItem, LayoutStyle,
    ReactiveList, ReadSignal, Rect, RectStyle, RwSignal, SizeDimension, StyledContainer, Text,
    box_item, detached, signal, use_theme,
};

use config::Config;
use config::theme::{FontRole, NordTheme};
use layout::{Anchor, CardKind, RoutedCard, Urgency};
use ui::chrome::{card_gap, content_radius, panel_fill};
use ui::descriptor::Built;
use ui::scale::space;

use crate::pinned::{self, Side};

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

/// A stack's column of samples, laid out where `place` says the column is — its box and the anchor it is pinned to, read again whenever they change — with what `shown` lists in it, in that order, each sample drawn by `draw`. It paints nothing of its own and takes nothing from the pointer.
pub fn column(
    shown: impl Fn() -> Vec<Shown> + 'static,
    place: impl Fn() -> Option<(Rect, Anchor)> + 'static,
    draw: impl Fn(&Sample) -> Built + 'static,
) -> Built {
    let theme = use_theme::<NordTheme>();
    let radius = content_radius();
    let list = ReactiveList::with_style(
        LayoutStyle::new().flex_column().gap(card_gap()),
        shown,
        Shown::key,
        move |item: Shown| match item {
            Shown::Card(sample) => draw(&sample),
            Shown::Launcher(launcher) => launcher_card(&launcher, theme, radius),
        },
    )?;
    Ok(Box::new(
        StyledContainer::new(
            LayoutStyle::new(),
            |_| RectStyle::default(),
            vec![Box::new(list)],
        )?
        .input_transparent()
        .styled_by(move || {
            let Some((column, anchor)) = place() else {
                return LayoutStyle::new().absolute().width(0.0).height(0.0);
            };
            let justify = match pinned::sides(anchor).1 {
                Side::Start => JustifyContent::START,
                Side::Middle => JustifyContent::CENTER,
                Side::End => JustifyContent::END,
            };
            crate::area::at(column)
                .flex_column()
                .justify_content(justify)
        }),
    ))
}

/// The sample launcher in the middle of `bounds`, where no stack says it opens.
pub fn centred_launcher(
    launcher: Launcher,
    bounds: impl Fn() -> Rect + 'static,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let theme = use_theme::<NordTheme>();
    Ok(Box::new(
        StyledContainer::new(
            LayoutStyle::new(),
            |_| RectStyle::default(),
            vec![launcher_card(&launcher, theme, content_radius())?],
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

fn text(
    said: String,
    theme: NordTheme,
    role: FontRole,
    tint: fn(&NordTheme) -> telar::Color,
    weight: u16,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    Ok(box_item(Text::new(
        move || said.clone(),
        LayoutStyle::new(),
        move || {
            theme
                .text_style(role, tint(&theme))
                .with_font_weight(weight)
                .with_clamp(2, true)
        },
    )?))
}

fn launcher_card(
    launcher: &Launcher,
    theme: NordTheme,
    radius: f32,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let search = StyledContainer::new(
        LayoutStyle::new()
            .padding_all(space::lg())
            .width(SizeDimension::Percent(1.0)),
        move |_| RectStyle::filled(theme.overlay, radius),
        vec![text(
            launcher.search.clone(),
            theme,
            FontRole::Body,
            |theme| theme.subtle,
            400,
        )?],
    )?;
    let mut rows: Vec<Box<dyn LayoutItem>> = vec![Box::new(search)];
    for app in &launcher.apps {
        rows.push(text(
            app.clone(),
            theme,
            FontRole::Body,
            |theme| theme.text,
            400,
        )?);
    }
    Ok(Box::new(
        Container::new(
            LayoutStyle::new()
                .flex_column()
                .gap(space::sm())
                .padding_all(space::xl())
                .width(SizeDimension::Percent(1.0)),
            rows,
        )
        .and_then(|inner| {
            StyledContainer::new(
                LayoutStyle::new().width(SizeDimension::Percent(1.0)),
                move |_| {
                    RectStyle::filled(panel_fill(), radius)
                        .with_border(Border::uniform(theme.highlight_med, 1.0))
                },
                vec![Box::new(inner)],
            )
        })?,
    ))
}
