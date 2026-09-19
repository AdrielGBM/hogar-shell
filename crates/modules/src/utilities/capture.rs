//! The capture half of the utilities panel: taking a picture, recording, and what has been recorded.
//!
//! The elapsed readout is the only thing here that ticks, and it ticks off the shared clock service rather than a timer of its own — the same second boundary the bar's clock uses, so nothing in the shell has two.

use std::path::{Path, PathBuf};
use ui::scale::space;

use telar::{
    AlignItems, Container, JustifyContent, LayoutError, LayoutItem, LayoutStyle, ReactiveList,
    RectStyle, RwSignal, SizeDimension, StyledContainer, Text, box_item, signal,
};

use config::theme::{FontRole, NordTheme};
use services::recorder::{self, Entry, Recording};
use services::screenshot::{self, Shot, Target};
use ui::glyph;
use ui::icon::icon_view;

const ROW_RADIUS: f32 = 10.0;
const ROW_ICON: f32 = 20.0;

/// The screenshot buttons, the recorder's own control, and a line about the last capture.
pub fn capture_card(theme: NordTheme) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let heading = Text::new(
        || telar::t!("capture.title"),
        LayoutStyle::new(),
        move || {
            theme
                .text_style(FontRole::Body, theme.text)
                .with_font_weight(700)
        },
    )?;

    let shots = Container::new(
        LayoutStyle::new()
            .flex_row()
            .gap(space::md())
            .width(SizeDimension::Percent(1.0)),
        vec![
            pill(
                || telar::t!("capture.screen"),
                glyph::screenshot(),
                true,
                || crate::capture::screenshot(Target::Screen),
                theme,
            )?,
            pill(
                || telar::t!("capture.output"),
                glyph::screenshot(),
                true,
                crate::capture::screenshot_output,
                theme,
            )?,
            pill(
                || telar::t!("capture.region"),
                glyph::area_select(),
                true,
                crate::capture::screenshot_region,
                theme,
            )?,
        ],
    )?;

    let children = vec![
        box_item(heading),
        Box::new(shots) as Box<dyn LayoutItem>,
        recorder_row(theme)?,
        last_capture(theme)?,
    ];
    card(children, theme)
}

/// The recorder's controls: one button that starts or stops, a pause beside it on a backend that can, and the elapsed time while it runs.
fn recorder_row(theme: NordTheme) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let live = signal(recorder::current());
    let sink = live;
    platform_wayland::watch(recorder::subscribe, move |state: Recording| sink.set(state));

    // The clock is what makes the readout move: a recording's elapsed time changes with the wall clock, not with anything the recorder publishes.
    let tick = signal(0u32);
    let ticker = tick;
    platform_wayland::watch(
        services::clock::subscribe,
        move |_: services::clock::Now| ticker.set(ticker.peek().wrapping_add(1)),
    );

    let config = config::config()
        .map(|c| c.recorder.clone())
        .unwrap_or_default();
    let backend = recorder::backend(&config);
    let can_pause = backend.is_some_and(|backend| backend.can_pause());

    let start_state = live.read_only();
    let start = pill_live(
        move || {
            if start_state.get().active {
                telar::t!("capture.stop")
            } else {
                telar::t!("capture.record")
            }
        },
        {
            let glyph_state = live.read_only();
            move || glyph::recording(glyph_state.get().active).to_string()
        },
        {
            let active_state = live.read_only();
            move || active_state.get().active
        },
        backend.is_some(),
        crate::capture::toggle_recording,
        theme,
    )?;

    let mut children: Vec<Box<dyn LayoutItem>> = vec![start];
    if can_pause {
        let pause_state = live.read_only();
        let paused_state = live.read_only();
        children.push(pill_live(
            move || {
                if pause_state.get().paused {
                    telar::t!("capture.resume")
                } else {
                    telar::t!("capture.pause")
                }
            },
            || "pause".to_string(),
            move || paused_state.get().paused,
            true,
            || {
                if let Err(reason) = recorder::toggle_pause() {
                    tracing::info!("recorder: {reason}");
                }
            },
            theme,
        )?);
    }

    let elapsed_state = live.read_only();
    let elapsed_tick = tick.read_only();
    let elapsed = Text::new(
        move || {
            // Both signals are read, and both matter: the recorder says whether anything is running, the tick is what brings the closure back a second later. Reading only the state would freeze the readout.
            elapsed_tick.get();
            let state = elapsed_state.get();
            if state.active {
                recorder::format_elapsed(state.elapsed())
            } else if backend.is_none() {
                telar::t!("capture.no_recorder")
            } else {
                String::new()
            }
        },
        LayoutStyle::new().flex_grow(1.0),
        move || theme.text_style(FontRole::Caption, theme.subtle),
    )?;
    children.push(box_item(elapsed));

    Ok(Box::new(Container::new(
        LayoutStyle::new()
            .flex_row()
            .align_items(AlignItems::CENTER)
            .gap(space::md())
            .width(SizeDimension::Percent(1.0)),
        children,
    )?))
}

/// What the last capture did — the file it wrote, or why it did not. Blank until the first one, so the card does not open with a line about nothing.
fn last_capture(theme: NordTheme) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let last = signal(screenshot::current());
    let sink = last;
    platform_wayland::watch(
        screenshot::subscribe,
        move |shot: Option<Result<Shot, String>>| sink.set(shot),
    );
    let text_state = last.read_only();
    let tint_state = last.read_only();
    let line = Text::new(
        move || match text_state.get() {
            Some(Ok(shot)) => shot_line(&shot),
            Some(Err(reason)) => reason,
            None => String::new(),
        },
        LayoutStyle::new(),
        move || {
            let failed = matches!(tint_state.get(), Some(Err(_)));
            let tint = if failed { theme.red } else { theme.subtle };
            theme
                .text_style(FontRole::Caption, tint)
                .with_clamp(1, true)
        },
    )?;
    Ok(box_item(line))
}

fn shot_line(shot: &Shot) -> String {
    match &shot.path {
        Some(path) => path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| path.display().to_string()),
        None if shot.copied => telar::t!("capture.copied"),
        None => telar::t!("capture.screenshot"),
    }
}

/// The recordings, newest first: press to open, right-click twice to delete.
///
/// Right-click-to-delete arms first, like the bluetooth panel's forget: a recording is minutes of something that cannot be taken again, and a stray click must not be able to remove it.
pub fn recordings_card(
    dir: PathBuf,
    limit: usize,
    theme: NordTheme,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let entries = signal(recorder::recordings(&dir, limit));
    // A finished recording is a new file in the list, and the recorder is the only thing that puts one there — so its state change is the refresh signal, rather than a watch on the directory.
    let refresh = entries;
    let refresh_dir = dir.clone();
    platform_wayland::watch(recorder::subscribe, move |_: Recording| {
        refresh.set(recorder::recordings(&refresh_dir, limit));
    });

    let heading = Text::new(
        || telar::t!("capture.recordings"),
        LayoutStyle::new(),
        move || {
            theme
                .text_style(FontRole::Body, theme.text)
                .with_font_weight(700)
        },
    )?;

    let armed = signal(String::new());
    let source = entries.read_only();
    let list_dir = dir.clone();
    let list_entries = entries;
    let rows = ReactiveList::new(
        move || source.get(),
        |entry: &Entry| row_key(entry),
        move |entry: Entry| row(entry, armed, list_entries, list_dir.clone(), limit, theme),
        6.0,
    )?;

    let empty_state = entries.read_only();
    let empty = Text::new(
        move || {
            if empty_state.get().is_empty() {
                telar::t!("capture.no_recordings")
            } else {
                String::new()
            }
        },
        LayoutStyle::new(),
        move || theme.text_style(FontRole::Caption, theme.muted),
    )?;

    card(
        vec![box_item(heading), Box::new(rows), box_item(empty)],
        theme,
    )
}

/// Keyed on what the row draws: a file being written grows, so its size — and therefore its subtitle — changes while the row is on screen.
fn row_key(entry: &Entry) -> String {
    format!(
        "{}|{}|{}",
        entry.path.display(),
        entry.bytes,
        entry.modified
    )
}

fn row(
    entry: Entry,
    armed: RwSignal<String>,
    entries: RwSignal<Vec<Entry>>,
    dir: PathBuf,
    limit: usize,
    theme: NordTheme,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let path = entry.path.clone();
    let key = path.display().to_string();
    let armed_text = armed.read_only();
    let armed_fill = armed.read_only();
    let armed_hover = armed.read_only();
    let is_armed = {
        let key = key.clone();
        move |signal: &telar::ReadSignal<String>| signal.get() == key
    };

    let icon = icon_view(|| "film".to_string(), move || theme.text, ROW_ICON)?;

    let name = Text::new(
        {
            let label = entry.name();
            move || label.clone()
        },
        LayoutStyle::new(),
        move || {
            theme
                .text_style(FontRole::Body, theme.text)
                .with_clamp(1, true)
        },
    )?;
    let subtitle = Text::new(
        {
            let size = entry.size_label();
            let is_armed = is_armed.clone();
            move || {
                if is_armed(&armed_text) {
                    telar::t!("capture.delete_confirm")
                } else {
                    size.clone()
                }
            }
        },
        LayoutStyle::new(),
        move || theme.text_style(FontRole::Caption, theme.subtle),
    )?;
    let labels = Container::new(
        LayoutStyle::new()
            .flex_column()
            .flex_grow(1.0)
            .gap(space::xs()),
        vec![box_item(name), box_item(subtitle)],
    )?;

    let reveal_dir = dir.clone();
    let reveal = pill(
        || telar::t!("capture.reveal"),
        "folder-open",
        true,
        move || reveal_in_files(&reveal_dir),
        theme,
    )?;

    let open_path = path.clone();
    let delete_path = path.clone();
    let disarm = armed;
    let row = StyledContainer::new(
        LayoutStyle::new()
            .flex_row()
            .align_items(AlignItems::CENTER)
            .justify_content(JustifyContent::SPACE_BETWEEN)
            .gap(space::lg())
            .padding_horizontal(space::lg())
            .padding_vertical(space::md())
            .width(SizeDimension::Percent(1.0)),
        {
            let is_armed = is_armed.clone();
            move |_| {
                let fill = if is_armed(&armed_fill) {
                    theme.red
                } else {
                    theme.base
                };
                RectStyle::filled(fill, ROW_RADIUS)
            }
        },
        vec![icon, Box::new(labels), reveal],
    )?
    .hover_style({
        let is_armed = is_armed.clone();
        move |_| {
            let fill = if is_armed(&armed_hover) {
                theme.red
            } else {
                theme.overlay
            };
            RectStyle::filled(fill, ROW_RADIUS)
        }
    })
    .on_press(move || {
        // A press on an armed row is a change of mind about deleting it, not an open.
        if disarm.peek() == open_path.display().to_string() {
            disarm.set(String::new());
            return;
        }
        open_file(&open_path);
    })
    .on_alt_press(move |_button| {
        let key = delete_path.display().to_string();
        if armed.peek() == key {
            if let Err(reason) = recorder::delete(&dir, &delete_path) {
                tracing::warn!("recordings: {reason}");
            }
            entries.set(recorder::recordings(&dir, limit));
            armed.set(String::new());
            return;
        }
        armed.set(key);
    });
    Ok(Box::new(row))
}

/// Opens a recording in whatever the desktop plays video with.
fn open_file(path: &Path) {
    services::apps::run_detached(format!("xdg-open {}", quoted(path)));
}

/// Shows the recordings directory in the configured file manager — `[general.apps] file_manager`, so this and every other "show me this folder" in the shell open the same application.
fn reveal_in_files(dir: &Path) {
    let manager = config::config()
        .map(|c| c.app_command(config::HelperApp::FileManager))
        .unwrap_or_else(|| "xdg-open".to_string());
    services::apps::run_detached(format!("{manager} {}", quoted(dir)));
}

/// A path as one shell word. The command goes through `sh -c`, so a recording in a folder with a space in its name would otherwise arrive as two arguments.
fn quoted(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', "'\\''"))
}

fn card(
    children: Vec<Box<dyn LayoutItem>>,
    theme: NordTheme,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    Ok(Box::new(StyledContainer::new(
        LayoutStyle::new()
            .flex_column()
            .gap(space::md())
            .padding_all(space::lg())
            .width(SizeDimension::Percent(1.0)),
        move |_| RectStyle::filled(theme.base, ui::panel::content_radius()),
        children,
    )?))
}

/// A labelled action button.
fn pill(
    label: impl Fn() -> String + 'static,
    icon: &'static str,
    enabled: bool,
    press: impl Fn() + 'static,
    theme: NordTheme,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    pill_live(
        label,
        move || icon.to_string(),
        || false,
        enabled,
        press,
        theme,
    )
}

/// A pill whose glyph, label and filled state all follow live values — the recorder's button, which is a different control depending on what the recorder is doing.
fn pill_live(
    label: impl Fn() -> String + 'static,
    icon: impl Fn() -> String + 'static,
    active: impl Fn() -> bool + 'static,
    enabled: bool,
    press: impl Fn() + 'static,
    theme: NordTheme,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let active = std::rc::Rc::new(active);
    let (fill_active, hover_active, text_active, icon_active) =
        (active.clone(), active.clone(), active.clone(), active);
    let glyph = icon_view(
        icon,
        move || {
            if !enabled {
                theme.muted
            } else if icon_active() {
                theme.accent.most_readable(&[theme.text, theme.base])
            } else {
                theme.text
            }
        },
        16.0,
    )?;
    let text = Text::new(label, LayoutStyle::new(), move || {
        let tint = if !enabled {
            theme.muted
        } else if text_active() {
            theme.accent.most_readable(&[theme.text, theme.base])
        } else {
            theme.text
        };
        theme.text_style(FontRole::Caption, tint)
    })?;
    let mut pill = StyledContainer::new(
        LayoutStyle::new()
            .flex_row()
            .align_items(AlignItems::CENTER)
            .justify_content(JustifyContent::CENTER)
            .gap(space::sm())
            .padding_horizontal(space::lg())
            .padding_vertical(space::md())
            .flex_shrink(0.0),
        move |_| {
            let fill = if fill_active() {
                theme.accent
            } else {
                theme.overlay
            };
            RectStyle::filled(fill, ROW_RADIUS)
        },
        vec![glyph, box_item(text)],
    )?
    .hover_style(move |_| {
        let fill = if !enabled {
            theme.overlay
        } else if hover_active() {
            theme.accent.darken(0.08)
        } else {
            theme.overlay.darken(0.1)
        };
        RectStyle::filled(fill, ROW_RADIUS)
    });
    if enabled {
        pill = pill.on_press(press);
    }
    Ok(Box::new(pill))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_cards_build() {
        telar::reset_layout_runtime();
        telar::set_theme(NordTheme::new());
        assert!(capture_card(NordTheme::new()).is_ok());

        telar::reset_layout_runtime();
        telar::set_theme(NordTheme::new());
        assert!(
            recordings_card(
                config::Config::default().recordings_dir(),
                config::RecorderConfig::default().entries(),
                NordTheme::new()
            )
            .is_ok()
        );
    }

    #[test]
    fn a_row_redraws_while_the_file_it_names_is_still_growing() {
        let mut entry = Entry {
            path: PathBuf::from("/tmp/a.mkv"),
            bytes: 1024,
            modified: 10,
        };
        let first = row_key(&entry);
        entry.bytes = 2048;
        assert_ne!(
            first,
            row_key(&entry),
            "the subtitle changed, so the row must"
        );
    }

    #[test]
    fn a_path_with_a_space_reaches_the_file_manager_as_one_word() {
        assert_eq!(quoted(Path::new("/home/a b/x.mkv")), "'/home/a b/x.mkv'");
        // A quote in a file name would otherwise end the argument early and hand `sh` the rest as code.
        assert_eq!(quoted(Path::new("/tmp/it's.mkv")), "'/tmp/it'\\''s.mkv'");
    }

    #[test]
    fn the_last_capture_line_says_what_happened() {
        let saved = Shot {
            path: Some(PathBuf::from("/pics/shot.png")),
            copied: true,
            size: (100, 100),
            taken_at: 0,
        };
        assert_eq!(shot_line(&saved), "shot.png");

        let clipboard_only = Shot {
            path: None,
            ..saved
        };
        assert!(
            !shot_line(&clipboard_only).is_empty(),
            "a capture that only reached the clipboard still has to say so"
        );
    }
}
