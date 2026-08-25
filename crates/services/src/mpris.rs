//! Whatever is playing, and the controls for it.
//!
//! MPRIS is a per-application interface — every player owns a `org.mpris.MediaPlayer2.<app>` bus name — so the shell's job is to pick one and present it as "the" player. Selection is: the user's configured preference if it is running, else the first one actually playing, else the first that exists. That ordering is what makes a media chip feel right when a browser tab and a music player are both alive.
//!
//! Position is deliberately *not* tracked here. It advances continuously, so publishing it would wake every subscribed surface many times a second for a value only a progress bar cares about; a consumer that wants it calls [`position`] on its own cadence.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use platform_wayland::EventSender;
use zbus::blocking::{Connection, MessageIterator, fdo::DBusProxy, fdo::PropertiesProxy};
use zbus::message::Type as MessageType;
use zbus::names::BusName;
use zbus::zvariant::{ObjectPath, Value};

use util::broadcast::{Broadcast, Service};

const MPRIS_PREFIX: &str = "org.mpris.MediaPlayer2.";
const MPRIS_PATH: &str = "/org/mpris/MediaPlayer2";
const PLAYER_IFACE: &str = "org.mpris.MediaPlayer2.Player";

/// A player's transport state, as MPRIS reports it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Playback {
    Playing,
    Paused,
    #[default]
    Stopped,
}

impl Playback {
    fn parse(status: &str) -> Self {
        match status {
            "Playing" => Self::Playing,
            "Paused" => Self::Paused,
            _ => Self::Stopped,
        }
    }

    pub fn is_playing(self) -> bool {
        self == Self::Playing
    }
}

/// What the shell shows and controls. `bus` identifies the player for the control calls; everything else is display.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Player {
    pub bus: String,
    /// The player's own name (`Spotify`), aliased through config where one is set.
    pub identity: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    /// Cover art URL as the player gave it — usually `file://` or `https://`.
    pub art_url: String,
    /// The track's own URL (`xesam:url`), which for a local file is where its `.lrc` lives next to it. Empty for a stream, and for the players that simply do not report one.
    pub url: String,
    /// Track length in microseconds; 0 when the player doesn't report one (a live stream).
    pub length: i64,
    pub playback: Playback,
    pub can_go_next: bool,
    pub can_go_previous: bool,
    /// Whether the player accepts `Seek` at all. A live stream and most browser tabs do not, and offering a scrub that silently does nothing is worse than not offering one.
    pub can_seek: bool,
    pub shuffle: bool,
    pub loop_status: LoopStatus,
}

/// MPRIS's `LoopStatus`. The variant is `Off` rather than `None` so it never reads as an absent value at a call site — this is a state the player is in, not a missing one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LoopStatus {
    #[default]
    Off,
    Track,
    Playlist,
}

impl LoopStatus {
    fn parse(status: &str) -> Self {
        match status {
            "Track" => Self::Track,
            "Playlist" => Self::Playlist,
            _ => Self::Off,
        }
    }

    /// The string MPRIS expects back.
    pub fn as_mpris(self) -> &'static str {
        match self {
            Self::Off => "None",
            Self::Track => "Track",
            Self::Playlist => "Playlist",
        }
    }

    /// What pressing a single loop button does: off → the whole playlist → this track → off. That order is what every player's own button does, so the shell's matches muscle memory.
    pub fn next(self) -> Self {
        match self {
            Self::Off => Self::Playlist,
            Self::Playlist => Self::Track,
            Self::Track => Self::Off,
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Track => "track",
            Self::Playlist => "playlist",
        }
    }
}

impl Player {
    pub fn is_empty(&self) -> bool {
        self.bus.is_empty()
    }

    /// A single line for a bar chip: `artist — title`, or just the title when there is no artist.
    pub fn summary(&self) -> String {
        match (self.artist.is_empty(), self.title.is_empty()) {
            (_, true) => self.identity.clone(),
            (true, false) => self.title.clone(),
            (false, false) => format!("{} — {}", self.artist, self.title),
        }
    }
}

/// The bus name suffix (`org.mpris.MediaPlayer2.spotify` → `spotify`).
fn short_name(bus: &str) -> &str {
    bus.strip_prefix(MPRIS_PREFIX).unwrap_or(bus)
}

/// The stable key config matches on, with the volatile part of the bus name removed.
///
/// The MPRIS spec lets a player that can run more than once append `.instanceNNNN`, so a browser shows up as `org.mpris.MediaPlayer2.chromium.instance4489` — with a PID that changes every launch. Keying config on the raw suffix would mean `preferred_player` and `[media.aliases]` silently stop matching after a restart, so both use this instead.
fn config_key(bus: &str) -> &str {
    let short = short_name(bus);
    match short.split_once(".instance") {
        Some((base, rest)) if rest.chars().all(|c| c.is_ascii_digit()) => base,
        _ => short,
    }
}

/// Picks the player to present, given every running one.
///
/// A preference only wins if that player is actually running, so configuring `Spotify` doesn't leave the chip blank when Spotify is closed. Otherwise something playing beats something paused — the thing making noise is the thing the user means — and a stable fallback keeps the chip from flickering between two idle players.
fn choose<'a>(players: &'a [Player], preferred: &str) -> Option<&'a Player> {
    let preferred = preferred.trim();
    if !preferred.is_empty()
        && let Some(hit) = players
            .iter()
            .find(|p| config_key(&p.bus).eq_ignore_ascii_case(preferred))
    {
        return Some(hit);
    }
    players
        .iter()
        .find(|p| p.playback.is_playing())
        .or_else(|| players.first())
}

/// Every MPRIS bus name currently on the session bus, sorted so the fallback pick is stable across polls.
fn player_names(conn: &Connection) -> Vec<String> {
    let Ok(dbus) = DBusProxy::new(conn) else {
        return Vec::new();
    };
    let Ok(names) = dbus.list_names() else {
        return Vec::new();
    };
    let mut names: Vec<String> = names
        .into_iter()
        .map(|n| n.to_string())
        .filter(|n| n.starts_with(MPRIS_PREFIX))
        .collect();
    names.sort();
    names
}

fn props_for(conn: &Connection, bus: &str) -> Option<PropertiesProxy<'static>> {
    let name = BusName::try_from(bus.to_string()).ok()?;
    let path = ObjectPath::try_from(MPRIS_PATH).ok()?;
    PropertiesProxy::builder(conn)
        .destination(name)
        .ok()?
        .path(path)
        .ok()?
        .build()
        .ok()
}

/// Pulls a string out of the `Metadata` dictionary. `xesam:artist` is a list; the first entry is the one a one-line chip has room for.
fn meta_string(metadata: &HashMap<String, Value<'_>>, key: &str) -> String {
    let Some(value) = metadata.get(key) else {
        return String::new();
    };
    match value {
        Value::Str(s) => s.to_string(),
        Value::Array(list) => list
            .iter()
            .find_map(|v| match v {
                Value::Str(s) => Some(s.to_string()),
                _ => None,
            })
            .unwrap_or_default(),
        _ => String::new(),
    }
}

fn meta_i64(metadata: &HashMap<String, Value<'_>>, key: &str) -> i64 {
    metadata
        .get(key)
        .and_then(|v| i64::try_from(v.try_clone().ok()?).ok())
        .unwrap_or(0)
}

fn read_player(conn: &Connection, bus: &str) -> Option<Player> {
    let props = props_for(conn, bus)?;
    // The interface name is rebuilt per call rather than captured: `InterfaceName` is not `Copy`, and a closure holding one is `FnOnce`, which a multi-property read cannot use.
    let get = |name: &str| props.get(PLAYER_IFACE.try_into().ok()?, name).ok();

    let metadata: HashMap<String, Value> = get("Metadata")
        .and_then(|v| HashMap::try_from(v).ok())
        .unwrap_or_default();
    let status = get("PlaybackStatus")
        .and_then(|v| String::try_from(v).ok())
        .unwrap_or_default();
    let identity = props
        .get("org.mpris.MediaPlayer2".try_into().ok()?, "Identity")
        .ok()
        .and_then(|v| String::try_from(v).ok())
        .unwrap_or_else(|| config_key(bus).to_string());

    Some(Player {
        bus: bus.to_string(),
        identity,
        title: meta_string(&metadata, "xesam:title"),
        artist: meta_string(&metadata, "xesam:artist"),
        album: meta_string(&metadata, "xesam:album"),
        art_url: meta_string(&metadata, "mpris:artUrl"),
        url: meta_string(&metadata, "xesam:url"),
        length: meta_i64(&metadata, "mpris:length"),
        playback: Playback::parse(&status),
        can_go_next: get("CanGoNext")
            .and_then(|v| bool::try_from(v).ok())
            .unwrap_or(false),
        can_go_previous: get("CanGoPrevious")
            .and_then(|v| bool::try_from(v).ok())
            .unwrap_or(false),
        can_seek: get("CanSeek")
            .and_then(|v| bool::try_from(v).ok())
            .unwrap_or(false),
        // Both are optional in the spec, and a player that implements neither reports the defaults rather than failing the whole read.
        shuffle: get("Shuffle")
            .and_then(|v| bool::try_from(v).ok())
            .unwrap_or(false),
        loop_status: get("LoopStatus")
            .and_then(|v| String::try_from(v).ok())
            .map(|s| LoopStatus::parse(&s))
            .unwrap_or_default(),
    })
}

/// The active player right now, or the empty value when nothing is running.
fn read_active(conn: &Connection) -> Player {
    let players: Vec<Player> = player_names(conn)
        .iter()
        .filter_map(|bus| read_player(conn, bus))
        .collect();
    let preferred = config::shared_config()
        .map(|c| c.media.preferred_player.clone())
        .unwrap_or_default();
    let mut chosen = choose(&players, &preferred).cloned().unwrap_or_default();
    chosen.identity = alias_for(&chosen);
    chosen
}

/// The display name for a player: its `[media.aliases]` entry keyed by bus suffix, else its own `Identity`. Players name themselves badly often enough (`com.github.th_ch.youtube_music`) that overriding is worth a config key.
fn alias_for(player: &Player) -> String {
    if player.is_empty() {
        return String::new();
    }
    config::shared_config()
        .and_then(|c| c.media.aliases.get(config_key(&player.bus)).cloned())
        .unwrap_or_else(|| player.identity.clone())
}

/// Poll interval for the fallback path, used only when the session bus can't be watched at all.
const RESCAN: Duration = Duration::from_secs(3);

/// A player that stops answering must not park a shell thread for the life of the process. Generous, because this bounds a browser's reply to a property read, not a user-facing wait.
const METHOD_TIMEOUT: Duration = Duration::from_secs(5);

/// The connection every read and every control call goes through.
fn session() -> Option<Connection> {
    crate::bus::session(Some(METHOD_TIMEOUT))
}

static MPRIS: Service<Player> = Service::new("hogar-shell-mpris", run);

fn run(out: &Arc<Broadcast<Player>>) {
    let Some(reads) = session() else {
        tracing::info!("no session bus; media control is unavailable");
        return;
    };
    out.publish(read_active(&reads));
    let watched = crate::bus::private_session(Some(METHOD_TIMEOUT))
        .and_then(|watch| watch_bus(out, &watch, &reads));
    if watched.is_none() {
        poll_fallback(out, &reads);
    }
}

/// Wakes on either signal that can change what the chip should show, from **one** iterator.
///
/// Watching only the active player's `PropertiesChanged` is not enough, and getting that wrong is what made a newly-opened player go unnoticed: the loop parks on the *current* player's signals, so a second player starting produces nothing to wake it, and the chip keeps showing the old one until the old one happens to change something. `NameOwnerChanged` over the `org.mpris.MediaPlayer2` namespace is the event for "a player appeared or went away", so both are matched here and either one triggers a re-read.
///
/// `watch` is only drained, never called on — the reads go to `reads`. Both on one connection deadlocks: zbus queues every message for the stream and its socket reader stops once that queue is full, so a blocking call waits on a reply sitting behind a queue only this loop drains, from inside the call. That froze the chip on whatever was playing, controls included, until the shell was restarted.
///
/// Returns `None` when the watch cannot be set up and again once the stream ends, so a connection that dies under the loop falls through to polling rather than freezing the chip by another route.
fn watch_bus(out: &Broadcast<Player>, watch: &Connection, reads: &Connection) -> Option<()> {
    let properties = zbus::MatchRule::builder()
        .msg_type(MessageType::Signal)
        .interface("org.freedesktop.DBus.Properties")
        .ok()?
        .member("PropertiesChanged")
        .ok()?
        .path(MPRIS_PATH)
        .ok()?
        .build();
    // `arg0namespace` limits the name traffic to MPRIS buses instead of every name change on the session bus.
    let ownership = zbus::MatchRule::builder()
        .msg_type(MessageType::Signal)
        .sender("org.freedesktop.DBus")
        .ok()?
        .interface("org.freedesktop.DBus")
        .ok()?
        .member("NameOwnerChanged")
        .ok()?
        .arg0ns("org.mpris.MediaPlayer2")
        .ok()?
        .build();

    let dbus = DBusProxy::new(watch).ok()?;
    dbus.add_match_rule(properties).ok()?;
    dbus.add_match_rule(ownership).ok()?;
    // Every message this connection receives, rather than one rule's: `for_match_rule` builds an iterator that *filters* to its own rule, so the ownership signals reached the socket and were then dropped on the floor — which is why a player quitting went unnoticed and its track stayed on the bar and the dashboard until something else happened to change.
    let signals = MessageIterator::from(watch);

    let mut last = read_active(reads);
    for message in signals.flatten() {
        // The `AddMatch` replies above arrive here too, and re-reading on a reply to a call this loop made is work for nothing. The two rules registered above only ever deliver signals.
        if message.message_type() != MessageType::Signal {
            continue;
        }
        // A player's position and metadata churn while a track runs; only a reading that actually differs is worth waking every subscribed surface for.
        let current = read_active(reads);
        if current != last {
            last = current.clone();
            out.publish(current);
        }
    }
    None
}

/// Belt-and-suspenders when the bus cannot be watched, or stops being watchable: a plain re-scan.
fn poll_fallback(out: &Broadcast<Player>, conn: &Connection) {
    let mut last = read_active(conn);
    while out.wanted() {
        std::thread::sleep(RESCAN);
        let current = read_active(conn);
        if current != last {
            last = current.clone();
            out.publish(current);
        }
    }
}

pub fn subscribe(tx: EventSender<Player>) {
    MPRIS.subscribe(tx);
}

/// The last known player, without touching the bus — what a click handler acts on.
pub fn current() -> Option<Player> {
    MPRIS.current().filter(|p| !p.is_empty())
}

/// Calls a `Player` method on the active player, off the UI thread: a D-Bus round-trip in a click handler would stall the frame. The reading that follows arrives through the producer's own watch.
fn control(method: &'static str) {
    let Some(player) = current() else { return };
    let _ = std::thread::Builder::new()
        .name("hogar-shell-mpris-call".to_string())
        .spawn(move || {
            let Some(conn) = session() else {
                return;
            };
            let Ok(name) = BusName::try_from(player.bus.clone()) else {
                return;
            };
            if let Err(e) =
                conn.call_method(Some(name), MPRIS_PATH, Some(PLAYER_IFACE), method, &())
            {
                tracing::warn!("mpris {method} on {}: {e}", player.bus);
            }
        });
}

pub fn play_pause() {
    control("PlayPause");
}

pub fn next() {
    control("Next");
}

pub fn previous() {
    control("Previous");
}

pub fn stop() {
    control("Stop");
}

/// Runs a `Player` method that takes arguments, off the UI thread like [`control`].
fn call_with<A>(method: &'static str, args: A)
where
    A: serde::Serialize + zbus::zvariant::DynamicType + Send + 'static,
{
    let Some(player) = current() else { return };
    let _ = std::thread::Builder::new()
        .name("hogar-shell-mpris-call".to_string())
        .spawn(move || {
            let Some(conn) = session() else {
                return;
            };
            let Ok(name) = BusName::try_from(player.bus.clone()) else {
                return;
            };
            if let Err(e) =
                conn.call_method(Some(name), MPRIS_PATH, Some(PLAYER_IFACE), method, &args)
            {
                tracing::warn!("mpris {method} on {}: {e}", player.bus);
            }
        });
}

/// Sets a `Player` property, off the UI thread. Shuffle and loop are properties, not methods — MPRIS models them as state you assign rather than as verbs.
fn set_property(name: &'static str, value: Value<'static>) {
    let Some(player) = current() else { return };
    let _ = std::thread::Builder::new()
        .name("hogar-shell-mpris-set".to_string())
        .spawn(move || {
            let Some(conn) = session() else {
                return;
            };
            let Ok(bus) = BusName::try_from(player.bus.clone()) else {
                return;
            };
            if let Err(e) = conn.call_method(
                Some(bus),
                MPRIS_PATH,
                Some("org.freedesktop.DBus.Properties"),
                "Set",
                &(PLAYER_IFACE, name, value),
            ) {
                tracing::warn!("mpris set {name} on {}: {e}", player.bus);
            }
        });
}

/// Moves the playhead by `offset` microseconds, forward or back.
///
/// Relative `Seek` rather than absolute `SetPosition`, because the absolute form takes the track id from the metadata and refuses the call when it does not match — which is exactly the race a scrub hits when the track changes underneath it. Relative seeking clamps at both ends in every player.
pub fn seek(offset_micros: i64) {
    if current().is_some_and(|p| !p.can_seek) {
        return;
    }
    call_with("Seek", (offset_micros,));
}

pub fn set_shuffle(on: bool) {
    set_property("Shuffle", Value::Bool(on));
}

pub fn toggle_shuffle() {
    if let Some(player) = current() {
        set_shuffle(!player.shuffle);
    }
}

pub fn set_loop(status: LoopStatus) {
    set_property("LoopStatus", Value::from(status.as_mpris()));
}

/// Advances the loop mode one step, which is what a single button does.
pub fn cycle_loop() {
    if let Some(player) = current() {
        set_loop(player.loop_status.next());
    }
}

/// The active player's position in microseconds. Read on demand rather than broadcast: it advances continuously, and publishing it would wake every subscriber many times a second.
pub fn position() -> Option<i64> {
    let player = current()?;
    let conn = session()?;
    let props = props_for(&conn, &player.bus)?;
    let value = props.get(PLAYER_IFACE.try_into().ok()?, "Position").ok()?;
    i64::try_from(value).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn player(bus: &str, playback: Playback) -> Player {
        Player {
            bus: format!("{MPRIS_PREFIX}{bus}"),
            identity: bus.to_string(),
            playback,
            ..Player::default()
        }
    }

    #[test]
    fn short_name_strips_the_mpris_prefix() {
        assert_eq!(short_name("org.mpris.MediaPlayer2.spotify"), "spotify");
        assert_eq!(short_name("something.else"), "something.else");
    }

    #[test]
    fn the_config_key_drops_the_volatile_instance_suffix() {
        // A browser's bus name carries its PID, which changes every launch; keying config on the raw suffix would make `preferred_player` and the aliases stop matching after a restart.
        assert_eq!(
            config_key("org.mpris.MediaPlayer2.chromium.instance4489"),
            "chromium"
        );
        assert_eq!(config_key("org.mpris.MediaPlayer2.spotify"), "spotify");
        assert_eq!(
            config_key("org.mpris.MediaPlayer2.my.instanceplayer"),
            "my.instanceplayer",
            "only a numeric instance suffix is volatile"
        );
    }

    #[test]
    fn a_preference_matches_a_player_that_carries_an_instance_suffix() {
        let players = [Player {
            bus: "org.mpris.MediaPlayer2.chromium.instance4489".to_string(),
            playback: Playback::Paused,
            ..Player::default()
        }];
        assert!(
            choose(&players, "chromium").is_some(),
            "the user configures 'chromium', not 'chromium.instance4489'"
        );
    }

    #[test]
    fn a_running_preference_wins_over_everything() {
        let players = [
            player("firefox", Playback::Playing),
            player("spotify", Playback::Paused),
        ];
        let chosen = choose(&players, "spotify").unwrap();
        assert_eq!(short_name(&chosen.bus), "spotify");
        assert!(
            choose(&players, "SPOTIFY").is_some_and(|p| short_name(&p.bus) == "spotify"),
            "the match is case-insensitive, since config is hand-written"
        );
    }

    #[test]
    fn a_preference_that_is_not_running_falls_through_to_whatever_plays() {
        let players = [
            player("firefox", Playback::Paused),
            player("mpv", Playback::Playing),
        ];
        let chosen = choose(&players, "spotify").expect("something is still chosen");
        assert_eq!(
            short_name(&chosen.bus),
            "mpv",
            "configuring an absent player must not blank the chip"
        );
    }

    #[test]
    fn playing_beats_paused_and_the_fallback_is_stable() {
        let players = [
            player("firefox", Playback::Paused),
            player("mpv", Playback::Playing),
        ];
        assert_eq!(short_name(&choose(&players, "").unwrap().bus), "mpv");

        let idle = [
            player("firefox", Playback::Paused),
            player("mpv", Playback::Stopped),
        ];
        assert_eq!(
            short_name(&choose(&idle, "").unwrap().bus),
            "firefox",
            "with nothing playing the pick is the first, so the chip does not flicker"
        );
        assert!(choose(&[], "").is_none());
    }

    #[test]
    fn summary_degrades_from_artist_and_title_down_to_the_player_name() {
        let mut p = player("spotify", Playback::Playing);
        p.identity = "Spotify".to_string();
        assert_eq!(p.summary(), "Spotify", "no title yet: name the player");

        p.title = "Blue in Green".to_string();
        assert_eq!(p.summary(), "Blue in Green");

        p.artist = "Miles Davis".to_string();
        assert_eq!(p.summary(), "Miles Davis — Blue in Green");
    }

    #[test]
    fn metadata_reads_the_first_artist_out_of_the_list() {
        let mut metadata: HashMap<String, Value> = HashMap::new();
        metadata.insert("xesam:title".to_string(), Value::from("So What"));
        metadata.insert(
            "xesam:artist".to_string(),
            Value::from(vec!["Miles Davis", "John Coltrane"]),
        );
        metadata.insert("mpris:length".to_string(), Value::from(545_000_000i64));

        assert_eq!(meta_string(&metadata, "xesam:title"), "So What");
        assert_eq!(meta_string(&metadata, "xesam:artist"), "Miles Davis");
        assert_eq!(meta_i64(&metadata, "mpris:length"), 545_000_000);
        assert_eq!(
            meta_string(&metadata, "xesam:album"),
            "",
            "a missing key is empty, not a panic"
        );
    }

    #[test]
    fn playback_parses_the_three_mpris_states() {
        assert_eq!(Playback::parse("Playing"), Playback::Playing);
        assert_eq!(Playback::parse("Paused"), Playback::Paused);
        assert_eq!(Playback::parse("Stopped"), Playback::Stopped);
        assert_eq!(
            Playback::parse("nonsense"),
            Playback::Stopped,
            "an unknown status is not playing"
        );
    }
}
