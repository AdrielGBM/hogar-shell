//! The theme popover: `[theme]`, and how applications' icons look in `[icons]`, are config rather than layout, so they preview on every window while it is open, are written to `config.toml` when it closes and stay outside the layout's undo history. The presets kept beside the config are picked, saved and deleted here too.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use telar::{
    Children, Container, JustifyContent, LayoutItem, LayoutStyle, RwSignal, Text, box_item,
    detached, effect, memo, signal, use_theme,
};

use config::presets;
use config::theme::{
    ACCENTS, FONT_SCALE_RANGE, FONT_WEIGHT_RANGE, FontRole, NordTheme, OPACITY_RANGE,
};
use config::{Config, IconMask, IconsConfig, ScaleConfig, SectionEdit, ThemeConfig};
use platform_wayland::KeyboardMode;
use surfaces::reconcile;
use surfaces::transient::{self, Place, Spec};
use ui::chrome::Chrome;
use ui::descriptor::Built;
use ui::form::listed_row::keeping_current;
use ui::form::named_list::{NamedActions, named_list};
use ui::form::theme_tiles::theme_tiles;

use crate::config_popover::{self, documented};
use crate::host;
use crate::mode;
use crate::popover::rows::{self, Range, label};
use crate::session::EditError;

pub const ID: &str = "editor:theme";

const WIDTH: f32 = 400.0;

/// The keys of `[theme]` and `[icons]` the popover sets, over the whole `[theme]` of a preset picked in it.
#[derive(Clone, Debug, PartialEq)]
pub struct Look {
    pub preset: Option<ThemeConfig>,
    pub name: String,
    pub accent: String,
    pub radius: Option<u32>,
    pub opacity: f32,
    pub font: f32,
    /// One per [`FontRole::ALL`], in its order.
    pub weights: [Option<u16>; 4],
    pub app_icon_theme: String,
    pub mask: IconMask,
}

impl Look {
    pub fn of(config: &Config) -> Self {
        Self {
            preset: None,
            name: config.theme.name.clone(),
            accent: config.theme.accent.clone(),
            radius: config.theme.radius,
            opacity: config.theme.opacity,
            font: config.theme.scale.font,
            weights: FontRole::ALL.map(|role| config.theme.fonts.spec(role).weight),
            app_icon_theme: config.icons.app_icon_theme.clone(),
            mask: config.icons.mask,
        }
    }

    pub fn on(&self, config: &Config) -> Config {
        let mut config = config.clone();
        config.theme = self.theme_on(&config.theme);
        config.icons = self.icons_on(&config.icons);
        config
    }

    fn theme_on(&self, theme: &ThemeConfig) -> ThemeConfig {
        let mut theme = match &self.preset {
            Some(preset) => presets::applied_to(theme, preset.clone()),
            None => theme.clone(),
        };
        theme.name = self.name.clone();
        theme.accent = self.accent.clone();
        theme.radius = self.radius;
        theme.opacity = self.opacity;
        theme.scale.font = self.font;
        for (role, weight) in FontRole::ALL.into_iter().zip(self.weights) {
            theme.fonts.spec_mut(role).weight = weight;
        }
        theme
    }

    fn icons_on(&self, icons: &IconsConfig) -> IconsConfig {
        IconsConfig {
            app_icon_theme: self.app_icon_theme.clone(),
            mask: self.mask,
            ..icons.clone()
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Controls {
    pub(crate) name: RwSignal<String>,
    pub(crate) accent: RwSignal<String>,
    pub(crate) radius: RwSignal<f32>,
    pub(crate) opacity: RwSignal<f32>,
    pub(crate) font: RwSignal<f32>,
    /// One per [`FontRole::ALL`], each a weight or empty for the role's own.
    pub(crate) weights: [RwSignal<String>; 4],
    pub(crate) app_icon_theme: RwSignal<String>,
    pub(crate) mask: RwSignal<String>,
    pub(crate) preset: RwSignal<Option<ThemeConfig>>,
    pub(crate) preset_name: RwSignal<String>,
    radius_pinned: RwSignal<bool>,
    radius_unset: RwSignal<f32>,
}

impl Controls {
    pub(crate) fn of(look: &Look, config: &Config) -> Self {
        let unset = unset_radius(&look.on(config));
        Self {
            name: signal(look.name.clone()),
            accent: signal(look.accent.clone()),
            radius: signal(look.radius.map_or(unset, |r| r as f32)),
            opacity: signal(look.opacity),
            font: signal(look.font),
            weights: look.weights.map(|weight| signal(spelled_weight(weight))),
            app_icon_theme: signal(look.app_icon_theme.clone()),
            mask: signal(look.mask.id().to_string()),
            preset: signal(look.preset.clone()),
            preset_name: signal(String::new()),
            radius_pinned: signal(look.radius.is_some()),
            radius_unset: signal(unset),
        }
    }

    /// The look the controls say now; inside an effect, it follows every one of them.
    fn look(&self) -> Look {
        let radius = self.radius.get();
        let pinned = self.radius_pinned.get() || radius != self.radius_unset.get();
        Look {
            preset: self.preset.get(),
            name: self.name.get(),
            accent: self.accent.get(),
            radius: pinned.then(|| radius.round().max(0.0) as u32),
            opacity: self
                .opacity
                .get()
                .clamp(*OPACITY_RANGE.start(), *OPACITY_RANGE.end()),
            font: self
                .font
                .get()
                .clamp(*FONT_SCALE_RANGE.start(), *FONT_SCALE_RANGE.end()),
            weights: self.weights.map(|weight| parsed_weight(&weight.get())),
            app_icon_theme: self.app_icon_theme.get(),
            mask: IconMask::from_id(&self.mask.get()).unwrap_or_default(),
        }
    }

    /// Sets every control to what `theme` says and keeps the whole of it as the preset the look starts from, in one change.
    fn pick(&self, theme: ThemeConfig, running: &Config) {
        let palette = Config {
            theme: theme.clone(),
            ..running.clone()
        };
        let unset = unset_radius(&palette);
        telar::batch(|| {
            self.name.set(theme.name.clone());
            self.accent.set(theme.accent.clone());
            self.radius_unset.set(unset);
            self.radius_pinned.set(theme.radius.is_some());
            self.radius.set(theme.radius.map_or(unset, |r| r as f32));
            self.opacity.set(theme.opacity);
            self.font.set(theme.scale.font);
            for (weight, role) in self.weights.iter().zip(FontRole::ALL) {
                weight.set(spelled_weight(theme.fonts.spec(role).weight));
            }
            self.preset.set(Some(theme));
        });
    }
}

fn spelled_weight(weight: Option<u16>) -> String {
    weight.map(|weight| weight.to_string()).unwrap_or_default()
}

fn parsed_weight(spelled: &str) -> Option<u16> {
    spelled
        .trim()
        .parse::<u16>()
        .ok()
        .map(|weight| weight.clamp(*FONT_WEIGHT_RANGE.start(), *FONT_WEIGHT_RANGE.end()))
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
    card(output, Controls::of(&start, &config))
}

/// The popover's card over `controls`: every change previewed on every window as it is made — one transaction over the look, kept when the card closes and put back by Esc or Cancel.
pub(crate) fn card(output: &str, controls: Controls) -> Built {
    let theme = use_theme::<NordTheme>();
    let config = running();
    let (open, transaction) = config_popover::hold(ID, pending(), |after| saved(after.clone()));
    OPEN.with(|held| *held.borrow_mut() = Some(open));

    let seeded = Cell::new(false);
    effect(move || {
        let look = controls.look();
        if !seeded.replace(true) || !transaction.is_open() {
            return;
        }
        let _ = transaction.preview(|now| *now = Some(look));
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
    effect(move || {
        if let Some(mode) = controls
            .preset
            .with(|preset| preset.as_ref().map(|theme| theme.mode.clone()))
            && mode_of.peek() != mode
        {
            mode_of.set(mode);
        }
    });
    let mut rows: Vec<Box<dyn LayoutItem>> = vec![
        box_item(title),
        rows::captioned(
            label!("editor.theme.palette"),
            documented("theme", "name"),
            theme_tiles(controls.name, mode_of.read_only(), Arc::clone(&config))?,
        )?,
        rows::colour(
            label!("editor.theme.accent"),
            documented("theme", "accent"),
            controls.accent,
            Rc::from(ACCENTS),
            Rc::new(NordTheme::has_accent),
        )?,
        rows::heading(|| telar::t!("editor.theme.shape"))?,
        rows::number(
            label!("editor.theme.radius"),
            documented("theme", "radius"),
            controls.radius,
            Range::whole(0.0, 32.0),
        )?,
        rows::number(
            label!("editor.theme.opacity"),
            documented("theme", "opacity"),
            controls.opacity,
            Range::new(*OPACITY_RANGE.start(), *OPACITY_RANGE.end(), 0.05),
        )?,
        rows::heading(|| telar::t!("editor.theme.text"))?,
        rows::number(
            label!("editor.theme.size"),
            documented("theme", "scale.font"),
            controls.font,
            Range::new(*FONT_SCALE_RANGE.start(), *FONT_SCALE_RANGE.end(), 0.05),
        )?,
    ];
    for (weight, role) in controls.weights.into_iter().zip(FontRole::ALL) {
        rows.push(rows::listed(
            weight_label(role),
            documented("theme", &format!("fonts.{}.weight", role.id())),
            weight,
            weight_options(&weight.peek()),
        )?);
    }
    rows.extend([
        rows::heading(|| telar::t!("editor.theme.icons"))?,
        rows::listed(
            label!("editor.theme.icon_theme"),
            documented("icons", "app_icon_theme"),
            controls.app_icon_theme,
            icon_theme_options(&controls.app_icon_theme.peek()),
        )?,
        rows::listed(
            label!("editor.theme.mask"),
            documented("icons", "mask"),
            controls.mask,
            mask_options(),
        )?,
    ]);
    rows.extend(preset_rows(controls, &config)?);
    let footer = vec![
        box_item(said),
        buttons(open, move || {
            let _ = transaction.revert();
        })?,
    ];
    config_popover::framed(output, WIDTH, rows, footer)
}

fn weight_label(role: FontRole) -> telar::Reactive<String> {
    match role {
        FontRole::Display => label!("editor.theme.weight_display"),
        FontRole::Title => label!("editor.theme.weight_title"),
        FontRole::Body => label!("editor.theme.weight_body"),
        FontRole::Caption => label!("editor.theme.weight_caption"),
    }
}

/// The role's own weight, every hundred a font can be drawn at, and `current` where it is none of them, so a weight the file says is shown as itself.
fn weight_options(current: &str) -> Rc<[(String, String)]> {
    let mut options = vec![(String::new(), telar::t!("editor.theme.weight_default"))];
    options.extend(
        config::theme::font_weights().map(|weight| (weight.to_string(), weight.to_string())),
    );
    keeping_current(options, current)
}

/// The desktop's own icon theme, every one installed, and `current` where it is not installed here.
fn icon_theme_options(current: &str) -> Rc<[(String, String)]> {
    let mut options = vec![(String::new(), telar::t!("editor.theme.icon_theme_auto"))];
    options.extend(
        ui::icon::installed_icon_themes()
            .into_iter()
            .map(|theme| (theme.id, theme.name)),
    );
    keeping_current(options, current)
}

fn mask_options() -> Rc<[(String, String)]> {
    IconMask::ALL
        .into_iter()
        .map(|mask| {
            let shown = match mask {
                IconMask::None => telar::t!("editor.theme.mask_none"),
                IconMask::Circle => telar::t!("editor.theme.mask_circle"),
                IconMask::Squircle => telar::t!("editor.theme.mask_squircle"),
            };
            (mask.id().to_string(), shown)
        })
        .collect()
}

fn presets_path() -> PathBuf {
    Config::default_path()
}

/// The presets kept beside the config, each picked into the controls or deleted, and the look the controls say saved as a new one.
fn preset_rows(controls: Controls, running: &Arc<Config>) -> rows::Rows {
    let names = signal(presets::list(&presets_path()));
    let naming = controls.preset_name;
    let status = signal(String::new());
    let refresh = move || names.set(presets::list(&presets_path()));

    let picking = Arc::clone(running);
    let actions = NamedActions {
        pick: Rc::new(|| telar::t!("editor.theme.use_preset")),
        on_pick: Rc::new(
            move |name: &str| match presets::load(&presets_path(), name) {
                Ok(theme) => {
                    controls.pick(theme, &picking);
                    naming.set(name.to_string());
                    status.set(String::new());
                }
                Err(why) => status.set(telar::t!(
                    "editor.theme.preset_failed",
                    why = why.to_string()
                )),
            },
        ),
        remove: Rc::new(|| telar::t!("editor.theme.delete_preset")),
        on_remove: Rc::new(move |name: &str| {
            match presets::delete(&presets_path(), name) {
                Ok(()) => status.set(telar::t!("editor.theme.preset_deleted", name = name)),
                Err(why) => status.set(telar::t!(
                    "editor.theme.preset_failed",
                    why = why.to_string()
                )),
            }
            refresh();
        }),
    };
    let saving = Arc::clone(running);
    let save = move || {
        let name = naming.peek().trim().to_string();
        if !presets::is_name(&name) {
            status.set(telar::t!("editor.theme.preset_bad_name"));
            return;
        }
        let theme = controls.look().on(&saving).theme;
        match presets::save(&presets_path(), &name, &theme) {
            Ok(()) => status.set(telar::t!("editor.theme.preset_saved", name = name)),
            Err(why) => status.set(telar::t!(
                "editor.theme.preset_failed",
                why = why.to_string()
            )),
        }
        refresh();
    };
    Ok(vec![
        rows::heading(|| telar::t!("editor.theme.presets"))?,
        named_list(
            names.read_only(),
            || telar::t!("editor.theme.no_presets"),
            actions,
        )?,
        rows::text(
            label!("editor.theme.preset_name"),
            Some(telar::t!("editor.theme.preset_name_help")),
            naming,
        )?,
        rows::action(|| telar::t!("editor.theme.save_preset"), save)?,
        rows::note(move || status.get())?,
    ])
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
    let after = look.on(&running);
    if after.theme == running.theme && after.icons == running.icons {
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

/// Writes `look` into the `[theme]` of the config at `path`, and into its `[icons]` where it changes them, in one write around whatever else the file says.
pub(crate) fn write(path: &Path, look: &Look) -> Result<(), String> {
    let config = Config::load_or_default(path);
    let mut edits = vec![
        SectionEdit::new("theme", &look.theme_on(&config.theme)).map_err(|why| why.to_string())?,
    ];
    let icons = look.icons_on(&config.icons);
    if icons != config.icons {
        edits.push(SectionEdit::new("icons", &icons).map_err(|why| why.to_string())?);
    }
    Config::save_sections(path, &edits)
        .map(|_| ())
        .map_err(|why| why.to_string())
}
