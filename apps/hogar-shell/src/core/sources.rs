//! The readings the shell's own modules expose to expressions, each fed by the service that owns it: `$battery.level`, `$clock.now`, `$workspace.active`. The two a module crate already declares beside the module that draws them — what is playing, what is waiting — live there.

use telar_expression::Value;
use ui::descriptor::{FieldDef, FieldType, Privacy, Reading, Sink, SourceDef, watch_feed};

use services::battery::Battery;
use services::gpu::Gpu;
use services::hyprland::Snapshot;
use services::netspeed::NetSpeed;
use services::powerprofiles::PowerProfile;
use services::resources::Resources;
use services::visualiser::Spectrum;
use services::volume::Volume;
use services::weather::Weather;

const fn public(name: &'static str, ty: FieldType) -> FieldDef {
    FieldDef {
        name,
        privacy: Privacy::Public,
        ty,
    }
}

const NUMBER: FieldType = FieldType::Number;
const TEXT: FieldType = FieldType::Text;
const BOOL: FieldType = FieldType::Bool;

fn number(value: impl Into<f64>) -> Value {
    Value::Number(value.into())
}

pub const BATTERY: SourceDef = SourceDef {
    id: "battery",
    fields: &[public("level", NUMBER), public("charging", BOOL)],
    feed: |sink| {
        watch_feed(services::battery::subscribe, sink, |b: &Battery| {
            Reading::from([number(b.level), Value::Bool(b.charging)])
        })
    },
};

/// The power profile power-profiles-daemon holds: `performance`, `balanced` or `power-saver`, and whether the daemon answered at all.
pub const POWER: SourceDef = SourceDef {
    id: "power",
    fields: &[public("profile", TEXT), public("available", BOOL)],
    feed: |sink| {
        watch_feed(
            services::powerprofiles::subscribe,
            sink,
            |p: &PowerProfile| {
                Reading::from([Value::text(p.active.as_str()), Value::Bool(p.available)])
            },
        )
    },
};

/// The time and the date as `[clock]` writes them, and the moment itself in seconds since the Unix epoch, for `df` to write any other way.
pub const CLOCK: SourceDef = SourceDef {
    id: "clock",
    fields: &[
        public("time", TEXT),
        public("date", TEXT),
        public("now", NUMBER),
    ],
    feed: clock,
};

fn clock(sink: Sink) {
    let settings = config::config()
        .map(|config| config.clock.clone())
        .unwrap_or_default();
    watch_feed(services::clock::subscribe, sink, move |now| {
        Reading::from([
            Value::text(services::clock::draw(now, settings.time_format())),
            Value::text(services::clock::draw(now, &settings.date_format)),
            number(now.timestamp() as f64),
        ])
    });
}

/// How busy the processor is, in percent, and its clock in MHz (zero where the kernel does not say).
pub const CPU: SourceDef = SourceDef {
    id: "cpu",
    fields: &[public("usage", NUMBER), public("frequency", NUMBER)],
    feed: |sink| {
        watch_feed(services::resources::subscribe, sink, |r: &Resources| {
            Reading::from([number(r.cpu), number(r.cpu_mhz.unwrap_or(0.0))])
        })
    },
};

/// How busy the graphics card is, in percent, and how much of its memory is in use, in bytes; zero for what the driver does not report.
pub const GPU: SourceDef = SourceDef {
    id: "gpu",
    fields: &[public("usage", NUMBER), public("vram", NUMBER)],
    feed: |sink| {
        watch_feed(services::gpu::subscribe, sink, |g: &Gpu| {
            Reading::from([
                number(g.usage.unwrap_or(0.0)),
                number(g.vram_used.unwrap_or(0) as f64),
            ])
        })
    },
};

/// Memory in use and in all, in bytes.
pub const MEMORY: SourceDef = SourceDef {
    id: "memory",
    fields: &[public("used", NUMBER), public("total", NUMBER)],
    feed: |sink| {
        watch_feed(services::resources::subscribe, sink, |r: &Resources| {
            Reading::from([number(r.memory.used as f64), number(r.memory.total as f64)])
        })
    },
};

/// Bytes a second, down and up, across every interface.
pub const NETSPEED: SourceDef = SourceDef {
    id: "netspeed",
    fields: &[public("down", NUMBER), public("up", NUMBER)],
    feed: |sink| {
        watch_feed(services::netspeed::subscribe, sink, |n: &NetSpeed| {
            Reading::from([number(n.down), number(n.up)])
        })
    },
};

/// The sensor `[temperature] sensor` names, in °C, and what that sensor is called — the hottest one where the name matches nothing.
pub const TEMPERATURE: SourceDef = SourceDef {
    id: "temperature",
    fields: &[public("celsius", NUMBER), public("sensor", TEXT)],
    feed: temperature,
};

fn temperature(sink: Sink) {
    let wanted = config::config()
        .map(|config| config.temperature.sensor.clone())
        .unwrap_or_default();
    watch_feed(
        services::resources::subscribe,
        sink,
        move |r: &Resources| {
            let sensor = r.sensor_of(&wanted);
            Reading::from([
                number(sensor.map_or(0.0, |sensor| sensor.celsius)),
                Value::text(sensor.map_or("", |sensor| sensor.label.as_str())),
            ])
        },
    );
}

/// The signed-in user's login name, and the path of their picture (empty where there is none).
pub const USER: SourceDef = SourceDef {
    id: "user",
    fields: &[public("name", TEXT), public("avatar", TEXT)],
    feed: user,
};

fn user(mut sink: Sink) {
    let dashboard = config::config()
        .map(|config| config.dashboard.clone())
        .unwrap_or_default();
    let avatar = modules::user::avatar_path(&dashboard)
        .map(|path| path.display().to_string())
        .unwrap_or_default();
    sink(Reading::from([
        Value::text(services::pam::current_user()),
        Value::text(avatar),
    ]));
}

/// Whether the audio the visualiser hears is silent.
pub const SPECTRUM: SourceDef = SourceDef {
    id: "spectrum",
    fields: &[public("silent", BOOL)],
    feed: |sink| {
        watch_feed(services::visualiser::subscribe, sink, |s: &Spectrum| {
            Reading::from([Value::Bool(s.silent)])
        })
    },
};

/// The output volume, in percent, and whether it is muted.
pub const VOLUME: SourceDef = SourceDef {
    id: "volume",
    fields: &[public("level", NUMBER), public("muted", BOOL)],
    feed: |sink| {
        watch_feed(services::volume::subscribe, sink, |v: &Volume| {
            Reading::from([number(v.level), Value::Bool(v.muted)])
        })
    },
};

/// Where the weather is for, the temperature there in °C, and the condition as a stable name — `clear`, `rain`, `snow` — to compare against rather than to show.
pub const WEATHER: SourceDef = SourceDef {
    id: "weather",
    fields: &[
        public("place", TEXT),
        public("temperature", NUMBER),
        public("condition", TEXT),
    ],
    feed: |sink| {
        watch_feed(services::weather::subscribe, sink, |w: &Weather| {
            Reading::from([
                Value::text(w.place.as_str()),
                number(w.temperature),
                Value::text(w.condition().id()),
            ])
        })
    },
};

/// The focused workspace: its id, its name, and how many workspaces there are.
pub const WORKSPACE: SourceDef = SourceDef {
    id: "workspace",
    fields: &[
        public("active", NUMBER),
        public("name", TEXT),
        public("count", NUMBER),
    ],
    feed: |sink| {
        watch_feed(services::hyprland::subscribe, sink, |s: &Snapshot| {
            let active = s.workspaces.iter().find(|w| w.id == s.active);
            Reading::from([
                number(s.active),
                Value::text(active.map_or("", |w| w.name.as_str())),
                number(s.workspaces.iter().filter(|w| !w.is_special()).count() as f64),
            ])
        })
    },
};
