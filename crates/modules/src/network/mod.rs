//! The network panel: the radio, the networks in range, and joining one.
//!
//! The bar chip stays what it was — a sysfs link verdict that needs no NetworkManager — and this panel is the NetworkManager view layered on top. So a machine without NM keeps a working chip and gets a panel that says why it is empty, rather than the chip going blank because the panel's dependency is missing.

telar::rsx_modules!(::config::theme::NordTheme);

use telar::{
    AlignItems, Container, Input, JustifyContent, LayoutError, LayoutItem, LayoutStyle,
    ReactiveList, RectStyle, RwSignal, SizeDimension, StyledContainer, Text, box_item, signal,
    use_theme,
};
use ui::scale::space;

use config::NetworkConfig;
use config::surface_env;
use config::theme::{FontRole, NordTheme};
use services::network::{self as net, AccessPoint, Security, Wifi};
use ui::glyph;
use ui::icon::icon_view;

const ROW_ICON: f32 = 20.0;
const ROW_RADIUS: f32 = 8.0;

/// One row of the list: a network, and whether it is the one currently asking for a password.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Row {
    point: AccessPoint,
    asking: bool,
}

impl Row {
    /// Keyed on what the row draws — but deliberately *not* on the signal strength, which moves on every scan. Folding it in would rebuild the row several times a second, and a rebuilt row destroys the password field mid-typing along with its focus. The strength is drawn from a signal instead.
    fn key(&self) -> String {
        format!(
            "{}|{}|{}|{}",
            self.point.ssid, self.point.active, self.point.saved, self.asking
        )
    }
}

pub fn network_panel() -> Result<Box<dyn LayoutItem>, LayoutError> {
    let config = surface_env()
        .map(|env| env.config.network)
        .unwrap_or_default();
    if let Some(env) = surface_env() {
        services::locale::attach(env.config.language());
    }
    network_view(config)
}

/// The panel's whole content, taking its config rather than reading the surface's, so a caller that already resolved one — a drawer, a float — does not have to be a surface for this to build.
pub fn network_view(config: NetworkConfig) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let theme = use_theme::<NordTheme>();

    let state = signal(net::current_wifi().unwrap_or_default());
    let sink = state;
    platform_wayland::watch(net::subscribe_wifi, move |wifi| sink.set(wifi));

    // Opening the panel is the gesture that means "show me what is around", so it is also what looks.
    if state.peek().enabled {
        net::request_scan();
    }

    let asking = signal(String::new());
    let password = signal(String::new());
    let armed = signal(String::new());

    let children = vec![
        header(state, theme)?,
        list(state, asking, password, armed, config, theme)?,
    ];
    Ok(Box::new(Container::new(
        LayoutStyle::new()
            .flex_column()
            .gap(space::lg())
            .width(SizeDimension::Percent(1.0)),
        children,
    )?))
}

fn header(state: RwSignal<Wifi>, theme: NordTheme) -> Result<Box<dyn LayoutItem>, LayoutError> {
    // One handle per closure: a signal is not `Copy`, and each reader below outlives the others.
    let subtitle_state = state.read_only();
    let radio_label = state.read_only();
    let radio_active = state.read_only();
    let scan_label = state.read_only();
    let scan_active = state.read_only();

    // Read out, then translate: `status_line` calls `t!`, and a `with` here would still hold the reactive runtime's borrow when it read the locale signal.
    let subtitle = Text::new(
        move || status_line(&subtitle_state.get()),
        LayoutStyle::new(),
        move || theme.text_style(FontRole::Caption, theme.subtle),
    )?;
    let title = Text::new(
        || telar::t!("network.title"),
        LayoutStyle::new(),
        move || {
            theme
                .text_style(FontRole::Title, theme.text)
                .with_font_weight(700)
        },
    )?;
    let labels = Container::new(
        LayoutStyle::new()
            .flex_column()
            .flex_grow(1.0)
            .gap(space::xs()),
        vec![box_item(title), box_item(subtitle)],
    )?;

    let radio = pill(
        move || {
            if radio_label.get().enabled {
                telar::t!("network.on")
            } else {
                telar::t!("network.off")
            }
        },
        move || radio_active.get().enabled,
        net::toggle_wifi,
        theme,
    )?;
    let scan = pill(
        move || {
            if scan_label.get().scanning {
                telar::t!("network.scanning_short")
            } else {
                telar::t!("network.scan")
            }
        },
        move || scan_active.get().scanning,
        net::request_scan,
        theme,
    )?;

    Ok(Box::new(Container::new(
        LayoutStyle::new()
            .flex_row()
            .align_items(AlignItems::CENTER)
            .gap(space::md())
            .width(SizeDimension::Percent(1.0)),
        vec![Box::new(labels), radio, scan],
    )?))
}

/// What the radio is doing, under the title.
fn status_line(wifi: &Wifi) -> String {
    if !wifi.available {
        return telar::t!("network.unavailable");
    }
    if !wifi.enabled {
        return telar::t!("network.off");
    }
    if let Some(active) = wifi.active() {
        return active.ssid.clone();
    }
    if wifi.scanning {
        return telar::t!("network.scanning");
    }
    telar::t!("network.not_connected")
}

/// The networks worth listing: one row per name, hidden ones dropped unless asked for, capped.
fn listed(wifi: &Wifi, config: NetworkConfig) -> Vec<AccessPoint> {
    wifi.networks()
        .into_iter()
        .filter(|p| config.show_hidden || !p.ssid.trim().is_empty())
        .take(config.network_limit())
        .collect()
}

fn list(
    state: RwSignal<Wifi>,
    asking: RwSignal<String>,
    password: RwSignal<String>,
    armed: RwSignal<String>,
    config: NetworkConfig,
    theme: NordTheme,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let source = state.read_only();
    let source_asking = asking.read_only();
    let empty_state = state.read_only();

    let rows = ReactiveList::new(
        move || {
            let asking = source_asking.get();
            listed(&source.get(), config)
                .into_iter()
                .map(|point| Row {
                    asking: asking == point.ssid,
                    point,
                })
                .collect()
        },
        |row: &Row| row.key(),
        move |row: Row| network_row(row, state, asking, password, armed, theme),
        6.0,
    )?;

    let empty = Text::new(
        move || empty_line(&empty_state.get(), config),
        LayoutStyle::new(),
        move || theme.text_style(FontRole::Caption, theme.muted),
    )?;

    Ok(Box::new(Container::new(
        LayoutStyle::new()
            .flex_column()
            .gap(space::md())
            .width(SizeDimension::Percent(1.0)),
        vec![Box::new(rows), box_item(empty)],
    )?))
}

/// The line under an empty list — never blank, so the panel always says why there is nothing to pick.
fn empty_line(wifi: &Wifi, config: NetworkConfig) -> String {
    if !wifi.available {
        telar::t!("network.unavailable")
    } else if !wifi.enabled {
        telar::t!("network.turn_on")
    } else if listed(wifi, config).is_empty() {
        telar::t!("network.no_networks")
    } else {
        String::new()
    }
}

/// One network. Press joins or leaves it; a secured network with no saved connection opens a password field first. Right-click forgets a saved one, arming before it fires — forgetting is not undoable from here.
fn network_row(
    row: Row,
    state: RwSignal<Wifi>,
    asking: RwSignal<String>,
    password: RwSignal<String>,
    armed: RwSignal<String>,
    theme: NordTheme,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let point = row.point.clone();
    let ssid = point.ssid.clone();
    let active = point.active;

    let armed_text = armed.read_only();
    let armed_tint = armed.read_only();
    let armed_fill = armed.read_only();
    let armed_hover = armed.read_only();
    let is_armed = {
        let ssid = ssid.clone();
        move |signal: &telar::ReadSignal<String>| signal.get() == ssid
    };

    // The strength is read from the live state rather than baked into the row, so a scan repaints the arc without rebuilding the row (see `Row::key`).
    let strength = {
        let state = state.read_only();
        let ssid = ssid.clone();
        move || {
            state
                .get()
                .networks()
                .into_iter()
                .find(|p| p.ssid == ssid)
                .map(|p| p.strength)
                .unwrap_or(0)
        }
    };
    let strength_icon = strength.clone();
    let strength_text = strength.clone();

    let icon = icon_view(
        move || glyph::wifi_signal(strength_icon()).to_string(),
        move || if active { theme.accent } else { theme.text },
        ROW_ICON,
    )?;

    let name = Text::new(
        {
            let ssid = ssid.clone();
            move || ssid.clone()
        },
        LayoutStyle::new(),
        move || theme.text_style(FontRole::Body, theme.text),
    )?;
    let status = Text::new(
        {
            let point = point.clone();
            let is_armed = is_armed.clone();
            move || {
                if is_armed(&armed_text) {
                    telar::t!("network.forget_confirm")
                } else {
                    detail_line(&point)
                }
            }
        },
        LayoutStyle::new(),
        {
            let is_armed = is_armed.clone();
            move || {
                let tint = if is_armed(&armed_tint) {
                    theme.red
                } else {
                    theme.subtle
                };
                theme.text_style(FontRole::Caption, tint)
            }
        },
    )?;
    let labels = Container::new(
        LayoutStyle::new()
            .flex_column()
            .flex_grow(1.0)
            .gap(space::xs()),
        vec![box_item(name), box_item(status)],
    )?;

    let trailing = Text::new(
        move || format!("{}%", strength_text()),
        LayoutStyle::new().flex_shrink(0.0),
        move || theme.text_style(FontRole::Caption, theme.muted),
    )?;

    let saved = point.saved;
    let press_ssid = ssid.clone();
    let press_asking = asking;
    let press_password = password;
    let disarm = armed;
    let head = StyledContainer::new(
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
        vec![icon, Box::new(labels), box_item(trailing)],
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
    .on_press({
        let point = point.clone();
        move || {
            // A press on an armed row is a change of mind about forgetting it, not a connection.
            if disarm.peek() == press_ssid {
                disarm.set(String::new());
                return;
            }
            if point.active {
                net::disconnect();
                return;
            }
            if needs_prompt(&point) {
                press_password.set(String::new());
                press_asking.set(press_ssid.clone());
                return;
            }
            join(&press_ssid, None);
        }
    })
    .on_alt_press({
        let ssid = ssid.clone();
        move |_button| {
            if !saved {
                return;
            }
            if armed.peek() == ssid {
                net::forget(&ssid);
                armed.set(String::new());
                return;
            }
            armed.set(ssid.clone());
        }
    });

    if !row.asking {
        return Ok(Box::new(head));
    }
    Ok(Box::new(Container::new(
        LayoutStyle::new()
            .flex_column()
            .gap(space::md())
            .width(SizeDimension::Percent(1.0)),
        vec![Box::new(head), prompt(ssid, asking, password, theme)?],
    )?))
}

/// Whether joining this network needs a password from the user: secured, not already saved, and something the shell can actually authenticate on its own.
fn needs_prompt(point: &AccessPoint) -> bool {
    point.security.needs_password() && !point.saved && point.security.joinable_with_a_password()
}

/// The password field, shown under the row it belongs to. Masked: a shell panel is on screen in front of whoever is in the room.
fn prompt(
    ssid: String,
    asking: RwSignal<String>,
    password: RwSignal<String>,
    theme: NordTheme,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let submit = {
        let ssid = ssid.clone();
        move || {
            join(&ssid, Some(password.peek()));
            password.set(String::new());
            asking.set(String::new());
        }
    };
    let on_enter = submit.clone();

    let field = Input::new(
        password,
        LayoutStyle::new()
            .flex_grow(1.0)
            .height(theme.font(FontRole::Body) * 1.6),
        move || theme.text_style(FontRole::Body, theme.text),
    )?
    .secret()
    .placeholder(telar::t!("network.password"))
    .on_submit(on_enter);

    let boxed = StyledContainer::new(
        LayoutStyle::new()
            .flex_grow(1.0)
            .padding_horizontal(space::md())
            .padding_vertical(space::sm()),
        move |_| RectStyle::filled(theme.base, ROW_RADIUS),
        vec![box_item(field)],
    )?;

    let join_button = pill(|| telar::t!("network.join"), || false, submit, theme)?;
    let cancel = pill(
        || telar::t!("network.cancel"),
        || false,
        move || asking.set(String::new()),
        theme,
    )?;

    Ok(Box::new(Container::new(
        LayoutStyle::new()
            .flex_row()
            .align_items(AlignItems::CENTER)
            .gap(space::md())
            .padding_horizontal(space::lg())
            .width(SizeDimension::Percent(1.0)),
        vec![Box::new(boxed), join_button, cancel],
    )?))
}

/// Joins by name rather than by object path: the strongest radio for an SSID changes as you move, and the row was built from a snapshot. Resolving at press time joins the one that is actually best right now.
fn join(ssid: &str, password: Option<String>) {
    let Some(point) =
        net::current_wifi().and_then(|w| w.networks().into_iter().find(|p| p.ssid == ssid))
    else {
        return;
    };
    net::connect(&point.path, password);
}

/// What a row says about itself: its state where it has one, else how it is secured and on which band.
fn detail_line(point: &AccessPoint) -> String {
    if point.active {
        return telar::t!("network.connected");
    }
    let security = match point.security {
        Security::Open => telar::t!("network.open"),
        Security::Enterprise => telar::t!("network.enterprise"),
        other => other.id().to_uppercase(),
    };
    let band = point.band();
    let mut detail = if band.is_empty() {
        security
    } else {
        format!("{security} · {band}")
    };
    if point.saved {
        detail = format!("{detail} · {}", telar::t!("network.saved"));
    }
    detail
}

fn pill(
    label: impl Fn() -> String + 'static,
    active: impl Fn() -> bool + 'static,
    on_press: impl Fn() + 'static,
    theme: NordTheme,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let active = std::rc::Rc::new(active);
    let (fill_active, hover_active, text_active) = (active.clone(), active.clone(), active.clone());
    let text = Text::new(label, LayoutStyle::new(), move || {
        let tint = if text_active() {
            theme.accent.most_readable(&[theme.text, theme.base])
        } else {
            theme.text
        };
        theme.text_style(FontRole::Caption, tint)
    })?;
    Ok(Box::new(
        StyledContainer::new(
            LayoutStyle::new()
                .padding_horizontal(space::lg())
                .padding_vertical(space::sm())
                .flex_shrink(0.0)
                .align_items(AlignItems::CENTER)
                .justify_content(JustifyContent::CENTER),
            move |_| {
                let fill = if fill_active() {
                    theme.accent
                } else {
                    theme.base
                };
                RectStyle::filled(fill, ROW_RADIUS)
            },
            vec![box_item(text)],
        )?
        .hover_style(move |_| {
            let fill = if hover_active() {
                theme.accent.darken(0.08)
            } else {
                theme.overlay
            };
            RectStyle::filled(fill, ROW_RADIUS)
        })
        .on_press(on_press),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn point(ssid: &str, security: Security, saved: bool) -> AccessPoint {
        AccessPoint {
            path: format!("/ap/{ssid}"),
            ssid: ssid.to_string(),
            strength: 60,
            security,
            frequency: 5180,
            saved,
            active: false,
        }
    }

    #[test]
    fn only_a_network_the_shell_can_authenticate_opens_a_password_field() {
        assert!(needs_prompt(&point("cafe", Security::Wpa2, false)));
        assert!(
            !needs_prompt(&point("home", Security::Wpa2, true)),
            "a saved network already has its key"
        );
        assert!(
            !needs_prompt(&point("open", Security::Open, false)),
            "nothing to ask for"
        );
        assert!(
            !needs_prompt(&point("corp", Security::Enterprise, false)),
            "802.1X needs a certificate and an identity, which a password field cannot supply"
        );
    }

    #[test]
    fn a_row_is_keyed_on_what_it_draws_but_not_on_the_signal() {
        let base = Row {
            point: point("cafe", Security::Wpa2, false),
            asking: false,
        };
        let stronger = Row {
            point: AccessPoint {
                strength: 95,
                ..base.point.clone()
            },
            ..base.clone()
        };
        assert_eq!(
            base.key(),
            stronger.key(),
            "a scan must not rebuild the row — it would destroy the password field being typed into"
        );

        let asking = Row {
            asking: true,
            ..base.clone()
        };
        assert_ne!(
            base.key(),
            asking.key(),
            "the prompt opening does rebuild it"
        );

        let joined = Row {
            point: AccessPoint {
                active: true,
                ..base.point.clone()
            },
            ..base.clone()
        };
        assert_ne!(base.key(), joined.key());
    }

    #[test]
    fn a_row_says_how_it_is_secured_and_where() {
        telar::set_locale("en");
        let saved = detail_line(&point("home", Security::Wpa3, true));
        assert!(saved.contains("WPA3") && saved.contains("5 GHz") && saved.contains("Saved"));

        let connected = detail_line(&AccessPoint {
            active: true,
            ..point("home", Security::Wpa3, true)
        });
        assert_eq!(
            connected, "Connected",
            "the state it is in outranks how it is secured"
        );
    }

    #[test]
    fn the_list_drops_nameless_networks_and_caps_the_rest() {
        let wifi = Wifi {
            available: true,
            enabled: true,
            points: vec![
                point("one", Security::Wpa2, false),
                point("two", Security::Open, false),
                point("", Security::Open, false),
            ],
            ..Wifi::default()
        };
        assert_eq!(listed(&wifi, NetworkConfig::default()).len(), 2);
        let shown = NetworkConfig {
            show_hidden: true,
            ..NetworkConfig::default()
        };
        assert_eq!(listed(&wifi, shown).len(), 3);
        let capped = NetworkConfig {
            max_networks: 1,
            ..NetworkConfig::default()
        };
        assert_eq!(listed(&wifi, capped).len(), 1);
    }

    /// The same trap the bluetooth panel hit: a closure reading a second signal inside another's `with` panics at build time and nowhere else.
    #[test]
    fn the_panel_builds_without_a_re_entrant_borrow() {
        telar::reset_layout_runtime();
        telar::set_theme(NordTheme::new());
        assert!(network_panel().is_ok());
    }
}
