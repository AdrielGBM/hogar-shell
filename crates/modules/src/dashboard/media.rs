//! The Media page: the track, the art, the playhead and the transport.
//!
//! The playhead is the one thing here the MPRIS service cannot broadcast: `Position` advances continuously and
//! emits no change signal, so following it means asking, and asking on everyone's behalf would wake the whole
//! shell several times a second for a number no bar chip shows. This page therefore owns the only ticker, at
//! the rate `[dashboard] media_update_interval` sets, and it dies with the surface.

use ui::scale::{paint, space};
use std::time::Duration;

use telar::{
    AlignItems, Color, Container, JustifyContent, LayoutError, LayoutItem, LayoutStyle,
    ReactiveList, RectStyle, RwSignal, SizeDimension, StyledContainer, Text, TextStyle, box_item,
    signal, track_layout,
};

use super::card::{self, Card, METER_HEIGHT};
use config::Config;
use config::theme::{FontRole, NordTheme};
use services::art::{self, ArtState};
use services::lyrics;
use services::mpris::{self, LoopStatus, Playback, Player};
use services::visualiser;
use ui::icon::icon_view;
use ui::widget;
use util::asset::Load;
use util::picture;
use util::reactive::{Live, derive, fixed, fixed_text};

const COVER: f32 = 96.0;

/// Clearance between the art's edge and the innermost bar of the visualiser ring, and how far the bars reach
/// past it. Together they are how much wider the heading's first column gets when the ring is switched on.
const RING_GAP: f32 = 6.0;
const RING_REACH: f32 = 22.0;

const TRANSPORT_ICON: f32 = 22.0;
const PRIMARY_ICON: f32 = 30.0;

/// Tall enough to show the line before and after the one being sung, which is what makes a lyric readable.
const LYRICS_HEIGHT: f32 = 200.0;

/// Kept off the edge when the card scrolls to it, so the current line never sits flush against the top.
const LYRIC_REVEAL_MARGIN: f32 = 28.0;

pub fn page(config: &Config, theme: NordTheme) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let player = signal(mpris::current().unwrap_or_default());
    let sink = player.clone();
    platform_wayland::watch(mpris::subscribe, move |p| sink.set(p));

    let position = signal(mpris::position().unwrap_or(0));
    let ticker = position.clone();
    let interval = config.dashboard.media_interval();
    platform_wayland::watch(
        move |tx| poll_position(tx, interval),
        move |micros| ticker.set(micros),
    );

    let mut cards = vec![now_playing(
        player.clone(),
        position.clone(),
        config,
        theme,
    )?];
    if config.lyrics.enabled {
        cards.push(lyrics_card(player, position, theme)?);
    }
    card::page(cards)
}

/// One line of the lyrics card, or the one line it shows when there are none.
///
/// A line carries the window it is sung in rather than the whole song: whether it is the current line is then a
/// comparison against two numbers, instead of every line re-scanning the list on every tick of the playhead.
#[derive(Clone, Debug, PartialEq, Eq)]
enum LyricLine {
    Sung {
        index: usize,
        from: i64,
        until: i64,
        text: String,
    },
    /// Nothing to show yet, or nothing to show at all — two different sentences, one row.
    Absent { searching: bool },
}

impl LyricLine {
    fn key(&self) -> (usize, String) {
        match self {
            LyricLine::Sung { index, text, .. } => (*index, text.clone()),
            LyricLine::Absent { searching } => (usize::MAX, searching.to_string()),
        }
    }
}

/// Turns timed lines into rows, giving each the moment the next one takes over.
fn lyric_lines(lines: &[lyrics::Line], searching: bool) -> Vec<LyricLine> {
    if lines.is_empty() {
        return vec![LyricLine::Absent { searching }];
    }
    lines
        .iter()
        .enumerate()
        .map(|(index, line)| LyricLine::Sung {
            index,
            from: line.at,
            // The last line holds until the track ends, which is what keeps it lit through an outro.
            until: lines.get(index + 1).map(|next| next.at).unwrap_or(i64::MAX),
            text: line.text.clone(),
        })
        .collect()
}

/// The lyrics, with the line being sung now lit and scrolled to.
///
/// The scroll area carries a definite height because it is a layout leaf — its content is laid out as its own root,
/// so nothing inside it contributes to its size and a `max_height` alone would measure zero.
fn lyrics_card(
    player: RwSignal<Player>,
    position: RwSignal<i64>,
    theme: NordTheme,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let scroll = telar::LayoutScrollArea::new_with(
        LayoutStyle::new()
            .flex_column()
            .width(SizeDimension::Percent(1.0))
            .height(LYRICS_HEIGHT),
        move |viewport| {
            let source = {
                let player = player.clone();
                move || {
                    // Read the track out of its cell first, then ask for its words: a signal read nested inside
                    // another's borrow panics, and `lyrics::of` takes a signal of its own.
                    let track = player.get();
                    let state = lyrics::of(&track).get();
                    let searching = state == Load::Loading;
                    lyric_lines(
                        state.ready().map(Vec::as_slice).unwrap_or_default(),
                        searching,
                    )
                }
            };
            let build = move |line: LyricLine| -> Result<Box<dyn LayoutItem>, LayoutError> {
                match line {
                    LyricLine::Absent { searching } => {
                        let message = if searching {
                            telar::t!("dashboard.lyrics_searching")
                        } else {
                            telar::t!("dashboard.lyrics_none")
                        };
                        Ok(box_item(Text::auto(
                            move || message.clone(),
                            LayoutStyle::new().width(SizeDimension::Percent(1.0)),
                            move || theme.text_style(FontRole::Caption, theme.muted),
                        )?))
                    }
                    LyricLine::Sung {
                        from, until, text, ..
                    } => {
                        let at = position.read_only();
                        let is_current = move || {
                            let now = at.get();
                            now >= from && now < until
                        };
                        let row = lyric_row(text, is_current.clone(), theme)?;
                        // Follow the song: the current line is brought into view, and only when it becomes the
                        // current one, so a user who scrolled ahead is not fighting the card. Tied to the row,
                        // since the list rebuilds these on every track change.
                        let node = row.layout_node();
                        let viewport = viewport.clone();
                        let follow = telar::effect(move || {
                            if is_current() {
                                viewport.reveal(node, LYRIC_REVEAL_MARGIN);
                            }
                        });
                        util::reactive::keeping(row, follow)
                    }
                }
            };
            Ok(Box::new(ReactiveList::with_style(
                LayoutStyle::new()
                    .flex_column()
                    .gap(space::XS)
                    .width(SizeDimension::Percent(1.0)),
                source,
                |line: &LyricLine| line.key(),
                build,
            )?) as Box<dyn LayoutItem>)
        },
    )?;

    Card::titled(telar::t!("dashboard.lyrics"))
        .icon("mic-vocal")
        .child(Box::new(scroll))
        .build(theme)
}

/// One line of words. An empty line is a gap between verses and still takes its height, so the lines do not
/// shuffle upwards while an instrumental break plays — but it is a *box* of that height rather than a `Text` of
/// blank characters: a space has no outline, and asking the renderer to fill an empty path is how tiny-skia's
/// "empty paths cannot be filled" warning gets emitted once per frame.
fn lyric_row(
    text: String,
    is_current: impl Fn() -> bool + Clone + 'static,
    theme: NordTheme,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    if text.trim().is_empty() {
        return Ok(box_item(Container::new(
            LayoutStyle::new()
                .width(SizeDimension::Percent(1.0))
                .height(theme.font(FontRole::Caption)),
            vec![],
        )?));
    }
    let shown = text;
    let styled = is_current.clone();
    Ok(box_item(Text::auto(
        move || shown.clone(),
        LayoutStyle::new().width(SizeDimension::Percent(1.0)),
        move || {
            if styled() {
                theme.text_style(FontRole::Body, theme.accent)
            } else {
                theme.text_style(FontRole::Caption, theme.subtle)
            }
        },
    )?))
}

/// Reads the playhead on one long-lived thread.
fn poll_position(tx: platform_wayland::EventSender<i64>, interval: Duration) {
    loop {
        if !tx.send(mpris::position().unwrap_or(0)) {
            return;
        }
        std::thread::sleep(interval);
    }
}

fn now_playing(
    player: RwSignal<Player>,
    position: RwSignal<i64>,
    config: &Config,
    theme: NordTheme,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let title = derive(player.clone(), |p| {
        let title = p.title.trim();
        if title.is_empty() {
            telar::t!("popout.nothing_playing")
        } else {
            title.to_string()
        }
    });
    let artist = derive(player.clone(), |p| non_empty(&p.artist));
    let album = derive(player.clone(), |p| non_empty(&p.album));
    let identity = derive(player.clone(), |p| non_empty(&p.identity));

    let heading = Container::new(
        LayoutStyle::new()
            .flex_row()
            .align_items(AlignItems::CENTER)
            .gap(space::XL)
            .width(SizeDimension::Percent(1.0)),
        vec![
            cover(player.clone(), config, theme)?,
            Box::new(Container::new(
                LayoutStyle::new().flex_column().flex_grow(1.0).gap(space::SM),
                vec![
                    text(title, theme.font(FontRole::Title), theme.text, true)?,
                    text(artist, theme.font(FontRole::Body), theme.subtle, false)?,
                    text(album, theme.font(FontRole::Caption), theme.muted, false)?,
                ],
            )?),
        ],
    )?;

    let card = Card::new(fixed_text(telar::t!("dashboard.now_playing")))
        .icon("disc-3")
        .trailing(identity)
        .child(Box::new(heading))
        .child(scrubber(player.clone(), position, theme)?)
        .child(transport(player, theme)?);
    card.build(theme)
}

/// The cover, ringed by the visualiser when `[media] visualiser` asks for it.
///
/// The ring is drawn *behind* the art in a box the art is centred in, so switching it on does not move the
/// title beside it by a different amount than it moves the picture. Nothing subscribes to the spectrum unless
/// the key is on, which is what keeps a media page from opening an audio capture nobody asked for.
fn cover(
    player: RwSignal<Player>,
    config: &Config,
    theme: NordTheme,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let art = cover_art(player, theme)?;
    if !config.media.visualiser {
        return Ok(art);
    }

    let bands = signal(visualiser::Spectrum::quiet(config.visualiser.band_count()).bars);
    let sink = bands.clone();
    platform_wayland::watch(
        visualiser::subscribe,
        move |spectrum: visualiser::Spectrum| sink.set(spectrum.bars),
    );

    // No fade and no floor: a silent spectrum is every bar at zero length, which draws nothing. The ring
    // hides itself by being made of the readings rather than by a rule about them.
    let ring = widget::spectrum_ring(
        derive(bands.read_only(), |bars| bars),
        fixed(theme.accent),
        COVER / 2.0 + RING_GAP,
        RING_REACH,
        widget::SpectrumStyle {
            gap: 1.5,
            radius: 1.5,
            floor: 0.0,
        },
        LayoutStyle::new().absolute_fill(),
    )?;

    let size = COVER + (RING_GAP + RING_REACH) * 2.0;
    Ok(Box::new(Container::new(
        LayoutStyle::new()
            .width(size)
            .height(size)
            .flex_shrink(0.0)
            .flex_row()
            .align_items(AlignItems::CENTER)
            .justify_content(JustifyContent::CENTER),
        vec![ring, art],
    )?))
}

/// The art itself, or a placeholder while there is nothing to show. A keyed list over the resolved file rather
/// than a plain image, so the art is decoded once per picture instead of once per repaint, and so the
/// placeholder is swapped for the real thing when the download lands.
fn cover_art(
    player: RwSignal<Player>,
    theme: NordTheme,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let source = derive(player, |p| p.art_url.clone());
    let rows = ReactiveList::new(
        move || vec![art_file(&source.get())],
        |path: &Option<String>| path.clone().unwrap_or_default(),
        move |path: Option<String>| match path.and_then(|p| picture::square(p.as_ref(), COVER, 8.0))
        {
            Some(image) => Ok(image),
            None => placeholder(theme),
        },
        0.0,
    )?;
    Ok(Box::new(rows))
}

/// The local file for an `artUrl`, starting a download the first time one is seen. Returns `None` while it is
/// still coming, which the card draws as the placeholder rather than as a gap that pops.
fn art_file(url: &str) -> Option<String> {
    match art::art(url).get() {
        ArtState::Ready(path) => Some(path.to_string_lossy().into_owned()),
        ArtState::Loading | ArtState::Missing => None,
    }
}

fn placeholder(theme: NordTheme) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let icon = icon_view(|| "music".to_string(), move || theme.muted, COVER * 0.45)?;
    Ok(Box::new(StyledContainer::new(
        LayoutStyle::new()
            .width(COVER)
            .height(COVER)
            .flex_shrink(0.0)
            .align_items(AlignItems::CENTER)
            .justify_content(JustifyContent::CENTER),
        paint::md(theme.overlay),
        vec![icon],
    )?))
}

/// A full-width playhead that seeks where it is pressed.
///
/// The jump is expressed as a *relative* `Seek`, because the absolute `SetPosition` takes the track id from the
/// metadata and refuses the call when it does not match — exactly the race a scrub hits when the track changes
/// under it. Which is also why the current position has to be subtracted here rather than sent as-is.
fn scrubber(
    player: RwSignal<Player>,
    position: RwSignal<i64>,
    theme: NordTheme,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let length = derive(player.clone(), |p| p.length);
    let seekable = derive(player, |p| p.can_seek);

    let for_fraction = length.clone();
    let fraction = derive(position.read_only(), move |micros| {
        let total = for_fraction.get();
        if total <= 0 {
            0.0
        } else {
            (micros as f32 / total as f32).clamp(0.0, 1.0)
        }
    });
    let tint = derive(seekable.clone(), move |can| {
        if can { theme.accent } else { theme.muted }
    });
    let bar = widget::meter(fraction, tint, theme.overlay, METER_HEIGHT)?;

    let track = StyledContainer::new(
        LayoutStyle::new()
            .width(SizeDimension::Percent(1.0))
            .padding_vertical(space::MD),
        move |_r| RectStyle::filled(Color::TRANSPARENT, 0.0),
        vec![bar],
    )?;
    let rect = track_layout(track.layout_node())
        .expect("a container registers its rect")
        .read_only();
    let (seek_length, seek_position) = (length.clone(), position.read_only());
    let track = track.on_drag(move |px, _py| {
        if !seekable.get() {
            return;
        }
        let width = rect.get().width;
        let total = seek_length.get();
        if width <= 0.0 || total <= 0 {
            return;
        }
        let target = ((px / width).clamp(0.0, 1.0) as f64 * total as f64) as i64;
        mpris::seek(target - seek_position.get());
    });

    let elapsed = derive(position.read_only(), clock_label);
    let remaining = derive(length, clock_label);

    Ok(Box::new(Container::new(
        LayoutStyle::new()
            .flex_column()
            .gap(space::XS)
            .width(SizeDimension::Percent(1.0)),
        vec![
            Box::new(track),
            widget::label_value(
                elapsed,
                remaining,
                theme.font(FontRole::Caption),
                theme.muted,
                theme.muted,
            )?,
        ],
    )?))
}

fn transport(
    player: RwSignal<Player>,
    theme: NordTheme,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let shuffle_tint = derive(player.clone(), move |p| {
        if p.shuffle { theme.accent } else { theme.muted }
    });
    let loop_tint = derive(player.clone(), move |p| {
        if p.loop_status == LoopStatus::Off {
            theme.muted
        } else {
            theme.accent
        }
    });
    let loop_glyph = derive(player.clone(), |p| {
        if p.loop_status == LoopStatus::Track {
            "repeat-1".to_string()
        } else {
            "repeat".to_string()
        }
    });
    let previous_tint = derive(player.clone(), move |p| enabled(p.can_go_previous, theme));
    let next_tint = derive(player.clone(), move |p| enabled(p.can_go_next, theme));
    let play_glyph = derive(player, |p| {
        if p.playback == Playback::Playing {
            "pause".to_string()
        } else {
            "play".to_string()
        }
    });

    let buttons: Vec<Box<dyn LayoutItem>> = vec![
        button(
            fixed_text("shuffle"),
            shuffle_tint,
            TRANSPORT_ICON,
            mpris::toggle_shuffle,
            theme,
        )?,
        button(
            fixed_text("skip-back"),
            previous_tint,
            TRANSPORT_ICON,
            mpris::previous,
            theme,
        )?,
        button(
            play_glyph,
            fixed(theme.text),
            PRIMARY_ICON,
            mpris::play_pause,
            theme,
        )?,
        button(
            fixed_text("skip-forward"),
            next_tint,
            TRANSPORT_ICON,
            mpris::next,
            theme,
        )?,
        button(
            loop_glyph,
            loop_tint,
            TRANSPORT_ICON,
            mpris::cycle_loop,
            theme,
        )?,
    ];

    Ok(Box::new(Container::new(
        LayoutStyle::new()
            .flex_row()
            .align_items(AlignItems::CENTER)
            .justify_content(JustifyContent::CENTER)
            .gap(space::LG)
            .width(SizeDimension::Percent(1.0)),
        buttons,
    )?))
}

/// A control the player says it cannot honour recedes to muted rather than disappearing: a transport row that
/// changes shape between tracks is harder to aim at than one whose buttons stay put.
fn enabled(can: bool, theme: NordTheme) -> Color {
    if can { theme.text } else { theme.muted }
}

fn button(
    glyph: Live<String>,
    tint: Live<Color>,
    size: f32,
    action: fn(),
    theme: NordTheme,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let icon = icon_view(move || glyph.get(), move || tint.get(), size)?;
    Ok(Box::new(
        StyledContainer::new(
            LayoutStyle::new()
                .padding_all(space::MD)
                .align_items(AlignItems::CENTER)
                .justify_content(JustifyContent::CENTER),
            paint::md(Color::TRANSPARENT),
            vec![icon],
        )?
        .hover_style(paint::md(theme.overlay))
        .on_press(action),
    ))
}

fn text(
    value: Live<String>,
    size: f32,
    color: Color,
    bold: bool,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    Ok(box_item(Text::auto(
        move || value.get(),
        LayoutStyle::new(),
        move || {
            let style = TextStyle::new(size, color);
            if bold { style.with_weight(700) } else { style }
        },
    )?))
}

fn non_empty(text: &str) -> String {
    let text = text.trim();
    if text.is_empty() {
        telar::t!("sysinfo.no_reading")
    } else {
        text.to_string()
    }
}

/// Microseconds as `m:ss`, or `h:mm:ss` past an hour. A track with no reported length (a live stream) reads as
/// `--:--` rather than as zero, which would claim it had just started.
fn clock_label(micros: i64) -> String {
    if micros <= 0 {
        return "--:--".to_string();
    }
    let total = micros / 1_000_000;
    let (hours, minutes, seconds) = (total / 3600, (total % 3600) / 60, total % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_playhead_reads_as_a_clock_and_an_unknown_length_says_so() {
        assert_eq!(clock_label(0), "--:--", "a live stream reports no length");
        assert_eq!(clock_label(-1), "--:--");
        assert_eq!(clock_label(9_000_000), "0:09");
        assert_eq!(clock_label(125_000_000), "2:05");
        assert_eq!(
            clock_label(3_725_000_000),
            "1:02:05",
            "past an hour the field grows rather than wrapping"
        );
    }

    fn sample_lines() -> Vec<lyrics::Line> {
        lyrics::parse("[00:10.00]First\n[00:14.00]\n[00:20.00]Last")
    }

    #[test]
    fn each_lyric_line_carries_the_window_it_is_sung_in() {
        let rows = lyric_lines(&sample_lines(), false);
        assert_eq!(rows.len(), 3);
        let window = |row: &LyricLine| match row {
            LyricLine::Sung { from, until, .. } => (*from, *until),
            LyricLine::Absent { .. } => (0, 0),
        };
        assert_eq!(window(&rows[0]), (10_000_000, 14_000_000));
        assert_eq!(
            window(&rows[1]),
            (14_000_000, 20_000_000),
            "a gap between verses is a line with no words, and it still takes its turn"
        );
        assert_eq!(
            window(&rows[2]),
            (20_000_000, i64::MAX),
            "the last line holds through the outro"
        );

        // Every row is its own identity, or the reactive list would reuse one line's widget for another's words.
        let keys: Vec<(usize, String)> = rows.iter().map(LyricLine::key).collect();
        assert_eq!(keys.len(), 3);
        assert!(
            keys.iter()
                .all(|key| keys.iter().filter(|k| *k == key).count() == 1)
        );
    }

    #[test]
    fn a_track_with_no_words_says_which_kind_of_nothing_it_is() {
        // Two different sentences: one is "wait", the other is "there is nothing to wait for".
        assert_eq!(
            lyric_lines(&[], true),
            vec![LyricLine::Absent { searching: true }]
        );
        assert_eq!(
            lyric_lines(&[], false),
            vec![LyricLine::Absent { searching: false }]
        );
        assert_ne!(
            LyricLine::Absent { searching: true }.key(),
            LyricLine::Absent { searching: false }.key(),
            "the row has to be rebuilt when the search gives up"
        );
    }

    /// The card's closures only run when something builds them, and each reads a signal — the shape that panics on
    /// a re-entrant borrow. Measured as well as built, because a scroll area is a layout leaf: it takes no size from
    /// its content, so a viewport with no height of its own clips every line away (see the launcher's list).
    #[test]
    fn the_lyrics_card_builds_with_a_viewport_that_has_a_size() {
        use telar::{AvailableSpace, compute_layout, new_container};

        telar::reset_layout_runtime();
        telar::set_theme(NordTheme::new());
        let player = signal(Player {
            title: "So What".to_string(),
            artist: "Miles Davis".to_string(),
            ..Player::default()
        });
        let position = signal(12_000_000i64);
        let card = lyrics_card(player, position, NordTheme::new()).expect("the card builds");
        let rect = track_layout(card.layout_node()).expect("the card registers its rect");
        let root = new_container(
            LayoutStyle::new().flex_column().width(420.0).height(600.0),
            &[card.layout_node()],
        )
        .expect("root");
        compute_layout(
            root,
            AvailableSpace::Definite(420.0),
            AvailableSpace::Definite(600.0),
        )
        .expect("layout");
        let rect = rect.get();
        assert!(
            rect.height >= LYRICS_HEIGHT,
            "the card measured {}px tall, which cannot hold a {LYRICS_HEIGHT}px lyric viewport",
            rect.height
        );
        assert!(rect.width > 0.0);
    }

    /// The ring is drawn *behind* the art, so the thing that can go wrong is silent: the art keeps its size and
    /// the bars are simply clipped away by a box that never grew. Measured, not built, for that reason.
    #[test]
    fn the_cover_grows_by_the_ring_only_when_the_ring_is_on() {
        use telar::{AvailableSpace, compute_layout, new_container};

        let measured = |visualiser: bool| {
            telar::reset_layout_runtime();
            telar::set_theme(NordTheme::new());
            let mut config = Config::starter();
            config.media.visualiser = visualiser;
            let cover = cover(signal(Player::default()), &config, NordTheme::new())
                .expect("the cover builds");
            let rect = track_layout(cover.layout_node()).expect("the cover registers its rect");
            let root = new_container(
                LayoutStyle::new().flex_row().width(420.0).height(400.0),
                &[cover.layout_node()],
            )
            .expect("root");
            compute_layout(
                root,
                AvailableSpace::Definite(420.0),
                AvailableSpace::Definite(400.0),
            )
            .expect("layout");
            rect.get()
        };

        assert_eq!(
            measured(false).width,
            COVER,
            "off, the art is the whole column"
        );
        assert_eq!(
            measured(true).width,
            COVER + (RING_GAP + RING_REACH) * 2.0,
            "on, the column has to hold the bars as well as the picture"
        );
    }
}
