//! What the shell drives on the machine: the lock, idle, radios, the keyboard and the weather.

use super::args::*;
use super::{Command, Target};

pub(crate) const LOCK: Target = Target {
    name: "lock",
    commands: &[
        Command {
            name: "on",
            args: "",
            help: "lock the session",
            run: |_| {
                use services::lock;
                // Refused up front rather than after the screen is covered: a lock this shell cannot
                // undo is the one failure the user has no way out of.
                lock::can_lock()?;
                lock::lock();
                Ok("locking".to_string())
            },
        },
        Command {
            name: "off",
            args: "",
            help: "unlock the session",
            run: |_| {
                services::lock::unlock();
                Ok("unlocking".to_string())
            },
        },
        Command {
            name: "toggle",
            args: "",
            help: "lock the session, or unlock it if it is locked",
            run: |_| {
                use services::lock;
                if lock::current().wanted {
                    lock::unlock();
                    Ok("unlocking".to_string())
                } else {
                    lock::can_lock()?;
                    lock::lock();
                    Ok("locking".to_string())
                }
            },
        },
        Command {
            name: "status",
            args: "",
            help: "whether the session is locked, and whether this machine can lock at all",
            run: |_| {
                use services::lock;
                let state = lock::current();
                let supported = match lock::can_lock() {
                    Ok(()) => "supported".to_string(),
                    Err(reason) => reason,
                };
                // Three columns because the middle one is the honest answer: between asking and being
                // granted, the desktop may still be on screen.
                Ok(format!(
                    "{}\t{}\t{supported}",
                    on_off(state.locked),
                    on_off(state.wanted)
                ))
            },
        },
    ],
};

pub(crate) const IDLE: Target = Target {
    name: "idle",
    commands: &[
        Command {
            name: "status",
            args: "",
            help: "whether the idle timers are armed, and what is holding them off",
            run: |_| {
                use services::idle;
                let config = config::config().map(|c| c.idle.clone()).unwrap_or_default();
                let held = idle::inhibited_by(&config).map(|r| r.id()).unwrap_or("-");
                Ok(format!("{}\t{held}", on_off(config.enabled)))
            },
        },
        Command {
            name: "inhibit",
            args: "<on|off|toggle>",
            help: "hold the idle timers off by hand",
            run: |args| {
                use services::idle;
                match arg(args, 0, "state")? {
                    "on" => idle::set_manual_inhibit(true),
                    "off" => idle::set_manual_inhibit(false),
                    "toggle" => idle::toggle_manual_inhibit(),
                    other => return Err(format!("expected on|off|toggle, got '{other}'")),
                }
                Ok(on_off(idle::manual_inhibit()).to_string())
            },
        },
    ],
};

pub(crate) const WEATHER: Target = Target {
    name: "weather",
    commands: &[
        Command {
            name: "now",
            args: "",
            help: "the current conditions: place, temperature and sky",
            run: |_| {
                use services::weather;
                let w = weather::current().ok_or("no weather reading yet")?;
                Ok(format!(
                    "{}\t{:.1}\t{}",
                    w.place,
                    w.temperature,
                    w.condition().id()
                ))
            },
        },
        Command {
            name: "forecast",
            args: "",
            help: "one line per day: date, high, low and chance of rain",
            run: |_| {
                use services::weather;
                let w = weather::current().ok_or("no weather reading yet")?;
                let rows: Vec<String> = w
                    .days
                    .iter()
                    .map(|d| {
                        format!(
                            "{}\t{:.0}\t{:.0}\t{}%",
                            d.date, d.high, d.low, d.precipitation
                        )
                    })
                    .collect();
                Ok(rows.join("\n"))
            },
        },
    ],
};

pub(crate) const GAMEMODE: Target = Target {
    name: "gamemode",
    commands: &[
        Command {
            name: "status",
            args: "",
            help: "whether GameMode is active, how many clients hold it, and whether the shell is one",
            run: |_| {
                use services::gamemode;
                let state = gamemode::current().filter(|s| s.available);
                let state = state.ok_or("gamemoded is not running")?;
                Ok(format!(
                    "{}\t{}\t{}",
                    on_off(state.active),
                    state.clients,
                    on_off(state.held)
                ))
            },
        },
        Command {
            name: "set",
            args: "<on|off|toggle>",
            help: "hold GameMode from the shell, or drop the shell's hold",
            run: |args| {
                use services::gamemode;
                match arg(args, 0, "state")? {
                    "on" => gamemode::set_held(true),
                    "off" => gamemode::set_held(false),
                    "toggle" => gamemode::toggle(),
                    other => return Err(format!("expected on|off|toggle, got '{other}'")),
                }
                Ok("ok".to_string())
            },
        },
    ],
};

pub(crate) const WIFI: Target = Target {
    name: "wifi",
    commands: &[
        Command {
            name: "status",
            args: "",
            help: "the radio state, the network joined and its signal",
            run: |_| {
                use services::network;
                // `enabled = false` and "no NetworkManager" both yield nothing here, and telling a user the
                // daemon is missing when they switched the section off themselves sends them hunting.
                let wifi = network::current_wifi()
                    .ok_or("[network] enabled is false, or NetworkManager is not running")?;
                let wifi = if wifi.available {
                    wifi
                } else {
                    return Err("NetworkManager is not running".to_string());
                };
                let (ssid, strength) = match wifi.active() {
                    Some(point) => (point.ssid.clone(), point.strength),
                    None => (String::new(), 0),
                };
                Ok(format!("{}\t{ssid}\t{strength}", on_off(wifi.enabled)))
            },
        },
        Command {
            name: "list",
            args: "",
            help: "networks in range: ssid, signal, security, saved",
            run: |_| {
                use services::network;
                let wifi = network::current_wifi().ok_or("NetworkManager is not running")?;
                let rows: Vec<String> = wifi
                    .networks()
                    .iter()
                    .map(|p| {
                        format!(
                            "{}\t{}\t{}\t{}",
                            p.ssid,
                            p.strength,
                            p.security.id(),
                            if p.saved { "saved" } else { "new" }
                        )
                    })
                    .collect();
                Ok(rows.join("\n"))
            },
        },
        Command {
            name: "radio",
            args: "<on|off|toggle>",
            help: "switch the wireless radio",
            run: |args| {
                use services::network;
                match arg(args, 0, "state")? {
                    "on" => network::set_wifi_enabled(true),
                    "off" => network::set_wifi_enabled(false),
                    "toggle" => network::toggle_wifi(),
                    other => return Err(format!("expected on|off|toggle, got '{other}'")),
                }
                Ok("ok".to_string())
            },
        },
        Command {
            name: "scan",
            args: "",
            help: "look for networks now",
            run: |_| {
                services::network::request_scan();
                Ok("scanning".to_string())
            },
        },
        Command {
            name: "connect",
            args: "<ssid> [password]",
            help: "join a network; the password is only needed the first time",
            run: |args| {
                use services::network;
                let ssid = arg(args, 0, "ssid")?;
                let wifi = network::current_wifi().ok_or("NetworkManager is not running")?;
                let point = wifi
                    .networks()
                    .into_iter()
                    .find(|p| p.ssid == ssid)
                    .ok_or_else(|| format!("'{ssid}' is not in range"))?;
                network::connect(&point.path, args.get(1).map(|p| p.to_string()));
                Ok(ssid.to_string())
            },
        },
        Command {
            name: "disconnect",
            args: "",
            help: "leave the current network, keeping it saved",
            run: |_| {
                services::network::disconnect();
                Ok("ok".to_string())
            },
        },
        Command {
            name: "forget",
            args: "<ssid>",
            help: "delete a saved network",
            run: |args| {
                let ssid = arg(args, 0, "ssid")?;
                services::network::forget(ssid);
                Ok(ssid.to_string())
            },
        },
    ],
};

pub(crate) const VPN: Target = Target {
    name: "vpn",
    commands: &[
        Command {
            name: "list",
            args: "",
            help: "every tunnel: id, state, kind and name",
            run: |_| {
                use services::vpn;
                let state = vpn::current().ok_or("no VPN service")?;
                let rows: Vec<String> = state
                    .tunnels
                    .iter()
                    .map(|t| {
                        format!(
                            "{}\t{}\t{}\t{}",
                            t.id,
                            if t.active { "up" } else { "down" },
                            t.kind,
                            t.name
                        )
                    })
                    .collect();
                Ok(rows.join("\n"))
            },
        },
        Command {
            name: "up",
            args: "<id>",
            help: "bring a tunnel up",
            run: |args| {
                let id = arg(args, 0, "id")?;
                services::vpn::set_active(id, true);
                Ok(id.to_string())
            },
        },
        Command {
            name: "down",
            args: "<id>",
            help: "bring a tunnel down",
            run: |args| {
                let id = arg(args, 0, "id")?;
                services::vpn::set_active(id, false);
                Ok(id.to_string())
            },
        },
        Command {
            name: "toggle",
            args: "",
            help: "drop the active tunnel, or raise the first configured one",
            run: |_| {
                services::vpn::toggle();
                Ok("ok".to_string())
            },
        },
    ],
};

pub(crate) const BLUETOOTH: Target = Target {
    name: "bluetooth",
    commands: &[
        Command {
            name: "status",
            args: "",
            help: "the adapter's power, scan state and how many devices are connected",
            run: |_| {
                use services::bluetooth;
                let bt = bluetooth::current().filter(|bt| bt.available);
                let bt = bt.ok_or("no bluetooth adapter")?;
                Ok(format!(
                    "{}\t{}\t{}",
                    on_off(bt.powered),
                    on_off(bt.discovering),
                    bt.connected_count()
                ))
            },
        },
        Command {
            name: "devices",
            args: "",
            help: "every known device: path, state and name",
            run: |_| {
                use services::bluetooth;
                let bt = bluetooth::current().ok_or("no bluetooth adapter")?;
                let rows: Vec<String> = bt
                    .devices
                    .iter()
                    .map(|d| {
                        let state = if d.connected {
                            "connected"
                        } else if d.paired {
                            "paired"
                        } else {
                            "available"
                        };
                        format!("{}\t{}\t{}", d.path, state, d.label())
                    })
                    .collect();
                Ok(rows.join("\n"))
            },
        },
        Command {
            name: "power",
            args: "<on|off|toggle>",
            help: "switch the adapter on or off",
            run: |args| {
                use services::bluetooth;
                match arg(args, 0, "state")? {
                    "on" => bluetooth::set_powered(true),
                    "off" => bluetooth::set_powered(false),
                    "toggle" => bluetooth::toggle_powered(),
                    other => return Err(format!("expected on|off|toggle, got '{other}'")),
                }
                Ok("ok".to_string())
            },
        },
        Command {
            name: "scan",
            args: "<on|off|toggle>",
            help: "start or stop looking for devices (a scan stops itself)",
            run: |args| {
                use services::bluetooth;
                match arg(args, 0, "state")? {
                    "on" => bluetooth::set_discovering(true),
                    "off" => bluetooth::set_discovering(false),
                    "toggle" => bluetooth::toggle_discovering(),
                    other => return Err(format!("expected on|off|toggle, got '{other}'")),
                }
                Ok("ok".to_string())
            },
        },
        Command {
            name: "connect",
            args: "<device-path>",
            help: "connect a device, pairing it first if it is new",
            run: |args| {
                let path = arg(args, 0, "device-path")?;
                services::bluetooth::connect(path);
                Ok(path.to_string())
            },
        },
        Command {
            name: "disconnect",
            args: "<device-path>",
            help: "disconnect a device",
            run: |args| {
                let path = arg(args, 0, "device-path")?;
                services::bluetooth::disconnect(path);
                Ok(path.to_string())
            },
        },
        Command {
            name: "forget",
            args: "<device-path>",
            help: "remove a pairing entirely",
            run: |args| {
                let path = arg(args, 0, "device-path")?;
                services::bluetooth::forget(path);
                Ok(path.to_string())
            },
        },
    ],
};

pub(crate) const KEYBOARD: Target = Target {
    name: "keyboard",
    commands: &[
        Command {
            name: "layout",
            args: "",
            help: "the main keyboard's active layout",
            run: |_| {
                use services::hyprland;
                let layout = hyprland::socket_dir()
                    .and_then(|dir| hyprland::keyboard_layout(&dir))
                    .ok_or("no keyboard reported")?;
                Ok(layout.name)
            },
        },
        Command {
            name: "next",
            args: "",
            help: "switch the main keyboard to its next layout",
            run: |_| {
                use services::hyprland;
                let dir = hyprland::socket_dir().ok_or("not running under Hyprland")?;
                let layout = hyprland::keyboard_layout(&dir).ok_or("no keyboard reported")?;
                hyprland::cycle_keyboard_layout(&dir, &layout.device);
                Ok("ok".to_string())
            },
        },
    ],
};
