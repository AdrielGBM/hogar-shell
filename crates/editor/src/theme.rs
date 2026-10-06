//! The theme popover: `[theme]` is config rather than layout, so it previews on every window while open, is written to `config.toml` when it closes and stays outside the layout's undo history.

use std::cell::{Cell, RefCell};
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;

use telar::{
    Children, Container, JustifyContent, LayoutItem, LayoutStyle, RwSignal, Text, box_item,
    detached, effect, memo, signal, use_theme,
};

use config::theme::{ACCENTS, FONT_SCALE_RANGE, FontRole, NordTheme, OPACITY_RANGE};
use config::{Config, ScaleConfig, ThemeConfig};
use platform_wayland::KeyboardMode;
use surfaces::reconcile;
use surfaces::transient::{self, Place, Spec};
use ui::chrome::Chrome;
use ui::descriptor::Built;
use ui::form::theme_tiles::theme_tiles;

use crate::config_popover::{self, documented};
use crate::host;
use crate::mode;
use crate::popover::rows::{self, Range, label};
use crate::session::EditError;

pub const ID: &str = "editor:theme";

const WIDTH: f32 = 400.0;

/// The keys of `[theme]` the popover sets.
#[derive(Clone, Debug, PartialEq)]
pub struct Look {
    pub name: String,
    pub accent: String,
    pub radius: Option<u32>,
    pub opacity: f32,
    pub font: f32,
}

impl Look {
    pub fn of(config: &Config) -> Self {
        Self {
            name: config.theme.name.clone(),
            accent: config.theme.accent.clone(),
            radius: config.theme.radius,
            opacity: config.theme.opacity,
            font: config.theme.scale.font,
        }
    }

    pub fn on(&self, config: &Config) -> Config {
        let mut config = config.clone();
        self.onto(&mut config.theme);
        config
    }

    fn onto(&self, theme: &mut ThemeConfig) {
        theme.name = self.name.clone();
        theme.accent = self.accent.clone();
        theme.radius = self.radius;
        theme.opacity = self.opacity;
        theme.scale.font = self.font;
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Controls {
    pub(crate) name: RwSignal<String>,
    pub(crate) accent: RwSignal<String>,
    pub(crate) radius: RwSignal<f32>,
    pub(crate) opacity: RwSignal<f32>,
    pub(crate) font: RwSignal<f32>,
}

impl Controls {
    pub(crate) fn of(look: &Look, config: &Config) -> Self {
        Self {
            name: signal(look.name.clone()),
            accent: signal(look.accent.clone()),
            radius: signal(
                look.radius
                    .map_or_else(|| unset_radius(config), |r| r as f32),
            ),
            opacity: signal(look.opacity),
            font: signal(look.font),
        }
    }
}

/// The radius the palette gives where `[theme] radius` is unset, which is what its row starts at.
fn unset_radius(config: &Config) -> f32 {
    config
        .theme_with(&ThemeConfig {
            radius: None,
            scale: ScaleConfig::default(),
            ..config.theme.clone()
        })
        .radius
        .round()
}

thread_local! {
    static PENDING: RwSignal<Option<Look>> = detached(|| signal(None));
    static OPEN: RefCell<Option<RwSignal<bool>>> = const { RefCell::new(None) };
    static SAVED_OVER: RefCell<Option<Arc<Config>>> = const { RefCell::new(None) };
}

/// What the popover has chosen and the running config does not say yet: `None` once a reload has read it back, or when nothing was chosen.
pub fn pending() -> RwSignal<Option<Look>> {
    PENDING.with(|pending| *pending)
}

pub(crate) fn install() {
    host::add_strip_action((|| telar::t!("editor.theme.open"), || mode::said(open())));
    detached(|| effect(show_pending));
    detached(|| effect(follow_the_config));
    detached(|| effect(close_with_the_mode));
}

fn running() -> Arc<Config> {
    config::config().unwrap_or_default()
}

fn show_pending() {
    match pending().get() {
        Some(look) => reconcile::preview_config(move |config| look.on(config)),
        None => reconcile::end_config_preview(),
    }
}

/// Lets go of the preview once a reload after the save has been reconciled: from then on the file is what the windows follow, a hand edit made since included.
fn follow_the_config() {
    reconcile::planned();
    if pending().get().is_none() || transient::is_open(ID) {
        return;
    }
    let reloaded = SAVED_OVER.with(|saved| {
        saved
            .borrow()
            .as_ref()
            .is_some_and(|saved| config::config().is_some_and(|now| !Arc::ptr_eq(saved, &now)))
    });
    if reloaded {
        SAVED_OVER.with(|saved| saved.borrow_mut().take());
        pending().set(None);
    }
}

fn close_with_the_mode() {
    if mode::active().get().is_none() && transient::is_open(ID) {
        close();
    }
}

/// Opens the theme popover on the edited screen, closing whatever popover, menu or palette was open.
pub(crate) fn open() -> Result<(), EditError> {
    let mode = mode::required()?;
    host::close_transients();
    let output = mode.output.clone();
    transient::open(
        Spec::new(
            ID,
            Place::Whole,
            Rc::new(move |_: &Chrome| theme_card(&output)),
        )
        .output(Some(mode.output.clone()))
        .keyboard(KeyboardMode::Exclusive)
        .dismiss_on_outside()
        .on_close(close),
    );
    Ok(())
}

fn close() {
    config_popover::close(ID, OPEN.with(|open| open.borrow_mut().take()));
}

pub(crate) fn theme_card(output: &str) -> Built {
    let config = running();
    let start = pending().peek().unwrap_or_else(|| Look::of(&config));
    card(output, &start, Controls::of(&start, &config))
}

/// The popover's card over `controls`: every change previewed on every window as it is made — one transaction over the look, kept when the card closes and put back by Esc or Cancel.
pub(crate) fn card(output: &str, start: &Look, controls: Controls) -> Built {
    let theme = use_theme::<NordTheme>();
    let config = running();
    let unset = unset_radius(&config);
    let (open, transaction) = config_popover::hold(ID, pending(), |after| saved(after.clone()));
    OPEN.with(|held| *held.borrow_mut() = Some(open));

    let Controls {
        name,
        accent,
        radius,
        opacity,
        font,
    } = controls;
    let pinned = start.radius.is_some();
    let seeded = Cell::new(false);
    effect(move || {
        let (name, accent, radius, opacity, font) = (
            name.get(),
            accent.get(),
            radius.get(),
            opacity.get(),
            font.get(),
        );
        if !seeded.replace(true) || !transaction.is_open() {
            return;
        }
        let _ = transaction.preview(|now| {
            *now = Some(Look {
                name,
                accent,
                radius: (pinned || radius != unset).then(|| radius.round().max(0.0) as u32),
                opacity: opacity.clamp(*OPACITY_RANGE.start(), *OPACITY_RANGE.end()),
                font: font.clamp(*FONT_SCALE_RANGE.start(), *FONT_SCALE_RANGE.end()),
            });
        });
    });

    let judged = Arc::clone(&config);
    let verdict = memo(move || {
        pending()
            .get()
            .and_then(|look| crate::modes::lock::falls_back_with(&judged, &look.on(&judged)))
    });
    let said = Text::new(
        move || match verdict.get() {
            Some(why) => telar::t!("editor.theme.lock_falls_back", why = why),
            None => telar::t!("editor.theme.note"),
        },
        LayoutStyle::new(),
        move || {
            let tint = match verdict.get() {
                Some(_) => theme.warning,
                None => theme.subtle,
            };
            theme.text_style(FontRole::Caption, tint)
        },
    )?;
    let title = Text::new(
        || telar::t!("editor.theme.title"),
        LayoutStyle::new(),
        move || {
            theme
                .text_style(FontRole::Body, theme.text)
                .with_font_weight(700)
        },
    )?;
    let mode_of = signal(config.theme.mode.clone());
    let rows: Vec<Box<dyn LayoutItem>> = vec![
        box_item(title),
        rows::captioned(
            label!("editor.theme.palette"),
            documented("theme", "name"),
            theme_tiles(name, mode_of.read_only(), Arc::clone(&config))?,
        )?,
        rows::colour(
            label!("editor.theme.accent"),
            documented("theme", "accent"),
            accent,
            Rc::from(ACCENTS),
            Rc::new(NordTheme::has_accent),
        )?,
        rows::heading(|| telar::t!("editor.theme.shape"))?,
        rows::number(
            label!("editor.theme.radius"),
            documented("theme", "radius"),
            radius,
            Range::whole(0.0, 32.0),
        )?,
        rows::number(
            label!("editor.theme.opacity"),
            documented("theme", "opacity"),
            opacity,
            Range::new(*OPACITY_RANGE.start(), *OPACITY_RANGE.end(), 0.05),
        )?,
        rows::heading(|| telar::t!("editor.theme.text"))?,
        rows::number(
            label!("editor.theme.size"),
            documented("theme", "scale.font"),
            font,
            Range::new(*FONT_SCALE_RANGE.start(), *FONT_SCALE_RANGE.end(), 0.05),
        )?,
        box_item(said),
        buttons(open, move || {
            let _ = transaction.revert();
        })?,
    ];
    config_popover::card(output, WIDTH, rows)
}

fn buttons(open: RwSignal<bool>, cancel: impl Fn() + 'static) -> Built {
    let button = |label: telar::Reactive<String>, press: Rc<dyn Fn()>| {
        telar::button(
            telar::ButtonProps::props()
                .label(label)
                .on_press(press)
                .build(),
            Children::default(),
        )
    };
    Ok(box_item(Container::new(
        LayoutStyle::new()
            .flex_row()
            .gap(ui::scale::space::sm())
            .justify_content(JustifyContent::END),
        vec![
            button(
                label!("editor.theme.cancel"),
                Rc::new(move || {
                    cancel();
                    open.set(false);
                }),
            )?,
            button(label!("editor.done"), Rc::new(move || open.set(false)))?,
        ],
    )?))
}

/// What closing the popover keeps: nothing when the running config says it already, else the look written into `config.toml`, previewed until a reload reads it back; a write that fails says so in the strip and the preview ends.
fn saved(kept: Option<Look>) {
    let Some(look) = kept else {
        return;
    };
    let running = running();
    if look == Look::of(&running) {
        pending().set(None);
        return;
    }
    match write(&Config::default_path(), &look) {
        Ok(()) => SAVED_OVER.with(|saved| *saved.borrow_mut() = Some(running)),
        Err(why) => {
            mode::refuse(telar::t!("editor.theme.not_saved", why = why));
            pending().set(None);
        }
    }
}

/// Writes `look` into the `[theme]` of the config at `path`, around whatever else the file says.
pub(crate) fn write(path: &Path, look: &Look) -> Result<(), String> {
    config_popover::save(path, "theme", |config| {
        let mut theme = config.theme;
        look.onto(&mut theme);
        theme
    })
}
