//! `brightness`, `wallpaper`, `screenshot` and `record`.

use super::args::*;
use super::{Command, Target};

pub(crate) const SCREENSHOT: Target = Target {
    name: "screenshot",
    commands: &[
        Command {
            name: "screen",
            args: "",
            help: "capture every monitor, composed into one image",
            run: |_| {
                use services::screenshot::Target;
                modules::capture::screenshot(Target::Screen);
                Ok("capturing".to_string())
            },
        },
        Command {
            name: "output",
            args: "[name]",
            help: "capture one monitor, the focused one by default",
            run: |args| {
                use services::screenshot::Target;
                match reading_output(args.first().copied())? {
                    Some(name) => modules::capture::screenshot(Target::Output(name)),
                    None => modules::capture::screenshot(Target::Screen),
                }
                Ok("capturing".to_string())
            },
        },
        Command {
            name: "region",
            args: "",
            help: "pick a region with the pointer, then capture it",
            run: |_| {
                modules::capture::screenshot_region();
                Ok("picking".to_string())
            },
        },
        Command {
            name: "cancel",
            args: "",
            help: "close the region picker without capturing",
            run: |_| {
                modules::capture::close_picker();
                Ok("cancelled".to_string())
            },
        },
        Command {
            name: "last",
            args: "",
            help: "where the last capture went, or why it failed",
            run: |_| {
                use services::screenshot;
                match screenshot::current() {
                    Some(Ok(shot)) => Ok(match shot.path {
                        Some(path) => path.display().to_string(),
                        None => "clipboard".to_string(),
                    }),
                    Some(Err(reason)) => Err(reason),
                    None => Ok(String::new()),
                }
            },
        },
    ],
};

pub(crate) const RECORD: Target = Target {
    name: "record",
    commands: &[
        Command {
            name: "start",
            args: "[screen|output|region]",
            help: "start recording; a region opens the picker first",
            run: |args| {
                match args.first().copied().unwrap_or("screen") {
                    "screen" => modules::capture::record_screen(),
                    "output" => modules::capture::record_output(),
                    "region" => modules::capture::record_region(),
                    other => {
                        return Err(format!("expected screen|output|region, got '{other}'"));
                    }
                }
                Ok("recording".to_string())
            },
        },
        Command {
            name: "stop",
            args: "",
            help: "stop the recording, letting the encoder close its file",
            run: |_| {
                services::recorder::stop();
                Ok("stopping".to_string())
            },
        },
        Command {
            name: "toggle",
            args: "",
            help: "stop the recording, or start one of the whole screen",
            run: |_| {
                modules::capture::toggle_recording();
                Ok("toggled".to_string())
            },
        },
        Command {
            name: "pause",
            args: "",
            help: "suspend or resume the recording, on a backend that can",
            run: |_| {
                let paused = services::recorder::toggle_pause()?;
                Ok(on_off(paused).to_string())
            },
        },
        Command {
            name: "status",
            args: "",
            help: "whether something is being recorded, for how long, and where",
            run: |_| {
                use services::recorder;
                let state = recorder::current();
                let backend = state
                    .backend
                    .map(|backend| backend.program().to_string())
                    .unwrap_or_else(|| "-".to_string());
                let file = state
                    .path
                    .as_ref()
                    .map(|path| path.display().to_string())
                    .unwrap_or_default();
                // Four columns, most useful first, so a bar script can cut one out.
                Ok(format!(
                    "{}\t{}\t{backend}\t{file}",
                    on_off(state.active),
                    recorder::format_elapsed(state.elapsed())
                ))
            },
        },
        Command {
            name: "list",
            args: "",
            help: "the recordings, newest first",
            run: |_| {
                use services::recorder;
                let config = config::config().ok_or("the shell is not running")?;
                let rows: Vec<String> =
                    recorder::recordings(&config.recordings_dir(), config.recorder.entries())
                        .into_iter()
                        .map(|entry| format!("{}\t{}", entry.size_label(), entry.path.display()))
                        .collect();
                Ok(rows.join("\n"))
            },
        },
    ],
};

pub(crate) const NIGHTLIGHT: Target = Target {
    name: "nightlight",
    commands: &[
        Command {
            name: "on",
            args: "[kelvin]",
            help: "warm every screen, 4000K by default",
            run: |args| {
                use services::nightlight;
                let kelvin = match args.first().copied() {
                    Some(value) => temperature(value)?,
                    None => nightlight::DEFAULT_TEMPERATURE,
                };
                if !nightlight::on(kelvin) {
                    return Err(refused());
                }
                Ok(format!("{kelvin}K"))
            },
        },
        Command {
            name: "off",
            args: "",
            help: "restore every screen's own colour",
            run: |_| {
                if !services::nightlight::off() {
                    return Err(refused());
                }
                Ok("off".to_string())
            },
        },
        Command {
            name: "toggle",
            args: "[kelvin]",
            help: "turn the night light off if it is on, and on if it is not",
            run: |args| {
                use services::nightlight;
                let kelvin = match args.first().copied() {
                    Some(value) => temperature(value)?,
                    None => nightlight::DEFAULT_TEMPERATURE,
                };
                if !nightlight::toggle(kelvin) {
                    return Err(refused());
                }
                Ok(match nightlight::current() {
                    Some(held) => format!("{held}K"),
                    None => "off".to_string(),
                })
            },
        },
        Command {
            name: "status",
            args: "",
            help: "the temperature currently held, or `off`",
            run: |_| {
                use services::nightlight;
                Ok(match nightlight::current() {
                    Some(kelvin) => format!("{kelvin}K"),
                    None => "off".to_string(),
                })
            },
        },
    ],
};

/// A temperature the protocol can actually act on, refused by name rather than clamped: a caller that typed
/// 400 meant something, and silently warming to 1000 would hide the typo behind a screen that went orange.
fn temperature(value: &str) -> Result<u32, String> {
    let kelvin: u32 = value
        .trim_end_matches(['k', 'K'])
        .parse()
        .map_err(|_| format!("'{value}' is not a temperature in kelvin"))?;
    let range = platform_wayland::MIN_TEMPERATURE..=platform_wayland::MAX_TEMPERATURE;
    if !range.contains(&kelvin) {
        return Err(format!(
            "{kelvin}K is outside {}–{}K",
            platform_wayland::MIN_TEMPERATURE,
            platform_wayland::MAX_TEMPERATURE
        ));
    }
    Ok(kelvin)
}

/// Why a night light did nothing, told apart: a compositor without the protocol is a different problem from a
/// compositor that has it and handed the gamma to something else.
fn refused() -> String {
    match services::nightlight::supported() {
        Some(true) => {
            "the compositor refused gamma control; something else may already hold it".to_string()
        }
        Some(false) => "this compositor does not implement wlr-gamma-control".to_string(),
        None => "no compositor could be reached".to_string(),
    }
}

pub(crate) const BRIGHTNESS: Target = Target {
    name: "brightness",
    commands: &[
        Command {
            name: "get",
            args: "[output]",
            help: "the brightness of a screen (no output means the primary one)",
            run: |args| {
                use services::brightness;
                let level = match args.first().copied() {
                    Some(output) => {
                        let output = dimmable_output(output)?;
                        brightness::current_output(&output)
                            .ok_or_else(|| format!("'{output}' reports no brightness"))?
                    }
                    None => brightness::current().ok_or("no controllable display")?,
                };
                Ok(level.to_string())
            },
        },
        Command {
            name: "refresh",
            args: "",
            help: "detect displays again, for a monitor plugged in since startup",
            run: |_| {
                services::brightness::refresh();
                Ok("detecting".to_string())
            },
        },
        Command {
            name: "list",
            args: "",
            help: "every controllable display: output, level, kind and label",
            run: |_| {
                use services::brightness::Kind;
                let rows: Vec<String> = services::brightness::snapshot()
                    .displays
                    .iter()
                    .map(|display| {
                        let kind = match display.kind {
                            Kind::Internal { .. } => "internal",
                            Kind::External { .. } => "external",
                        };
                        format!(
                            "{}\t{}\t{kind}\t{}",
                            display.output, display.level, display.label
                        )
                    })
                    .collect();
                Ok(rows.join("\n"))
            },
        },
        Command {
            name: "set",
            args: "<percent> [output|all]",
            help: "set the brightness of a screen (no output means the primary one)",
            run: |args| {
                let level = number(args, 0, "percent")?;
                for output in dimmable_targets(args.get(1).copied())? {
                    services::brightness::set_output(&output, level);
                }
                modules::osd::show_brightness();
                // The applied value, not the requested one: `set 150` puts the panel at 100, and a script that
                // reads the reply back is owed the level the screen is actually at.
                Ok(level.clamp(0, 100).to_string())
            },
        },
        Command {
            name: "step",
            args: "<±percent> [output|all]",
            help: "move a screen's brightness by a delta",
            run: |args| {
                let delta = number(args, 0, "delta")?;
                for output in dimmable_targets(args.get(1).copied())? {
                    services::brightness::step_output(&output, delta);
                }
                modules::osd::show_brightness();
                Ok(delta.to_string())
            },
        },
        Command {
            name: "up",
            args: "[output|all]",
            help: "raise the brightness by [brightness] increment",
            run: |args| {
                let step = services::brightness::settings().step();
                for output in dimmable_targets(args.first().copied())? {
                    services::brightness::step_output(&output, step);
                }
                modules::osd::show_brightness();
                Ok(step.to_string())
            },
        },
        Command {
            name: "down",
            args: "[output|all]",
            help: "lower the brightness by [brightness] increment",
            run: |args| {
                let step = services::brightness::settings().step();
                for output in dimmable_targets(args.first().copied())? {
                    services::brightness::step_output(&output, -step);
                }
                modules::osd::show_brightness();
                Ok((-step).to_string())
            },
        },
    ],
};

pub(crate) const WALLPAPER: Target = Target {
    name: "wallpaper",
    commands: &[
        Command {
            name: "get",
            args: "[output]",
            help: "the image a screen is showing (no output means the focused one)",
            run: |args| {
                use services::wallpaper;
                let config = config::config().ok_or("the shell is not running")?;
                let output = reading_output(args.first().copied())?;
                wallpaper::current_image(&config, output.as_deref())
                    .map(|path| path.display().to_string())
                    .ok_or_else(|| "no wallpaper is set".to_string())
            },
        },
        Command {
            name: "list",
            args: "",
            help: "every image in the library: folder, name and path",
            run: |_| {
                let rows: Vec<String> = services::wallpaper::all()
                    .iter()
                    .map(|entry| {
                        format!(
                            "{}\t{}\t{}",
                            if entry.folder.is_empty() {
                                "-"
                            } else {
                                &entry.folder
                            },
                            entry.name,
                            entry.path.display()
                        )
                    })
                    .collect();
                Ok(rows.join("\n"))
            },
        },
        Command {
            name: "reload",
            args: "",
            help: "re-scan the wallpaper folder",
            run: |_| Ok(services::wallpaper::reload().to_string()),
        },
        Command {
            name: "set",
            args: "<path> [output]",
            help: "put an image on every screen, or on one of them",
            run: |args| {
                use services::wallpaper;
                let path = util::paths::expand_tilde(std::path::Path::new(arg(args, 0, "path")?));
                // Checked here rather than left to the surface: a `set` that answered `ok` and changed
                // nothing because the file is gone is the one reply a script cannot act on.
                if !path.is_file() {
                    return Err(format!("'{}' is not a file", path.display()));
                }
                wallpaper::set(&path, target_output(args.get(1).copied())?.as_deref());
                refresh_scheme();
                Ok(path.display().to_string())
            },
        },
        Command {
            name: "random",
            args: "[output]",
            help: "pick one from the library at random",
            run: |args| {
                use services::wallpaper;
                let config = config::config().ok_or("the shell is not running")?;
                let output = target_output(args.first().copied())?;
                let showing = wallpaper::current_image(&config, output.as_deref());
                // Named, because the folder is the thing that is wrong nine times out of ten — it defaults
                // to `$XDG_PICTURES_DIR/Wallpapers` and a user whose collection is one directory over has
                // no way to tell an empty folder from the wrong one.
                let picked = wallpaper::random(showing.as_deref()).ok_or_else(|| {
                    format!(
                        "no images in {} (set [paths] wallpapers)",
                        config.wallpaper_dir().display()
                    )
                })?;
                wallpaper::set(&picked, output.as_deref());
                refresh_scheme();
                Ok(picked.display().to_string())
            },
        },
        Command {
            name: "clear",
            args: "[output]",
            help: "drop the runtime choice, putting [background] back in charge",
            run: |args| {
                let output = target_output(args.first().copied())?;
                services::wallpaper::clear(output.as_deref());
                refresh_scheme();
                Ok("cleared".to_string())
            },
        },
    ],
};
