//! `[stack]`, `[notifications]`, `[toasts]` and `[sidebar]`.
//!
//! One type per `[toml]` table, each with the defaults the shell falls back to. The doc comment on a
//! field is what `hogar-shell config schema` prints for it, so it is written for a user reading the reference.

use serde::{Deserialize, Serialize};

use crate::sections::*;

/// The column of cards the shell pins to a screen edge and takes away again (`[stack]`): notification popups,
/// in-shell toasts, and the OSD a volume or brightness change flashes.
///
/// **One section because they are one column.** They were three, each with its own `edge`, `align`, `width` and
/// timeout, and being three is what let them sit in three different places and overlap each other on a narrow
/// screen with no one of them able to know. Where the column is, how wide it is and how many cards it shows at
/// once are properties of the column; what each card *is* stays in `[notifications]` and `[toasts]`.
///
/// `timeout_ms` is one number for the same reason. Which is not to say every card goes: a `critical`
/// notification under `[notifications] critical_sticky` stays until it is dealt with, and so does an OSD with
/// nothing left to say. Not expiring is a property of the card, not a second timeout.
#[derive(Deserialize, Serialize, Clone, Copy, Debug)]
#[serde(default)]
pub struct StackConfig {
    pub edge: Edge,
    pub align: Align,
    pub width: f32,
    /// How many cards show at once; the rest queue behind them.
    ///
    /// Not a hard ceiling, and the exception is the point: every source with something to say — a notification,
    /// a toast, an OSD — is guaranteed one card before this is shared out, so a brightness reading you asked for
    /// by pressing a key is never queued behind notifications you did not. With more sources speaking at once
    /// than this allows, the column is that many cards tall.
    pub max_visible: u32,
    pub timeout_ms: u64,
    /// How far sideways a card must be dragged before letting go retires it, as a fraction of its width.
    /// `0` switches the gesture off, which is what a touchpad user who keeps catching it wants.
    ///
    /// The column's, not any one card's: a notification, a toast and an OSD are dismissed by the same gesture,
    /// and a threshold that differed between them would make the column feel like three surfaces again.
    pub clear_threshold: f32,
}

impl Default for StackConfig {
    fn default() -> Self {
        Self {
            edge: Edge::Top,
            align: Align::End,
            width: 380.0,
            max_visible: 4,
            // Between the 5 s a notification used to get and the 1.2 s an OSD did: long enough to read a line
            // of text that arrived unannounced, short enough that a volume nudge is gone before it is in the way.
            timeout_ms: 3000,
            clear_threshold: 0.35,
        }
    }
}

impl StackConfig {
    /// How long a card stays. Floored rather than allowed to be zero: a card that expires on the frame it was
    /// posted is a feature that looks broken. A card that must *not* expire says so itself.
    pub fn lifetime(&self) -> std::time::Duration {
        std::time::Duration::from_millis(self.timeout_ms.clamp(400, 60_000))
    }

    /// The swipe distance that retires a card, in px for a card `width` wide, or `None` when the gesture is
    /// off. Bounded below the full width: a threshold you cannot reach is a gesture that never fires, which
    /// reads as the card being stuck rather than as the setting being wrong.
    pub fn swipe_distance(&self, width: f32) -> Option<f32> {
        if !self.clear_threshold.is_finite() || self.clear_threshold <= 0.0 {
            return None;
        }
        Some(width * self.clear_threshold.min(0.9))
    }

    /// How many cards are on screen at once, bounded so a typo cannot ask for a column taller than the screen.
    pub fn visible(&self) -> usize {
        self.max_visible.clamp(1, 10) as usize
    }
}

/// Whether a popup still appears while a fullscreen window has focus. The three values escalate: `on` never
/// holds anything back, `off` holds back everything but `critical` (don't interrupt a game or a film unless it
/// matters), `never` holds back all of it. Suppression only affects the *popup* — the notification is recorded
/// and waits in the history either way, exactly as Do-Not-Disturb does.
#[derive(Deserialize, Serialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum FullscreenPopups {
    On,
    #[default]
    Off,
    Never,
}

impl FullscreenPopups {
    /// Whether a notification of `urgency` may still pop while a fullscreen window is focused.
    pub fn allows(self, urgency: crate::policy::Urgency) -> bool {
        use crate::policy::Urgency;
        match self {
            Self::On => true,
            Self::Off => urgency == Urgency::Critical,
            Self::Never => false,
        }
    }
}

/// Notification popups: what a card shows and how it behaves. Where the column sits, how wide it is, how many
/// cards it holds and how long each stays are the column's — see [`StackConfig`].
///
/// The history panel's own behaviour lives here too, since it draws the same cards: `group_by_app` collapses an
/// application's notifications under one header with a count, a mute and a clear, showing `group_preview_num`
/// of them until the group is expanded; `action_on_click` makes tapping a card body invoke the notification's
/// `default` action rather than only dismissing it; `body_lines`/`open_expanded` bound (or release) how much of
/// a long body a card shows.
///
/// `sound` is a command run — detached, through `sh -c` — each time a notification actually pops. Empty is
/// silent, which is the default: a shell that started making noise on upgrade would be a bug, and the right
/// command is per-machine (`canberra-gtk-play -i message`, `paplay /usr/share/sounds/…`).
#[derive(Deserialize, Serialize, Clone, Debug)]
#[serde(default)]
pub struct NotificationsConfig {
    /// Whether a `critical` notification ignores `[stack] timeout_ms` and waits to be dealt with.
    pub critical_sticky: bool,
    /// How long a sticky `critical` notification waits before retiring to the history panel anyway, in seconds.
    /// `0` restores the unbounded wait.
    ///
    /// A ceiling rather than a second timeout: sticky means "long enough that it cannot be missed", and the
    /// unbounded reading of that has one failure mode with no way out — a card whose only exit is a gesture,
    /// on a shell where the gesture did not land, stays on screen until the shell is restarted. Retiring is not
    /// dismissing, so nothing is lost when it fires: the notification is still in the panel behind the bell.
    pub critical_max_secs: u64,
    pub fullscreen: FullscreenPopups,
    pub group_by_app: bool,
    pub group_preview_num: u32,
    pub action_on_click: bool,
    /// Lines of body a card shows before ellipsizing. Ignored while `open_expanded` is on.
    pub body_lines: u32,
    pub open_expanded: bool,
    pub sound: String,
}

impl Default for NotificationsConfig {
    fn default() -> Self {
        Self {
            critical_sticky: true,
            critical_max_secs: 120,
            fullscreen: FullscreenPopups::default(),
            group_by_app: true,
            group_preview_num: 3,
            action_on_click: true,
            body_lines: 4,
            open_expanded: false,
            sound: String::new(),
        }
    }
}

impl NotificationsConfig {
    /// How long a sticky `critical` popup may stay before retiring to the history, or `None` for the unbounded
    /// wait `critical_max_secs = 0` asks for.
    pub fn critical_ceiling(&self) -> Option<std::time::Duration> {
        (self.critical_max_secs > 0).then(|| std::time::Duration::from_secs(self.critical_max_secs))
    }

    /// How many of a group's cards show before it is expanded. At least one, so a cap of `0` collapses a group
    /// to its header instead of hiding the notifications behind a row that says nothing is there.
    pub fn group_preview(&self) -> usize {
        self.group_preview_num.max(1) as usize
    }

    /// The card's body cap, or `None` when `open_expanded` asks for the whole thing. Clamped so a `0` cannot
    /// render a card with no body at all.
    pub fn body_max_lines(&self) -> Option<u16> {
        (!self.open_expanded).then(|| self.body_lines.clamp(1, 100) as u16)
    }

    /// The command to run when a notification pops, if one is configured.
    pub fn sound_command(&self) -> Option<&str> {
        let command = self.sound.trim();
        (!command.is_empty()).then_some(command)
    }
}

/// Which in-shell toasts to show (`[toasts.events]`).
///
/// One switch per event rather than a single `enabled`, because the useful set is personal: the point of a toast
/// is that it tells you something you would otherwise miss, and a toast about something you already know is
/// noise. Every one is on by default except the two that fire most often.
#[derive(Deserialize, Serialize, Clone, Copy, Debug)]
#[serde(default)]
pub struct ToastEvents {
    pub config_loaded: bool,
    pub charging: bool,
    pub game_mode: bool,
    pub dnd: bool,
    pub audio_output: bool,
    pub audio_input: bool,
    /// Caps Lock and Num Lock — the one piece of state a keyboard changes and never reports.
    pub lock_keys: bool,
    pub kb_layout: bool,
    pub vpn: bool,
    /// Off by default: a music player already says what it is playing, and every skipped track would be a toast.
    pub now_playing: bool,
    pub screenshot: bool,
    pub recording: bool,
}

impl Default for ToastEvents {
    fn default() -> Self {
        Self {
            config_loaded: true,
            charging: true,
            game_mode: true,
            dnd: true,
            audio_output: true,
            audio_input: true,
            lock_keys: true,
            kb_layout: true,
            vpn: true,
            now_playing: false,
            screenshot: false,
            recording: true,
        }
    }
}

/// In-shell toasts (`[toasts]`): the transient messages the shell says about itself.
///
/// Not notifications. A notification belongs to an application, goes into history and waits under
/// Do-Not-Disturb; "Caps Lock is on" is feedback about a key that was just pressed and is worthless a second
/// later. See `shared::services::toaster`.
#[derive(Deserialize, Serialize, Clone, Debug)]
#[serde(default)]
pub struct ToastsConfig {
    pub enabled: bool,
    pub events: ToastEvents,
}

impl Default for ToastsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            events: ToastEvents::default(),
        }
    }
}

impl ToastsConfig {
    /// Whether `event` should reach the screen: toasts on at all, and this event not switched off.
    pub fn allows(&self, event: crate::policy::ToastEvent) -> bool {
        use crate::policy::ToastEvent as Event;
        self.enabled
            && match event {
                Event::ConfigLoaded => self.events.config_loaded,
                Event::Charging => self.events.charging,
                Event::GameMode => self.events.game_mode,
                Event::Dnd => self.events.dnd,
                Event::AudioOutput => self.events.audio_output,
                Event::AudioInput => self.events.audio_input,
                Event::LockKeys => self.events.lock_keys,
                Event::KbLayout => self.events.kb_layout,
                Event::Vpn => self.events.vpn,
                Event::NowPlaying => self.events.now_playing,
                Event::Screenshot => self.events.screenshot,
                Event::Recording => self.events.recording,
            }
    }
}

/// The notification centre (`[sidebar]`): a full-height surface that is the home for the notification history and
/// the quick toggles.
///
/// Distinct from the bell drawer, which is a glance: this is where a user goes to *deal with* what has arrived,
/// so it takes the whole edge, scrolls, and hosts the utilities panel's own toggles rather than a second set.
#[derive(Deserialize, Serialize, Clone, Debug)]
#[serde(default)]
pub struct SidebarConfig {
    pub edge: Edge,
    /// Width for a left/right sidebar, height for a top/bottom one, in px.
    pub size: u32,
    pub show_toggles: bool,
    pub show_history: bool,
}

impl Default for SidebarConfig {
    fn default() -> Self {
        Self {
            edge: Edge::Right,
            size: 400,
            show_toggles: true,
            show_history: true,
        }
    }
}

impl SidebarConfig {
    /// Clamped so a hand-edited `size` cannot produce a sidebar too narrow to read or one that covers the screen.
    pub fn thickness(&self) -> u32 {
        self.size.clamp(240, 1200)
    }
}
