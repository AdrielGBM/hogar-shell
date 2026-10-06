//! Lock mode (TA-8): no tools of its own. The lock layer is edited with the background mode's regions and textures and the desktop mode's grids, palette and keys, over a preview of the lock screen; what this adds is only what the lock has and the other layers do not — the prompt, moved and restyled but never removed, and how much a locked screen may reveal.
//!
//! **What is edited is what the lock draws.** The preview runs the lock's own check ([`LockLayout::checked`]) and, where a real lock would fall back to the minimal one, shows that and says why. An edit that would make it fall back is refused before it is previewed ([`kept`]), with the lock's reason, so the lock layer an edit leaves is one the lock draws.
//!
//! **The privacy keys are config, not layout.** `[lock] notification_detail` and `media_detail` live in `config.toml`, which has one owner apart from the layout's (TA-7): their popover previews a choice live on the readings, Esc puts it back, and any other way of closing it writes it through the same format-preserving save the settings window uses. They are outside the layout's undo history.

use std::cell::{Cell, RefCell};
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;

use telar::{
    Children, LayoutError, LayoutStyle, ReactiveList, RwSignal, Text, box_item, detached, effect,
    signal, use_theme,
};

use config::theme::{FontRole, NordTheme};
use config::{Config, MediaDetail, NotificationDetail};
use layout::{
    LayerKind, Layout, LayoutId, LayoutOp, Library, OutputMatch, ResolvedAreaKind, SMALLEST_PROMPT,
    Style,
};
use modules::lock::LockLayout;
use platform_wayland::KeyboardMode;
use surfaces::catalogue::Descriptors;
use surfaces::reconcile::{self, Desktop};
use surfaces::rects::{self, Node};
use surfaces::transient::{self, Place, Spec};
use ui::chrome::Chrome;
use ui::descriptor::Built;
use util::report::Report;

use crate::config_popover;
use crate::host::{self, passthrough, whole};
use crate::keys::{self, Chord, KeyOp, Run};
use crate::mode::{self, Mode, said};
use crate::popover::area::rect_rows;
use crate::popover::rows::{self, label};
use crate::popover::{AreaDraft, Inspector, kind_field, parsed, spelled};
use crate::session::{self, EditError};
use crate::written::{Written, known};

use super::{background, desktop, rect_handles, widgets};

/// The transient the privacy popover is.
pub const PRIVACY: &str = "editor:privacy";

/// How wide the privacy popover's card is.
const WIDTH: f32 = 360.0;

pub(crate) fn install() {
    crate::popover::add_area_tool("prompt", prompt_tool);
    host::add_tool(LayerKind::Lock, crate::select::tool);
    host::add_tool(LayerKind::Lock, rect_handles::bodies);
    host::add_tool(LayerKind::Lock, background::tool);
    host::add_tool(LayerKind::Lock, widgets::tool);
    host::add_tool(LayerKind::Lock, tool);
    desktop::add_grid_tools(LayerKind::Lock);
    host::add_toolbar_button(LayerKind::Lock, background::TEXTURE_BUTTON);
    host::add_toolbar_button(
        LayerKind::Lock,
        (|| telar::t!("editor.lock.privacy"), || said(open_privacy())),
    );
    keys::add_mode_key_op(
        LayerKind::Lock,
        KeyOp {
            name: "lock-privacy",
            keys: vec![Chord::char('p')],
            label: || telar::t!("editor.keys.op.lock-privacy"),
            run: Run::Act(|_| open_privacy()),
        },
    );
    detached(|| effect(follow_the_config));
}

/// The config the lock is drawn with: the running one, which is what a lock taken now would read, with a theme being previewed on it.
fn lock_config() -> Arc<Config> {
    let running = config::config()
        .or_else(|| {
            reconcile::desktops_now()
                .first()
                .map(|desktop| Arc::clone(&desktop.config))
        })
        .unwrap_or_default();
    reconcile::shown_config(&running).1
}

/// What the lock's own check is asked with, read as a lock taken now would read it: every layout the store holds, every screen there is, the module table installed and the lock's theme.
struct Judged {
    known: Library,
    outputs: Vec<String>,
    file: String,
    theme: NordTheme,
}

impl Judged {
    fn now(config: &Config) -> Self {
        Self {
            known: known(),
            outputs: reconcile::desktops_now()
                .iter()
                .filter_map(|desktop| desktop.output.clone())
                .collect(),
            file: surfaces::layouts::read(|store| {
                store.path_of(store.active_id()).display().to_string()
            })
            .unwrap_or_default(),
            theme: config.resolve_theme(),
        }
    }

    fn outputs(&self) -> Vec<Option<&str>> {
        self.outputs
            .iter()
            .map(|output| Some(output.as_str()))
            .collect()
    }

    fn problems(&self, layout: &Layout) -> Report {
        LockLayout::problems(
            layout,
            &self.known,
            &Descriptors::installed(),
            &self.theme,
            &self.outputs(),
            &self.file,
        )
    }

    fn checked(&self, layout: &Layout) -> Result<LockLayout, Report> {
        LockLayout::checked(
            layout,
            &self.known,
            &Descriptors::installed(),
            &self.theme,
            &self.outputs(),
            &self.file,
        )
    }
}

/// What a refused lock layer is refused for, as the preview says it: the first thing wrong, in the lock's own words.
fn reason(report: &Report) -> String {
    report
        .findings()
        .next()
        .map(|found| found.message.render())
        .unwrap_or_default()
}

/// Refuses `after` where it would make a locked screen fall back to the minimal lock and `before` did not — a prompt too faint, too small, off its screen or unreadable, a control — saying why in the lock's own words. Only an edit that changes the lock layer is judged, and a lock layer already refused is never made harder to fix.
pub(crate) fn kept(before: &Layout, after: &Layout) -> Result<(), String> {
    if lock_of(before) == lock_of(after) {
        return Ok(());
    }
    let judged = Judged::now(&lock_config());
    let was = judged.problems(before).errors;
    match judged
        .problems(after)
        .errors
        .into_iter()
        .find(|found| !was.contains(found))
    {
        Some(found) => Err(telar::t!(
            "editor.lock.would_fall_back",
            why = found.message.render()
        )),
        None => Ok(()),
    }
}

/// The first reason the lock would fall back to the minimal one with `after`'s theme that `before`'s theme did not give, in the lock's own words.
pub(crate) fn falls_back_with(before: &Config, after: &Config) -> Option<String> {
    let layout = session::draft().peek();
    let mut judged = Judged::now(before);
    let was = judged.problems(&layout).errors;
    judged.theme = after.resolve_theme();
    judged
        .problems(&layout)
        .errors
        .into_iter()
        .find(|found| !was.contains(found))
        .map(|found| found.message.render())
}

/// What of `layout` its lock layer is resolved from.
fn lock_of(layout: &Layout) -> (Option<&LayoutId>, Vec<(&OutputMatch, &layout::Layer)>) {
    (
        layout.extends.as_ref(),
        layout
            .outputs
            .iter()
            .map(|rule| (&rule.matches, &rule.layers.lock))
            .collect(),
    )
}

/// The lock layer previewed over the whole screen `output` (TA-8): the draft's, so an undecided edit shows as the lock would draw it, with the privacy the popover has chosen — built again whenever either, or the screen, changes.
pub(crate) fn preview(output: &str) -> Built {
    let output = output.to_string();
    let draft = session::draft();
    let seen: RefCell<(u64, Option<Layout>)> = RefCell::new((0, None));
    Ok(Box::new(ReactiveList::with_style(
        whole(),
        move || {
            let mut seen = seen.borrow_mut();
            if draft.with(|layout| seen.1.as_ref() != Some(layout)) {
                *seen = (seen.0.wrapping_add(1), Some(draft.peek()));
            }
            let (version, privacy) = (seen.0, chosen().get());
            reconcile::desktop(Some(&output))
                .map(|desktop| (version, privacy, desktop))
                .into_iter()
                .collect()
        },
        |(version, privacy, desktop): &(u64, Option<Privacy>, Desktop)| {
            (
                *version,
                format!("{privacy:?}{}", host::arrangement(desktop)),
            )
        },
        move |(_, _, desktop): (u64, Option<Privacy>, Desktop)| drawn(&draft.peek(), &desktop),
    )?))
}

/// The lock screen `layout` would put on `desktop`'s screen: its lock layer where the lock's own check passes, else the minimal lock and why.
fn drawn(layout: &Layout, desktop: &Desktop) -> Built {
    let config = shown(&lock_config());
    let judged = Judged::now(&config);
    let (output, size) = (desktop.output.as_deref(), desktop.size);
    match judged.checked(layout) {
        Ok(lock) => modules::lock::preview(&config, Ok(&lock), output, size),
        Err(report) => modules::lock::preview(&config, Err(&reason(&report)), output, size),
    }
}

/// What lock mode lays over the preview besides the tools it borrows: the prompt, dragged by itself to move it, kept wholly on its screen. A press on it selects what is under it, as every target does; a secondary press asks for a menu, which the lock layer never has, and the strip says so.
pub(crate) fn tool(mode: &Mode) -> Built {
    let output = mode.output.clone();
    let handles = ReactiveList::with_style(
        whole(),
        move || prompts(&output),
        |node: &Node| node.clone(),
        |node: Node| rect_handles::carried_body(node, telar::t!("editor.lock.prompt_moved")),
    )?;
    Ok(Box::new(passthrough(whole(), vec![Box::new(handles)])?))
}

/// The prompt of the screen `output`, where the preview has drawn it.
fn prompts(output: &str) -> Vec<Node> {
    let Some(desktop) = reconcile::desktop(Some(output)) else {
        return Vec::new();
    };
    let Some(layer) = desktop.resolved.layer(LayerKind::Lock) else {
        return Vec::new();
    };
    layer
        .areas
        .iter()
        .filter(|area| matches!(area.kind, ResolvedAreaKind::Prompt { .. }))
        .map(|area| Node::area(Some(output), LayerKind::Lock, &area.id))
        .filter(|node| rects::rect(node).is_some())
        .collect()
}

/// The operations that put the prompt `node` names at `to`, kept wholly on its screen and no smaller than a prompt may be — written where the layout decides the prompt.
pub(crate) fn moved_to(
    layout: &Layout,
    node: &Node,
    to: layout::Rect,
) -> Result<Vec<LayoutOp>, EditError> {
    let written = Written::area(
        layout,
        node.output.as_deref(),
        LayerKind::Lock,
        &node.area,
        None,
    )
    .map_err(EditError::Refused)?;
    let mut area = written.area.clone();
    kind_field!(
        &mut area,
        "prompt",
        Prompt { rect },
        to.kept_on_output(SMALLEST_PROMPT)
    );
    Ok(written.ops(&area))
}

/// The prompt's own popover rows: the rectangle it is placed at, kept wholly on its screen and no smaller than a prompt may be.
fn prompt_tool(draft: &AreaDraft) -> Result<Inspector, LayoutError> {
    let ResolvedAreaKind::Prompt { rect } = draft.resolved.kind else {
        return Ok(Inspector::default());
    };
    Ok(Inspector {
        rows: rect_rows(draft, rect)?,
        handles: Vec::new(),
    })
}

/// The line under a prompt's style rows: how readable its text is on the card the rows make, live, and — where a fill the layout chose would leave it unreadable — that the lock would fall back to the minimal one, so the fill is not kept.
pub(crate) fn contrast_row(draft: &AreaDraft) -> Built {
    let theme = use_theme::<NordTheme>();
    let judged = lock_config().resolve_theme();
    let inherited = draft.resolved.style.clone();
    let area = draft.area();
    let style = move || {
        let written = area.with(|area| area.style.clone());
        Style {
            fill: written.fill.or_else(|| inherited.fill.clone()),
            opacity: written.opacity.or(inherited.opacity),
            ..inherited.clone()
        }
    };
    let reading = style.clone();
    Ok(box_item(Text::new(
        move || contrast_of(&reading(), &judged),
        LayoutStyle::new(),
        move || {
            let tint = match readable(&style(), &judged) {
                true => theme.subtle,
                false => theme.warning,
            };
            theme.text_style(FontRole::Caption, tint)
        },
    )?))
}

/// Whether validation would let the prompt be drawn with `style`: only a fill the layout chose is judged, as the lock judges it.
fn readable(style: &Style, theme: &NordTheme) -> bool {
    style.fill.is_none()
        || config::scheme::is_readable(theme.text, layout::prompt_backdrop(style, theme))
}

/// How readable the prompt's text is with `style`, as the contrast row says it.
pub fn contrast_of(style: &Style, theme: &NordTheme) -> String {
    let ratio = format!(
        "{:.1}",
        theme
            .text
            .contrast_ratio(layout::prompt_backdrop(style, theme))
    );
    let least = config::scheme::MIN_TEXT_CONTRAST.to_string();
    match readable(style, theme) {
        true => telar::t!("editor.lock.contrast", ratio = ratio, least = least),
        false => telar::t!("editor.lock.unreadable", ratio = ratio, least = least),
    }
}

/// How much a locked screen may reveal: `[lock] notification_detail` and `media_detail` (TA-8).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Privacy {
    pub notifications: NotificationDetail,
    pub media: MediaDetail,
}

impl Privacy {
    pub fn of(config: &Config) -> Self {
        Self {
            notifications: config.lock.notification_detail,
            media: config.lock.media_detail,
        }
    }

    fn on(self, config: &Config) -> Config {
        let mut config = config.clone();
        config.lock.notification_detail = self.notifications;
        config.lock.media_detail = self.media;
        config
    }
}

thread_local! {
    static CHOSEN: RwSignal<Option<Privacy>> = detached(|| signal(None));
    static PRIVACY_OPEN: RefCell<Option<RwSignal<bool>>> = const { RefCell::new(None) };
}

/// What the privacy popover has chosen and the config does not say yet, as a signal: `None` once the config says it, or when nothing was chosen.
pub fn chosen() -> RwSignal<Option<Privacy>> {
    CHOSEN.with(|chosen| *chosen)
}

/// `config` with the privacy the popover has chosen, which is what the preview's readings are built for.
fn shown(config: &Arc<Config>) -> Arc<Config> {
    match chosen().get() {
        Some(privacy) if privacy != Privacy::of(config) => Arc::new(privacy.on(config)),
        _ => Arc::clone(config),
    }
}

/// Lets go of a chosen privacy once the config the shell runs says it too: the write has been read back, and from then on the config is what the preview follows — a hand edit to the file included.
fn follow_the_config() {
    // The config is no signal: following the screens is what runs this again once a reload has been reconciled.
    reconcile::desktops();
    let Some(privacy) = chosen().get() else {
        return;
    };
    if transient::is_open(PRIVACY) {
        return;
    }
    if config::config().is_some_and(|config| Privacy::of(&config) == privacy) {
        chosen().set(None);
    }
}

/// Opens the lock layer's popover, closing whatever popover, menu or palette was open: what a locked screen may reveal about notifications and what is playing.
pub(crate) fn open_privacy() -> Result<(), EditError> {
    let mode = mode::current()
        .filter(|mode| mode.layer == LayerKind::Lock)
        .ok_or_else(|| EditError::Refused(telar::t!("editor.lock.only_here")))?;
    host::close_transients();
    let output = mode.output.clone();
    transient::open(
        Spec::new(
            PRIVACY,
            Place::Whole,
            Rc::new(move |_: &Chrome| privacy_card(&output)),
        )
        .output(Some(mode.output.clone()))
        .keyboard(KeyboardMode::Exclusive)
        .dismiss_on_outside()
        .on_close(close_privacy),
    );
    Ok(())
}

fn close_privacy() {
    config_popover::close(PRIVACY, PRIVACY_OPEN.with(|open| open.borrow_mut().take()));
}

/// The privacy popover's card: a choice for each key, previewed on the readings as it is made — one transaction over the choice, kept when the card closes and put back on Esc.
pub(crate) fn privacy_card(output: &str) -> Built {
    let theme = use_theme::<NordTheme>();
    let start = chosen()
        .peek()
        .unwrap_or_else(|| Privacy::of(&lock_config()));
    let (open, transaction) = config_popover::hold(PRIVACY, chosen(), |after| saved(*after));
    PRIVACY_OPEN.with(|held| *held.borrow_mut() = Some(open));

    let notifications = signal(spelled(&start.notifications));
    let media = signal(spelled(&start.media));
    let seeded = Cell::new(false);
    effect(move || {
        let picked = Privacy {
            notifications: parsed(&notifications.get()).unwrap_or(start.notifications),
            media: parsed(&media.get()).unwrap_or(start.media),
        };
        if seeded.replace(true) && transaction.is_open() {
            let _ = transaction.preview(|now| *now = Some(picked));
        }
    });
    let title = Text::new(
        || telar::t!("editor.lock.privacy_title"),
        LayoutStyle::new(),
        move || {
            theme
                .text_style(FontRole::Body, theme.text)
                .with_font_weight(700)
        },
    )?;
    let rows = vec![
        box_item(title),
        rows::listed(
            label!("editor.lock.notifications"),
            config_popover::documented("lock", "notification_detail"),
            notifications,
            Rc::from(vec![
                (
                    spelled(&NotificationDetail::Count),
                    telar::t!("editor.lock.count"),
                ),
                (
                    spelled(&NotificationDetail::Apps),
                    telar::t!("editor.lock.apps"),
                ),
            ]),
        )?,
        rows::listed(
            label!("editor.lock.media"),
            config_popover::documented("lock", "media_detail"),
            media,
            Rc::from(vec![
                (spelled(&MediaDetail::Title), telar::t!("editor.lock.title")),
                (spelled(&MediaDetail::State), telar::t!("editor.lock.state")),
            ]),
        )?,
        rows::note(|| telar::t!("editor.lock.privacy_note"))?,
        telar::button(
            telar::ButtonProps::props()
                .label(label!("editor.done"))
                .on_press(Rc::new(move || open.set(false)))
                .build(),
            Children::default(),
        )?,
    ];
    config_popover::card(output, WIDTH, rows)
}

/// What closing the privacy popover keeps: nothing when the config says it already, else the choice written into `config.toml`; a write that fails says so in the strip and the preview goes back to what the config says.
fn saved(kept: Option<Privacy>) {
    let Some(privacy) = kept else {
        return;
    };
    if privacy == Privacy::of(&lock_config()) {
        chosen().set(None);
        return;
    }
    if let Err(why) = write(&Config::default_path(), privacy) {
        mode::refuse(telar::t!("editor.lock.not_saved", why = why));
        chosen().set(None);
    }
}

/// Writes `privacy` into the `[lock]` of the config at `path`, around whatever else the file says there.
pub(crate) fn write(path: &Path, privacy: Privacy) -> Result<(), String> {
    config_popover::save(path, "lock", |config| {
        let mut lock = config.lock;
        lock.notification_detail = privacy.notifications;
        lock.media_detail = privacy.media;
        lock
    })
}
