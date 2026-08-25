//! Timed lyrics for whatever is playing.
//!
//! Not a `Service`, for the same reason cover art is not one: this is a *request* keyed by the track, answered once and cached, not a reading of the system that changes under you. So it goes through `shared::asset` — the view asks, gets a signal, and a worker looks on disk and then, if allowed, online. Both of those would stall the frame if they happened where they were asked from.
//!
//! Local files win over the network, always: a `.lrc` next to the track is what the user chose to keep, and it needs no permission, no connection and no waiting.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::time::Duration;

use telar::ReadSignal;

use crate::mpris::Player;
use config::LyricsConfig;
use util::asset::{Load, Loader};
use util::paths;

/// A remote lyric server is a nicety, not a dependency: a slow one must not leave a card waiting for ever.
const FETCH_TIMEOUT: Duration = Duration::from_secs(10);

/// LRCLIB asks callers to identify themselves, which is the whole of its rate-limiting policy.
const USER_AGENT: &str = concat!("hogar-shell/", env!("CARGO_PKG_VERSION"));

/// One line, and when it is sung. `at` is microseconds from the start of the track, which is the unit MPRIS reports its position in — so the comparison the view makes every tick needs no conversion.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Line {
    pub at: i64,
    pub text: String,
}

/// What identifies a track for the purpose of finding its words.
///
/// The player's bus is deliberately not part of it: the same song played by a different program has the same lyrics, and keying on the player would fetch them twice.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Track {
    pub artist: String,
    pub title: String,
    pub album: String,
    /// Seconds, as the online backend's matching wants it. Zero for a stream.
    pub duration: i64,
    /// The track's own file, when it has one — where a sibling `.lrc` would be.
    pub file: Option<PathBuf>,
}

impl Track {
    pub fn of(player: &Player) -> Self {
        Self {
            artist: player.artist.trim().to_string(),
            title: player.title.trim().to_string(),
            album: player.album.trim().to_string(),
            duration: player.length / 1_000_000,
            file: local_file(&player.url),
        }
    }

    /// Whether there is enough here to look anything up. A title is the minimum; without one there is no question.
    pub fn is_searchable(&self) -> bool {
        !self.title.is_empty()
    }
}

/// The local path a `file://` URL names, percent-decoding included.
fn local_file(url: &str) -> Option<PathBuf> {
    let url = url.trim();
    if url.is_empty() {
        return None;
    }
    if let Some(path) = url.strip_prefix('/') {
        let path = PathBuf::from(format!("/{path}"));
        return path.exists().then_some(path);
    }
    let decoded = percent_decode(url.strip_prefix("file://")?.trim_start_matches("localhost"))?;
    let path = PathBuf::from(decoded);
    path.exists().then_some(path)
}

/// Percent-decoding, the same shape cover art needs for the same reason: players emit encoded URLs, and a track in a folder with a space resolves to a path that does not exist unless this runs.
fn percent_decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok()?;
            if let Ok(byte) = u8::from_str_radix(hex, 16) {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).ok()
}

/// Parses an LRC file into timed lines, in order.
///
/// The format is loose in practice, so this reads what players actually write: several timestamps on one line (a chorus, written once and played four times), `[offset:±ms]` for a file that runs early, `mm:ss.xx` with hundredths or `mm:ss.xxx` with milliseconds, and metadata tags (`[ti:]`, `[ar:]`) that are not lines at all. An unsynced file — plain text with no timestamps — yields nothing rather than a wall of untimed words, since a view that highlights the current line has nothing to do with one.
pub fn parse(text: &str) -> Vec<Line> {
    let mut lines: Vec<Line> = Vec::new();
    let mut offset_micros: i64 = 0;
    for raw in text.lines() {
        let raw = raw.trim();
        if !raw.starts_with('[') {
            continue;
        }
        let mut stamps: Vec<i64> = Vec::new();
        let mut rest = raw;
        // `end` is where the `]` sits inside the *stripped* body, so `1..=end` is exactly the tag's own text and `end + 2` is the first character after the bracket.
        while let Some(end) = rest.strip_prefix('[').and_then(|body| body.find(']')) {
            let tag = &rest[1..=end];
            rest = &rest[end + 2..];
            if let Some(micros) = timestamp(tag) {
                stamps.push(micros);
                continue;
            }
            // `[offset:+250]` shifts every line, and is the one metadata tag that changes the timing.
            if let Some(value) = tag.strip_prefix("offset:") {
                if let Ok(ms) = value.trim().trim_start_matches('+').parse::<i64>() {
                    offset_micros = ms * 1000;
                }
                continue;
            }
            // Any other bracketed tag is metadata; the text after it is not a lyric either.
            rest = "";
        }
        if stamps.is_empty() {
            continue;
        }
        let text = rest.trim().to_string();
        for at in stamps {
            lines.push(Line {
                at: at + offset_micros,
                text: text.clone(),
            });
        }
    }
    // Sorted because a repeated chorus writes its stamps out of order by construction, and stable so two lines sharing a timestamp keep the order the file put them in.
    lines.sort_by_key(|line| line.at);
    lines
}

/// `mm:ss.xx`, `mm:ss.xxx` or `mm:ss` as microseconds, or `None` when the tag is not a timestamp.
fn timestamp(tag: &str) -> Option<i64> {
    let (minutes, rest) = tag.split_once(':')?;
    let minutes: i64 = minutes.trim().parse().ok()?;
    let (seconds, fraction) = match rest.split_once(['.', ':']) {
        Some((seconds, fraction)) => (seconds, fraction),
        None => (rest, ""),
    };
    let seconds: i64 = seconds.trim().parse().ok()?;
    let fraction = fraction.trim();
    // The fraction is scaled by how many digits it has, not by a fixed unit: files in the wild write tenths, hundredths and milliseconds in the same field, and reading `.5` as five milliseconds puts a line half a second early.
    let sub_micros = if fraction.is_empty() {
        0
    } else {
        let digits: String = fraction.chars().take(6).collect();
        let scale = 10_i64.pow(6 - digits.len() as u32);
        digits.parse::<i64>().ok()? * scale
    };
    Some((minutes * 60 + seconds) * 1_000_000 + sub_micros)
}

/// Which line is being sung at `position` (microseconds): the last one that has started.
///
/// `None` before the first line, which is the instrumental opening most songs have — a view that highlighted line one through it would be wrong for the first twenty seconds of a lot of music.
pub fn active(lines: &[Line], position: i64) -> Option<usize> {
    lines
        .iter()
        .rposition(|line| line.at <= position)
        .filter(|_| !lines.is_empty())
}

fn settings() -> LyricsConfig {
    config::shared_config()
        .map(|config| config.lyrics.clone())
        .unwrap_or_default()
}

/// Where hand-kept `.lrc` files live: `[paths] lyrics`, resolved the same way every other folder the shell owns is.
fn library_dir() -> PathBuf {
    config::shared_config()
        .map(|config| config.lyrics_dir())
        .unwrap_or_else(|| paths::data_dir().join("lyrics"))
}

type Store = Loader<Track, Vec<Line>>;

thread_local! {
    static LYRICS: RefCell<Option<Store>> = const { RefCell::new(None) };
}

/// The lyrics for `player`'s track, looking them up the first time they are asked for.
pub fn of(player: &Player) -> ReadSignal<Load<Vec<Line>>> {
    let track = Track::of(player);
    if !track.is_searchable() || !settings().enabled {
        return telar::signal(Load::Missing).read_only();
    }
    ensure_store();
    LYRICS.with(|store| {
        let borrow = store.borrow();
        let Some(store) = borrow.as_ref() else {
            return telar::signal(Load::Missing).read_only();
        };
        // `at_hand` is deliberately nothing: even the local path involves reading a directory, and a track change must not do that on the frame that draws it.
        store.get(track, |_| None)
    })
}

fn ensure_store() {
    if LYRICS.with(|store| store.borrow().is_some()) {
        return;
    }
    let store = Loader::new(|track: &Track| find(track));
    LYRICS.with(|cell| *cell.borrow_mut() = Some(store));
}

/// The words for `track`. Blocking — only ever called on the worker thread.
fn find(track: &Track) -> Option<Vec<Line>> {
    let config = settings();
    if let Some(path) = local_lyrics(track, &library_dir())
        && let Ok(text) = std::fs::read_to_string(&path)
    {
        let lines = parse(&text);
        if !lines.is_empty() {
            return Some(lines);
        }
        tracing::debug!("{} has no timed lines", path.display());
    }
    if !config.online {
        return None;
    }
    let lines = fetch_online(track)?;
    (!lines.is_empty()).then_some(lines)
}

/// The `.lrc` for `track`, looked for where a person would have put it.
///
/// Next to the audio file first — that is where every tagger and every download writes it — then in the configured library under the two names a human would choose. The library scan is a last resort and case-insensitive, because "Artist - Title.lrc" is a name typed by hand.
fn local_lyrics(track: &Track, library: &Path) -> Option<PathBuf> {
    if let Some(file) = &track.file {
        let sibling = file.with_extension("lrc");
        if sibling.exists() {
            return Some(sibling);
        }
    }
    for name in candidate_names(track) {
        let direct = library.join(format!("{name}.lrc"));
        if direct.exists() {
            return Some(direct);
        }
    }
    let wanted: Vec<String> = candidate_names(track)
        .into_iter()
        .map(|name| name.to_lowercase())
        .collect();
    let entries = std::fs::read_dir(library).ok()?;
    entries.flatten().find_map(|entry| {
        let path = entry.path();
        if path
            .extension()
            .and_then(|ext| ext.to_str())?
            .to_lowercase()
            != "lrc"
        {
            return None;
        }
        let stem = path.file_stem()?.to_str()?.to_lowercase();
        wanted.contains(&stem).then_some(path)
    })
}

/// The file names a track could reasonably have been saved under.
fn candidate_names(track: &Track) -> Vec<String> {
    let mut names = Vec::new();
    if !track.artist.is_empty() {
        names.push(sanitise(&format!("{} - {}", track.artist, track.title)));
    }
    names.push(sanitise(&track.title));
    names
}

/// A file name cannot hold a separator, and a track title can.
fn sanitise(name: &str) -> String {
    name.replace(['/', '\\'], "_").trim().to_string()
}

/// Asks LRCLIB for synced lyrics.
///
/// A public, free, no-key service whose whole purpose is this question, and whose answer is one JSON object — so this needs no client library, just the request and the one field that matters. A miss is a 404, which is an answer rather than an error.
fn fetch_online(track: &Track) -> Option<Vec<Line>> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(FETCH_TIMEOUT))
        .user_agent(USER_AGENT)
        .build()
        .into();
    let mut request = agent
        .get("https://lrclib.net/api/get")
        .query("track_name", &track.title)
        .query("artist_name", &track.artist);
    if !track.album.is_empty() {
        request = request.query("album_name", &track.album);
    }
    if track.duration > 0 {
        request = request.query("duration", track.duration.to_string());
    }
    let mut response = request.call().ok()?;
    let body = response.body_mut().read_to_string().ok()?;
    let parsed: serde_json::Value = serde_json::from_str(&body).ok()?;
    let synced = parsed.get("syncedLyrics")?.as_str()?;
    Some(parse(synced))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
[ti:Test Song]
[ar:Someone]
[00:12.00]First line
[00:17.20]Second line
[00:21.10]Third line
";

    #[test]
    fn a_timestamped_file_parses_to_lines_in_order() {
        let lines = parse(SAMPLE);
        assert_eq!(lines.len(), 3, "the metadata tags are not lines");
        assert_eq!(lines[0].at, 12_000_000);
        assert_eq!(lines[0].text, "First line");
        assert_eq!(lines[1].at, 17_200_000);
        assert_eq!(lines[2].at, 21_100_000);
    }

    #[test]
    fn every_precision_the_format_is_written_in_reads_the_same() {
        assert_eq!(timestamp("00:12.00"), Some(12_000_000));
        assert_eq!(
            timestamp("00:12.5"),
            Some(12_500_000),
            "one digit is tenths of a second"
        );
        assert_eq!(
            timestamp("00:12.50"),
            Some(12_500_000),
            "two digits are hundredths"
        );
        assert_eq!(
            timestamp("00:12.500"),
            Some(12_500_000),
            "three are milliseconds"
        );
        assert_eq!(
            timestamp("01:00"),
            Some(60_000_000),
            "and a whole second needs no fraction"
        );
        assert_eq!(timestamp("02:03.45"), Some(123_450_000));
        assert_eq!(timestamp("ti:Title"), None);
        assert_eq!(timestamp("offset:+250"), None);
        assert_eq!(timestamp(""), None);
    }

    /// A chorus is written once with every time it is sung, which is also the only way lines arrive out of order.
    #[test]
    fn a_line_with_several_timestamps_appears_at_each_of_them() {
        let lines = parse("[00:40.00][01:20.00][00:10.00]Chorus\n[00:30.00]Verse");
        let times: Vec<i64> = lines.iter().map(|line| line.at).collect();
        assert_eq!(
            times,
            vec![10_000_000, 30_000_000, 40_000_000, 80_000_000],
            "sorted, whatever order the file listed them in"
        );
        assert_eq!(lines[0].text, "Chorus");
        assert_eq!(lines[1].text, "Verse");
        assert_eq!(lines[3].text, "Chorus");
    }

    #[test]
    fn an_offset_tag_shifts_every_line() {
        let lines = parse("[offset:+500]\n[00:10.00]Late\n[00:20.00]Later");
        assert_eq!(lines[0].at, 10_500_000);
        assert_eq!(lines[1].at, 20_500_000);

        let early = parse("[offset:-250]\n[00:10.00]Early");
        assert_eq!(early[0].at, 9_750_000);
    }

    #[test]
    fn a_file_with_no_timings_is_not_lyrics_this_view_can_use() {
        // Plain text, which is what an unsynced download looks like. A view that highlights the current line has nothing to do with it, and half a screen of untimed words would read as a bug.
        assert!(parse("First line\nSecond line").is_empty());
        assert!(parse("").is_empty());
        assert!(parse("[ti:Only metadata]\n[ar:Nobody]").is_empty());
    }

    #[test]
    fn an_empty_line_keeps_its_timing() {
        // A gap between verses is written as a timestamp with nothing after it, and dropping it would make the previous line stay highlighted through the instrumental break.
        let lines = parse("[00:10.00]Words\n[00:14.00]\n[00:20.00]More");
        assert_eq!(lines.len(), 3);
        assert!(lines[1].text.is_empty());
    }

    #[test]
    fn the_active_line_is_the_last_one_that_has_started() {
        let lines = parse(SAMPLE);
        assert_eq!(active(&lines, 0), None, "the intro belongs to no line");
        assert_eq!(active(&lines, 11_999_999), None);
        assert_eq!(
            active(&lines, 12_000_000),
            Some(0),
            "exactly on the beat is on the line"
        );
        assert_eq!(active(&lines, 15_000_000), Some(0));
        assert_eq!(active(&lines, 17_200_000), Some(1));
        assert_eq!(
            active(&lines, 99_000_000),
            Some(2),
            "the last line holds to the end"
        );
        assert_eq!(active(&[], 5), None);
    }

    #[test]
    fn a_track_is_identified_by_the_song_and_not_by_the_player() {
        let player = Player {
            title: " So What ".to_string(),
            artist: "Miles Davis".to_string(),
            album: "Kind of Blue".to_string(),
            length: 545_000_000,
            bus: "spotify".to_string(),
            ..Player::default()
        };
        let track = Track::of(&player);
        assert_eq!(
            track.title, "So What",
            "trimmed, because tags are typed by hand"
        );
        assert_eq!(
            track.duration, 545,
            "seconds, which is what the backend matches on"
        );
        assert!(track.is_searchable());

        let same_song_elsewhere = Player {
            bus: "mpv".to_string(),
            ..player
        };
        assert_eq!(
            Track::of(&same_song_elsewhere),
            track,
            "the same song in another player must not fetch twice"
        );

        assert!(
            !Track::of(&Player::default()).is_searchable(),
            "no title, no question"
        );
    }

    #[test]
    fn the_file_names_a_track_might_be_saved_under_include_the_bare_title() {
        let track = Track {
            artist: "Miles Davis".to_string(),
            title: "So What".to_string(),
            ..Track::default()
        };
        assert_eq!(
            candidate_names(&track),
            vec!["Miles Davis - So What".to_string(), "So What".to_string()]
        );
        // A separator in a title is not a directory.
        let slashed = Track {
            title: "AC/DC Medley".to_string(),
            ..Track::default()
        };
        assert_eq!(candidate_names(&slashed), vec!["AC_DC Medley".to_string()]);
    }

    #[test]
    fn a_local_file_url_is_decoded_and_checked() {
        assert_eq!(local_file(""), None);
        assert_eq!(local_file("https://stream.test/live.mp3"), None);
        assert_eq!(
            local_file("file:///nonexistent-track-9e3f.flac"),
            None,
            "a path the player named but that is not there"
        );

        let dir = std::env::temp_dir().join(format!("hogar-shell-lyrics-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let track = dir.join("My Track.flac");
        std::fs::write(&track, b"audio").unwrap();
        let url = format!("file://{}", track.display().to_string().replace(' ', "%20"));
        assert_eq!(local_file(&url), Some(track.clone()));

        // The sibling `.lrc` is the first place to look, and the reason the URL is parsed at all.
        let words = dir.join("My Track.lrc");
        std::fs::write(&words, "[00:01.00]Hello").unwrap();
        let found = local_lyrics(
            &Track {
                title: "My Track".to_string(),
                file: Some(track),
                ..Track::default()
            },
            Path::new("/nonexistent-lyrics-library"),
        );
        assert_eq!(found, Some(words));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_hand_kept_library_is_searched_by_name_whatever_its_case() {
        let dir = std::env::temp_dir().join(format!("hogar-shell-lyrlib-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("miles davis - so what.LRC"), "[00:01.00]x").unwrap();

        let track = Track {
            artist: "Miles Davis".to_string(),
            title: "So What".to_string(),
            ..Track::default()
        };
        assert!(
            local_lyrics(&track, &dir).is_some(),
            "a name typed by hand is not typed in the same case"
        );
        assert!(
            local_lyrics(
                &Track {
                    title: "Nothing Here".to_string(),
                    ..Track::default()
                },
                &dir
            )
            .is_none()
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
