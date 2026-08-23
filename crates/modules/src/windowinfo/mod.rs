//! What the compositor knows about the focused window, and the four things worth doing to it.
//!
//! The details were already in the client list; what this panel adds is the preview and the actions. The preview
//! is a real capture of the window's own rectangle, taken on a producer thread on a period from
//! `[utilities] window_preview_ms` — a screen copy per refresh is not something to do on the frame, and a
//! preview that re-read the screen every frame would cost more than the panel drawing it.
//!
//! The actions are Hyprland dispatchers. Each is *verified* rather than trusted: `hl.dsp.window.<action>` will
//! not say what arguments it takes — called outside a dispatch it refuses to build at all — so the service tries
//! a shape and checks whether the compositor's own client list moved. Closing is the exception: it gets one
//! attempt, because trying a second spelling of a close is how the wrong window gets closed twice.

use std::sync::Arc;
use std::time::Duration;
use ui::scale::space;

use platform_wayland::EventSender;
use telar::{
    AlignItems, Container, Image, ImageData, ImageFilter, JustifyContent, LayoutError, LayoutItem,
    LayoutStyle, ObjectFit, RectStyle, SizeDimension, StyledContainer, Text, box_item, signal,
    use_theme,
};

use config::theme::{FontRole, NordTheme};
use services::hyprland::{self, Client};
use services::screenshot::{self, Area, Target};
use ui::icon::icon_view;
use ui::module::{icon_px, module_fg, surface_env};
use ui::widget::label_value;

pub const ID: &str = "windowinfo";

const PREVIEW_HEIGHT: f32 = 150.0;
const ROW_RADIUS: f32 = 10.0;

/// The bar chip: a window glyph that opens the panel.
pub fn window_chip() -> Result<Box<dyn LayoutItem>, LayoutError> {
    let fg = module_fg();
    icon_view(
        || ui::glyph::window_info().to_string(),
        move || fg.get(),
        icon_px(),
    )
}

/// The panel: a preview, the window's own facts, and what can be done to it.
pub fn window_panel() -> Result<Box<dyn LayoutItem>, LayoutError> {
    let theme = use_theme::<NordTheme>();
    if let Some(env) = surface_env() {
        services::locale::attach(env.config.language());
    }
    let interval = surface_env()
        .map(|env| env.config.utilities.window_preview_interval())
        .unwrap_or(Some(Duration::from_secs(1)));

    // The focused window, live: the panel follows focus rather than pinning whatever was focused when it opened,
    // which is what makes it usable for looking at one window after another.
    let focused = signal(current_focus());
    let sink = focused.clone();
    platform_wayland::watch(hyprland::subscribe_clients, move |_: Vec<Client>| {
        sink.set(current_focus())
    });
    let follow = focused.clone();
    platform_wayland::watch(
        hyprland::subscribe_active_window,
        move |_: hyprland::ActiveWindow| follow.set(current_focus()),
    );

    let title = Text::auto(
        {
            let source = focused.read_only();
            move || match source.get() {
                Some(client) => client.title.clone(),
                None => telar::t!("window.none"),
            }
        },
        LayoutStyle::new(),
        move || {
            theme
                .text_style(FontRole::Title, theme.text)
                .with_weight(700)
                .with_max_lines(1)
                .with_ellipsis(true)
        },
    )?;

    let children: Vec<Box<dyn LayoutItem>> = vec![
        box_item(title),
        preview(interval, theme)?,
        details(focused.read_only(), theme)?,
        actions(focused.read_only(), theme)?,
        workspace_row(focused.read_only(), theme)?,
    ];
    Ok(Box::new(Container::new(
        LayoutStyle::new()
            .flex_column()
            .gap(space::lg())
            .width(SizeDimension::Percent(1.0)),
        children,
    )?))
}

/// The focused window as the compositor's own client list describes it, so the geometry the preview crops and
/// the facts the rows print come from one reading.
fn current_focus() -> Option<Client> {
    let dir = hyprland::socket_dir()?;
    let address = hyprland::active_window(&dir).address;
    if address.is_empty() {
        return None;
    }
    hyprland::current_clients()
        .unwrap_or_else(|| hyprland::clients(&dir))
        .into_iter()
        .find(|client| client.address == address)
}

/// The live preview. `None` — no window, or a capture that failed — draws a placeholder rather than a gap, the
/// same rule the shell's other pictures follow.
fn preview(
    interval: Option<Duration>,
    theme: NordTheme,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let frame = signal(None::<Arc<ImageData>>);
    let sink = frame.clone();
    platform_wayland::watch(
        move |tx| capture_loop(tx, interval),
        move |image: Option<Arc<ImageData>>| sink.set(image),
    );

    let source = frame.read_only();
    let picture = Image::new(
        LayoutStyle::new()
            .width(SizeDimension::Percent(1.0))
            .height(PREVIEW_HEIGHT),
        move || {
            source
                .get()
                .unwrap_or_else(|| Arc::new(ImageData::new(Vec::new(), 0, 0)))
        },
        || ImageFilter::Linear,
        // Contained, not cropped: a preview is for recognising the window, and a cover crop of a tall window
        // shows a strip of its middle.
        || ObjectFit::Contain,
    )?;

    Ok(Box::new(StyledContainer::new(
        LayoutStyle::new()
            .width(SizeDimension::Percent(1.0))
            .align_items(AlignItems::CENTER)
            .justify_content(JustifyContent::CENTER),
        move |_| RectStyle::filled(theme.base, ROW_RADIUS),
        vec![Box::new(picture)],
    )?))
}

/// Captures the focused window's rectangle on `interval`, or once when there is none.
///
/// A producer, not a timer on the UI thread: each turn is a compositor round trip plus a copy of the whole
/// window. The loop ends when `send` fails, which is how it learns the panel closed — the same contract every
/// other producer here follows.
fn capture_loop(tx: EventSender<Option<Arc<ImageData>>>, interval: Option<Duration>) {
    loop {
        if !tx.send(window_frame()) {
            return;
        }
        let Some(interval) = interval else { return };
        std::thread::sleep(interval);
    }
}

fn window_frame() -> Option<Arc<ImageData>> {
    let client = current_focus()?;
    let area = Area {
        x: client.at.0,
        y: client.at.1,
        width: client.size.0,
        height: client.size.1,
    };
    match screenshot::snapshot(Target::Area(area), false) {
        Ok(image) => Some(Arc::new(ImageData::new(
            image.pixels,
            image.width,
            image.height,
        ))),
        Err(reason) => {
            tracing::debug!("window preview: {reason}");
            None
        }
    }
}

/// The facts, in the order someone debugging a window rule wants them.
///
/// Each row names its own key literally: `t!` resolves at compile time — which is what lets the analyzer catch a
/// missing translation — so a loop over a list of key *strings* would not compile.
fn details(
    focused: telar::ReadSignal<Option<Client>>,
    theme: NordTheme,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let size = theme.font(FontRole::Caption);
    let row = |label: fn() -> String, read: fn(&Client) -> String| {
        label_value(
            util::reactive::derive(focused.clone(), move |_| label()),
            util::reactive::derive(focused.clone(), move |client: Option<Client>| {
                client.as_ref().map(read).unwrap_or_default()
            }),
            size,
            theme.subtle,
            theme.text,
        )
    };
    let children: Vec<Box<dyn LayoutItem>> = vec![
        row(|| telar::t!("window.class"), |client| client.class.clone())?,
        row(|| telar::t!("window.pid"), |client| client.pid.to_string())?,
        row(
            || telar::t!("window.workspace"),
            |client| {
                if client.workspace_name.is_empty() {
                    client.workspace.to_string()
                } else {
                    format!("{} ({})", client.workspace_name, client.workspace)
                }
            },
        )?,
        row(
            || telar::t!("window.geometry"),
            |client| {
                format!(
                    "{}×{} at {},{}",
                    client.size.0, client.size.1, client.at.0, client.at.1
                )
            },
        )?,
        row(
            || telar::t!("window.address"),
            |client| client.address.clone(),
        )?,
        row(|| telar::t!("window.state"), state_line)?,
    ];
    Ok(Box::new(Container::new(
        LayoutStyle::new()
            .flex_column()
            .gap(space::sm())
            .width(SizeDimension::Percent(1.0)),
        children,
    )?))
}

/// Everything about a window that is a flag rather than a value, in one line — and never blank, so the row
/// always says something.
fn state_line(client: &Client) -> String {
    let mut states = Vec::new();
    if client.floating {
        states.push(telar::t!("window.floating"));
    }
    if client.fullscreen {
        states.push(telar::t!("window.fullscreen"));
    }
    if client.pinned {
        states.push(telar::t!("window.pinned"));
    }
    if client.xwayland {
        states.push("XWayland".to_string());
    }
    if states.is_empty() {
        telar::t!("window.tiled")
    } else {
        states.join(", ")
    }
}

fn actions(
    focused: telar::ReadSignal<Option<Client>>,
    theme: NordTheme,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let float = focused.clone();
    let fullscreen = focused.clone();
    let close = focused.clone();
    let children = vec![
        pill(
            || telar::t!("window.float"),
            "move",
            move || {
                if let Some(client) = float.peek() {
                    act(move |dir| {
                        hyprland::set_floating(dir, &client.address, !client.floating);
                    });
                }
            },
            theme,
        )?,
        pill(
            || telar::t!("window.fullscreen"),
            "maximize",
            move || {
                if let Some(client) = fullscreen.peek() {
                    act(move |dir| {
                        hyprland::set_fullscreen(dir, &client.address, !client.fullscreen);
                    });
                }
            },
            theme,
        )?,
        pill(
            || telar::t!("window.close"),
            "x",
            move || {
                if let Some(client) = close.peek() {
                    act(move |dir| hyprland::close_window(dir, &client.address));
                }
            },
            theme,
        )?,
    ];
    Ok(Box::new(Container::new(
        LayoutStyle::new()
            .flex_row()
            .gap(space::md())
            .width(SizeDimension::Percent(1.0)),
        children,
    )?))
}

/// The workspaces this window can be moved to, as the compositor currently lists them.
///
/// The existing workspaces rather than a fixed 1–10: a dispatcher can create a workspace on demand, but a row of
/// ten numbers on a session that uses three is a row of buttons that mean nothing.
fn workspace_row(
    focused: telar::ReadSignal<Option<Client>>,
    theme: NordTheme,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let workspaces: Vec<i32> = hyprland::current_workspaces()
        .map(|snapshot| {
            snapshot
                .workspaces
                .iter()
                .filter(|workspace| !workspace.is_special())
                .map(|workspace| workspace.id)
                .collect()
        })
        .unwrap_or_default();
    if workspaces.is_empty() {
        return Ok(Box::new(Container::new(LayoutStyle::new(), vec![])?));
    }

    let label = Text::auto(
        || telar::t!("window.move_to"),
        LayoutStyle::new().flex_shrink(0.0),
        move || theme.text_style(FontRole::Caption, theme.subtle),
    )?;
    let mut children: Vec<Box<dyn LayoutItem>> = vec![box_item(label)];
    for workspace in workspaces {
        let source = focused.clone();
        children.push(pill(
            move || workspace.to_string(),
            "",
            move || {
                if let Some(client) = source.peek() {
                    act(move |dir| {
                        hyprland::move_window_to_workspace(dir, &client.address, workspace);
                    });
                }
            },
            theme,
        )?);
    }
    Ok(Box::new(Container::new(
        LayoutStyle::new()
            .flex_row()
            .align_items(AlignItems::CENTER)
            .gap(space::md())
            .width(SizeDimension::Percent(1.0)),
        children,
    )?))
}

/// Runs a dispatcher off the UI thread. Each of these verifies itself by re-reading the client list, which is a
/// socket round trip — cheap, but not something to do inside a press handler on the frame.
fn act(action: impl FnOnce(&std::path::Path) + Send + 'static) {
    let _ = std::thread::Builder::new()
        .name("hogar-shell-window-act".to_string())
        .spawn(move || {
            let Some(dir) = hyprland::socket_dir() else {
                return;
            };
            action(&dir);
        });
}

fn pill(
    label: impl Fn() -> String + 'static,
    icon: &'static str,
    press: impl Fn() + 'static,
    theme: NordTheme,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let text = Text::auto(label, LayoutStyle::new(), move || {
        theme.text_style(FontRole::Caption, theme.text)
    })?;
    let mut children: Vec<Box<dyn LayoutItem>> = Vec::new();
    if !icon.is_empty() {
        children.push(icon_view(
            move || icon.to_string(),
            move || theme.text,
            16.0,
        )?);
    }
    children.push(box_item(text));
    Ok(Box::new(
        StyledContainer::new(
            LayoutStyle::new()
                .flex_row()
                .align_items(AlignItems::CENTER)
                .justify_content(JustifyContent::CENTER)
                .gap(space::sm())
                .padding_horizontal(space::lg())
                .padding_vertical(space::md())
                .flex_shrink(0.0),
            move |_| RectStyle::filled(theme.base, ROW_RADIUS),
            children,
        )?
        .hover_style(move |_| RectStyle::filled(theme.overlay, ROW_RADIUS))
        .on_press(press),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client() -> Client {
        Client {
            address: "0x55f1".to_string(),
            class: "foot".to_string(),
            title: "shell".to_string(),
            pid: 4242,
            workspace: 3,
            workspace_name: "code".to_string(),
            at: (100, 80),
            size: (1200, 900),
            mapped: true,
            ..Client::default()
        }
    }

    #[test]
    fn the_state_row_always_says_something() {
        assert_eq!(state_line(&client()), telar::t!("window.tiled"));

        let floating = Client {
            floating: true,
            xwayland: true,
            ..client()
        };
        let line = state_line(&floating);
        assert!(line.contains(&telar::t!("window.floating")), "{line}");
        assert!(line.contains("XWayland"), "{line}");
    }

    #[test]
    fn the_panel_builds_with_a_window_and_with_none() {
        telar::reset_layout_runtime();
        telar::set_theme(NordTheme::new());
        assert!(
            window_panel().is_ok(),
            "no compositor: the panel still builds"
        );

        telar::reset_layout_runtime();
        telar::set_theme(NordTheme::new());
        let focused = signal(Some(client()));
        assert!(details(focused.read_only(), NordTheme::new()).is_ok());
        assert!(actions(focused.read_only(), NordTheme::new()).is_ok());
    }

    #[test]
    fn the_preview_takes_one_still_when_the_refresh_is_switched_off() {
        // `window_preview_ms = 0` is a machine on battery: one capture, then nothing. The loop has to end rather
        // than fall through to a zero sleep, which would spin a capture per turn.
        let off = config::UtilitiesConfig {
            window_preview_ms: 0,
            ..config::UtilitiesConfig::default()
        };
        assert!(off.window_preview_interval().is_none());

        let fast = config::UtilitiesConfig {
            window_preview_ms: 10,
            ..off
        };
        assert_eq!(
            fast.window_preview_interval(),
            Some(Duration::from_millis(250)),
            "a refresh faster than the capture itself is floored, not honoured"
        );
    }
}
