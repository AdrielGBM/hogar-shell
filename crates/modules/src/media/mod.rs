//! What is playing, on the bar.

use config::{MediaConfig, MediaScroll};
use services::mpris::{self, Playback, Player};

/// The glyph for a player's transport state. Stopped and absent look the same on purpose: a chip that shows a pause symbol for a player that isn't running would invite a click that does nothing.
pub fn glyph(player: &Player) -> &'static str {
    match player.playback {
        Playback::Playing => "pause",
        Playback::Paused => "play",
        Playback::Stopped => "music",
    }
}

/// The gap drawn between the end of a scrolling title and its own start, so the wrap reads as a loop rather than as the words running into each other.
const MARQUEE_GAP: &str = "   ·   ";

/// The chip's text: `artist — title`, whole. Empty when nothing is running, which lets the module collapse to just its icon instead of showing a placeholder.
///
/// Nothing trims it here. The chip gives up width when its side of the bar is short of it and the label elides to what is left, which is the same question asked in the unit it is actually about — `max_chars` cut a short title on an empty bar exactly as readily as a long one on a full bar. It still sizes the marquee, which is a window measured in characters because it steps in characters.
pub fn label(player: &Player) -> String {
    if player.is_empty() {
        return String::new();
    }
    player.summary()
}

/// One frame of a scrolling title: the full text rotated left by `step` characters, cut to `max`.
///
/// A rotation rather than a bounce, so the chip's width never changes — a bar whose modules shift sideways as a title scrolls is worse than a truncated title. Text that already fits is returned untouched and never animates, which is what keeps the common case free.
pub fn marquee(player: &Player, config: &MediaConfig, step: usize) -> String {
    if player.is_empty() {
        return String::new();
    }
    let max = config.max_chars as usize;
    let text = player.summary();
    if max == 0 || text.chars().count() <= max {
        return truncate(&text, max);
    }
    let looped: Vec<char> = text.chars().chain(MARQUEE_GAP.chars()).collect();
    let offset = step % looped.len();
    looped
        .iter()
        .cycle()
        .skip(offset)
        .take(max)
        .collect::<String>()
}

/// Whether a title is long enough to be worth scrolling. The ticker is only started when it is, so a bar showing a short title costs nothing.
pub fn overflows(player: &Player, config: &MediaConfig) -> bool {
    !player.is_empty() && player.summary().chars().count() > config.max_chars.max(1) as usize
}

/// Cuts `text` to `max` characters, ending in `…` when it had to. Counts characters, not bytes, so a track title with accents or CJK is never cut mid-codepoint.
fn truncate(text: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    if text.chars().count() <= max {
        return text.to_string();
    }
    let kept: String = text.chars().take(max.saturating_sub(1)).collect();
    format!("{}…", kept.trim_end())
}

/// The marquee's clock: one tick per `[media] marquee_speed_ms`, forever.
///
/// A `watch` producer rather than a re-armed `timeout`, and that is the whole point: `watch` binds the subscription to the surface and drops it when the surface goes away, so the ticker ends with the bar it belongs to. A self-rescheduling timeout would keep firing into a torn-down surface.
pub fn marquee_ticks(tx: platform_wayland::EventSender<u64>) {
    let step = config::shared_config()
        .map(|c| c.media.marquee_step())
        .unwrap_or_else(|| std::time::Duration::from_millis(220));
    let mut frame: u64 = 0;
    loop {
        std::thread::sleep(step);
        frame = frame.wrapping_add(1);
        if !tx.send(frame) {
            return;
        }
    }
}

/// Click toggles playback. Nothing running is a no-op rather than an error: the chip is a readout until a player appears.
pub fn toggle() {
    mpris::play_pause();
}

/// The wheel over the chip, per `[media] scroll`: adjust the volume (the common case — the chip is where your pointer already is when a track is too loud), skip tracks, or nothing.
pub fn scroll(_dx: f32, dy: f32) {
    let config = config::config()
        .map(|c| c.media.clone())
        .unwrap_or_default();
    let up = dy > 0.0;
    match config.scroll {
        MediaScroll::Seek => {
            let step = config.seek_micros();
            mpris::seek(if up { step } else { -step });
        }
        MediaScroll::Volume => {
            services::volume::step(if up { 5 } else { -5 });
            crate::osd::show_volume();
        }
        MediaScroll::Track => {
            if up {
                mpris::previous();
            } else {
                mpris::next();
            }
        }
        MediaScroll::None => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn playing(title: &str, artist: &str) -> Player {
        Player {
            bus: "org.mpris.MediaPlayer2.spotify".to_string(),
            identity: "Spotify".to_string(),
            title: title.to_string(),
            artist: artist.to_string(),
            playback: Playback::Playing,
            ..Player::default()
        }
    }

    #[test]
    fn the_glyph_offers_the_action_not_the_state() {
        // A playing track offers "pause"; a paused one offers "play".
        assert_eq!(glyph(&playing("x", "y")), "pause");
        let paused = Player {
            playback: Playback::Paused,
            ..playing("x", "y")
        };
        assert_eq!(glyph(&paused), "play");
        assert_eq!(
            glyph(&Player::default()),
            "music",
            "nothing running shows a neutral glyph, not a control"
        );
    }

    #[test]
    fn nothing_running_yields_no_label_so_the_chip_collapses() {
        assert_eq!(label(&Player::default()), "");
        assert_eq!(marquee(&Player::default(), &MediaConfig::default(), 3), "");
    }

    #[test]
    fn a_title_that_fits_never_scrolls() {
        let config = MediaConfig {
            max_chars: 40,
            ..MediaConfig::default()
        };
        let short = playing("Blue", "Miles");
        assert!(!overflows(&short, &config));
        // Every step yields the same frame, so a ticker running over it would repaint nothing.
        assert_eq!(marquee(&short, &config, 0), marquee(&short, &config, 7));
    }

    #[test]
    fn a_long_title_scrolls_at_a_fixed_width_and_wraps_round() {
        let config = MediaConfig {
            max_chars: 10,
            ..MediaConfig::default()
        };
        let long = playing("A Love Supreme, Pt. I", "John Coltrane");
        assert!(overflows(&long, &config));

        let first = marquee(&long, &config, 0);
        assert_eq!(first.chars().count(), 10, "the chip's width never moves");
        assert!(
            first.starts_with("John Colt"),
            "it starts at the beginning: {first:?}"
        );
        assert_ne!(marquee(&long, &config, 1), first, "and it advances");

        // A full cycle returns to the start: the text plus the gap between its end and its own beginning.
        let cycle = long.summary().chars().count() + MARQUEE_GAP.chars().count();
        assert_eq!(marquee(&long, &config, cycle), first);
        assert_eq!(
            marquee(&long, &config, cycle * 3),
            first,
            "and stays in step"
        );
    }

    #[test]
    fn the_marquee_counts_characters_not_bytes() {
        let config = MediaConfig {
            max_chars: 6,
            ..MediaConfig::default()
        };
        let accented = playing("mañana señor mío", "Café");
        for step in 0..12 {
            assert_eq!(
                marquee(&accented, &config, step).chars().count(),
                6,
                "step {step} must not cut a codepoint short"
            );
        }
    }

    /// A track name reaches the chip whole; the elide is what shortens it, at the width it actually has.
    ///
    /// `max_chars` used to cut it here too, which took the room away before the layout could offer it — and it is still what sizes the marquee, so this is the line between the two: a window that steps in characters is measured in characters, and a label that has to fit a bar is measured in pixels.
    #[test]
    fn a_long_track_is_handed_over_untouched() {
        let track = playing("A Love Supreme, Pt. I", "John Coltrane");
        assert_eq!(label(&track), "John Coltrane — A Love Supreme, Pt. I");
        assert_eq!(
            marquee(
                &track,
                &MediaConfig {
                    max_chars: 12,
                    ..MediaConfig::default()
                },
                0
            )
            .chars()
            .count(),
            12,
            "and the marquee still steps through its own twelve-character window"
        );
    }

    #[test]
    fn truncation_counts_characters_not_bytes() {
        assert_eq!(truncate("mañana señor", 6), "mañan…");
        assert_eq!(truncate("東京の夜", 3), "東京…");
        assert_eq!(truncate("short", 40), "short", "what fits is untouched");
    }
}
