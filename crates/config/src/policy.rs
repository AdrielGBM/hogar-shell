//! The vocabulary `Config` needs to express a policy about something a service produces.
//!
//! Each of these types is named by a config section — `[notifications]` gates on an urgency, `[toasts.events]` on a toast event, `[weather]` resolves to a coordinate — so they belong to the config rather than to the producer. The service that raises them re-exports the type from here, which is what keeps the dependency pointing one way: a service knows about the config, the config knows nothing about a service.

/// How insistent a notification is, as delivered in the freedesktop `urgency` hint.
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Urgency {
    Low,
    Normal,
    Critical,
}

/// What a toast is about. Each one is a switch in `[toasts.events]`, so a user who wants to know about their VPN and not about their keyboard layout can have exactly that.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToastEvent {
    ConfigLoaded,
    Charging,
    GameMode,
    Dnd,
    AudioOutput,
    AudioInput,
    LockKeys,
    KbLayout,
    Vpn,
    NowPlaying,
    Screenshot,
    Recording,
}

impl ToastEvent {
    pub const ALL: [ToastEvent; 12] = [
        ToastEvent::ConfigLoaded,
        ToastEvent::Charging,
        ToastEvent::GameMode,
        ToastEvent::Dnd,
        ToastEvent::AudioOutput,
        ToastEvent::AudioInput,
        ToastEvent::LockKeys,
        ToastEvent::KbLayout,
        ToastEvent::Vpn,
        ToastEvent::NowPlaying,
        ToastEvent::Screenshot,
        ToastEvent::Recording,
    ];

    pub fn id(self) -> &'static str {
        match self {
            ToastEvent::ConfigLoaded => "config_loaded",
            ToastEvent::Charging => "charging",
            ToastEvent::GameMode => "game_mode",
            ToastEvent::Dnd => "dnd",
            ToastEvent::AudioOutput => "audio_output",
            ToastEvent::AudioInput => "audio_input",
            ToastEvent::LockKeys => "lock_keys",
            ToastEvent::KbLayout => "kb_layout",
            ToastEvent::Vpn => "vpn",
            ToastEvent::NowPlaying => "now_playing",
            ToastEvent::Screenshot => "screenshot",
            ToastEvent::Recording => "recording",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|event| event.id() == id.trim())
    }
}

/// Where on earth `[weather]` is asking about.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Coordinates {
    pub latitude: f32,
    pub longitude: f32,
}
