//! `audio`, `volume`, `mic` and `media`.

use services::pipewire::NodeKind;

use super::args::*;
use super::{Command, Target};

pub(crate) const AUDIO: Target = Target {
    name: "audio",
    commands: &[
        Command {
            name: "sinks",
            args: "",
            help: "output devices: id, level, mute, and which is the default",
            run: |_| Ok(list_nodes(NodeKind::Sink)),
        },
        Command {
            name: "sources",
            args: "",
            help: "input devices: id, level, mute, and which is the default",
            run: |_| Ok(list_nodes(NodeKind::Source)),
        },
        Command {
            name: "streams",
            args: "",
            help: "applications playing audio, with their own level",
            run: |_| Ok(list_nodes(NodeKind::OutputStream)),
        },
        Command {
            name: "default",
            args: "<id>",
            help: "make a device the default sink or source",
            run: |args| {
                let id = node_id(args)?;
                services::volume::set_default(id);
                Ok(id.to_string())
            },
        },
        Command {
            name: "set",
            args: "<id> <percent>",
            help: "set one device's or application's level",
            run: |args| {
                let id = node_id(args)?;
                let level = number(args, 1, "percent")?;
                services::volume::set_node(id, level);
                Ok(level.to_string())
            },
        },
        Command {
            name: "mute",
            args: "<id>",
            help: "toggle one device's or application's mute",
            run: |args| {
                let id = node_id(args)?;
                services::volume::toggle_node_mute(id);
                Ok(id.to_string())
            },
        },
    ],
};

pub(crate) const VOLUME: Target = Target {
    name: "volume",
    commands: &[
        Command {
            name: "get",
            args: "",
            help: "the default sink's level, and whether it is muted",
            run: |_| {
                let v = services::volume::reading().ok_or("no audio sink available")?;
                Ok(format!("{} {}", v.level, on_off(v.muted)))
            },
        },
        Command {
            name: "set",
            args: "<percent>",
            help: "set the default sink's level",
            run: |args| {
                let level = number(args, 0, "percent")?;
                services::volume::set(level);
                modules::osd::show_volume();
                Ok(level.to_string())
            },
        },
        Command {
            name: "step",
            args: "<±percent>",
            help: "move the level by a delta",
            run: |args| {
                let delta = number(args, 0, "delta")?;
                let v = services::volume::reading().ok_or("no audio sink available")?;
                let level = services::volume::set(v.level + delta);
                modules::osd::show_volume();
                Ok(level.to_string())
            },
        },
        Command {
            name: "up",
            args: "",
            help: "raise the level by [audio] increment",
            run: |_| {
                let step = services::volume::settings().step();
                let v = services::volume::reading().ok_or("no audio sink available")?;
                let level = services::volume::set(v.level + step);
                modules::osd::show_volume();
                Ok(level.to_string())
            },
        },
        Command {
            name: "down",
            args: "",
            help: "lower the level by [audio] increment",
            run: |_| {
                let step = services::volume::settings().step();
                let v = services::volume::reading().ok_or("no audio sink available")?;
                let level = services::volume::set(v.level - step);
                modules::osd::show_volume();
                Ok(level.to_string())
            },
        },
        Command {
            name: "mute",
            args: "",
            help: "toggle mute on the default sink",
            run: |_| {
                services::volume::toggle_mute();
                modules::osd::show_volume();
                Ok("toggled".to_string())
            },
        },
    ],
};

pub(crate) const MIC: Target = Target {
    name: "mic",
    commands: &[
        Command {
            name: "get",
            args: "",
            help: "the default source's level, and whether it is muted",
            run: |_| {
                let v = services::volume::mic_reading().ok_or("no audio source available")?;
                Ok(format!("{} {}", v.level, on_off(v.muted)))
            },
        },
        Command {
            name: "set",
            args: "<percent>",
            help: "set the default source's level",
            run: |args| {
                let level = number(args, 0, "percent")?;
                services::volume::set_mic(level);
                modules::osd::show_microphone();
                Ok(level.to_string())
            },
        },
        Command {
            name: "step",
            args: "<±percent>",
            help: "move the source level by a delta",
            run: |args| {
                let delta = number(args, 0, "delta")?;
                let v = services::volume::mic_reading().ok_or("no audio source available")?;
                let level = services::volume::set_mic(v.level + delta);
                modules::osd::show_microphone();
                Ok(level.to_string())
            },
        },
        Command {
            name: "up",
            args: "",
            help: "raise the source level by [audio] increment",
            run: |_| {
                let step = services::volume::settings().step();
                let v = services::volume::mic_reading().ok_or("no audio source available")?;
                let level = services::volume::set_mic(v.level + step);
                modules::osd::show_microphone();
                Ok(level.to_string())
            },
        },
        Command {
            name: "down",
            args: "",
            help: "lower the source level by [audio] increment",
            run: |_| {
                let step = services::volume::settings().step();
                let v = services::volume::mic_reading().ok_or("no audio source available")?;
                let level = services::volume::set_mic(v.level - step);
                modules::osd::show_microphone();
                Ok(level.to_string())
            },
        },
        Command {
            name: "mute",
            args: "",
            help: "toggle mute on the default source",
            run: |_| {
                modules::osd::mic_action();
                Ok("toggled".to_string())
            },
        },
    ],
};

pub(crate) const MEDIA: Target = Target {
    name: "media",
    commands: &[
        Command {
            name: "status",
            args: "",
            help: "the active player, its state and what it is playing",
            run: |_| {
                use services::mpris;
                let p = mpris::current().ok_or("no media player is running")?;
                Ok(format!("{:?}\t{}\t{}", p.playback, p.identity, p.summary()))
            },
        },
        Command {
            name: "get",
            args: "<title|artist|album|player|status|art>",
            help: "one field of the active player",
            run: |args| {
                use services::mpris;
                let field = arg(args, 0, "field")?;
                let p = mpris::current().ok_or("no media player is running")?;
                Ok(match field {
                    "title" => p.title,
                    "artist" => p.artist,
                    "album" => p.album,
                    "player" => p.identity,
                    "status" => format!("{:?}", p.playback),
                    "art" => p.art_url,
                    other => return Err(format!("unknown field '{other}'")),
                })
            },
        },
        Command {
            name: "play-pause",
            args: "",
            help: "toggle playback on the active player",
            run: |_| {
                services::mpris::play_pause();
                Ok("toggled".to_string())
            },
        },
        Command {
            name: "next",
            args: "",
            help: "skip to the next track",
            run: |_| {
                services::mpris::next();
                Ok("next".to_string())
            },
        },
        Command {
            name: "previous",
            args: "",
            help: "skip to the previous track",
            run: |_| {
                services::mpris::previous();
                Ok("previous".to_string())
            },
        },
        Command {
            name: "stop",
            args: "",
            help: "stop the active player",
            run: |_| {
                services::mpris::stop();
                Ok("stopped".to_string())
            },
        },
        Command {
            name: "seek",
            args: "<±seconds>",
            help: "move the playhead, if the player can seek",
            run: |args| {
                use services::mpris;
                let seconds = number(args, 0, "seconds")?;
                let player = mpris::current().ok_or("no media player is running")?;
                if !player.can_seek {
                    return Err(format!("{} cannot seek", player.identity));
                }
                mpris::seek(seconds as i64 * 1_000_000);
                Ok(seconds.to_string())
            },
        },
        Command {
            name: "shuffle",
            args: "<on|off|toggle>",
            help: "set the shuffle state",
            run: |args| {
                use services::mpris;
                match arg(args, 0, "state")? {
                    "on" => mpris::set_shuffle(true),
                    "off" => mpris::set_shuffle(false),
                    "toggle" => mpris::toggle_shuffle(),
                    other => return Err(format!("expected on|off|toggle, got '{other}'")),
                }
                Ok("ok".to_string())
            },
        },
        Command {
            name: "loop",
            args: "<off|track|playlist|cycle>",
            help: "set the repeat mode",
            run: |args| {
                use services::mpris::{self, LoopStatus};
                match arg(args, 0, "mode")? {
                    "off" | "none" => mpris::set_loop(LoopStatus::Off),
                    "track" => mpris::set_loop(LoopStatus::Track),
                    "playlist" => mpris::set_loop(LoopStatus::Playlist),
                    "cycle" => mpris::cycle_loop(),
                    other => {
                        return Err(format!("expected off|track|playlist|cycle, got '{other}'"));
                    }
                }
                Ok("ok".to_string())
            },
        },
    ],
};
