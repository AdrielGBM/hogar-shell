//! The mixer: every device and every stream in the audio graph, each with its own level and mute.
//!
//! The shell has been able to *read* the whole graph since the PipeWire service replaced the `wpctl` poll, and `hogar-shell audio` has been able to drive all of it — but with a pointer there was no way to reach anything but the default sink. Choosing a different output meant a keybind or a script. This is that missing half: one surface per adjustable node, so the graph the service already carries is something a user can touch.
//!
//! Nothing here holds its own state. Every row's level, mute and default marker is read out of the live graph by node id, which is what lets a row survive its own drag: a slider that rebuilt on every value it set would drop the gesture that was setting it (the same trap the network panel's signal strength documents).

use telar::{
    AlignItems, Container, JustifyContent, LayoutError, LayoutItem, LayoutStyle, ReactiveList,
    RectStyle, RwSignal, SizeDimension, StyledContainer, Text, box_item, signal, use_theme,
};
use ui::scale::space;

use config::AudioConfig;
use config::surface_env;
use config::theme::{FontRole, NordTheme};
use services::pipewire::{self, Graph, Node, NodeKind};
use services::volume::{self, Volume};
use ui::glyph;
use ui::icon::icon_view;
use ui::widget;
use util::reactive::derive;

const ROW_ICON: f32 = 20.0;
const ROW_RADIUS: f32 = 8.0;
const METER_HEIGHT: f32 = 6.0;

/// The three lists, in the order a user reaches for them: what they are listening on, what is playing, and — last, because it is the one they set once — what they are recording with.
const GROUPS: [Group; 3] = [
    Group {
        label: "outputs",
        kinds: &[NodeKind::Sink],
    },
    Group {
        label: "streams",
        kinds: &[NodeKind::OutputStream, NodeKind::InputStream],
    },
    Group {
        label: "inputs",
        kinds: &[NodeKind::Source],
    },
];

struct Group {
    /// Key under `mixer.group`.
    label: &'static str,
    kinds: &'static [NodeKind],
}

/// One row of a list: the node, and whether it is the default device of its kind.
///
/// The level and the mute state are deliberately *not* here. They move while the row is being dragged, and a keyed list rebuilds a row whose key changed — which would destroy the drag mid-gesture. Both are read from the graph signal inside the row instead, so a scrub repaints the bar and rebuilds nothing.
#[derive(Clone, Debug, PartialEq)]
struct Row {
    node: Node,
    default: bool,
}

impl Row {
    fn key(&self) -> String {
        format!(
            "{}|{}|{}|{}",
            self.node.id,
            self.node.label(),
            self.node.media,
            self.default
        )
    }
}

/// The panel behind the volume chip's right-click, and the audio settings page's live half.
pub fn mixer_panel() -> Result<Box<dyn LayoutItem>, LayoutError> {
    let config = surface_env()
        .map(|env| env.config.audio)
        .unwrap_or_default();
    if let Some(env) = surface_env() {
        services::locale::attach(env.config.language());
    }
    mixer_view(config, use_theme::<NordTheme>())
}

/// The mixer itself, taking its config and theme rather than reading the surface's, so a caller that already resolved them does not have to be a surface for this to build.
pub fn mixer_view(
    config: AudioConfig,
    theme: NordTheme,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let graph = signal(pipewire::current().unwrap_or_default());
    let sink = graph;
    platform_wayland::watch(pipewire::subscribe, move |g| sink.set(g));

    let mut children: Vec<Box<dyn LayoutItem>> = Vec::with_capacity(GROUPS.len() + 2);
    children.push(title(theme)?);
    for group in &GROUPS {
        children.push(group_list(group, graph, config, theme)?);
    }
    children.push(empty_line(graph, theme)?);

    Ok(Box::new(Container::new(
        LayoutStyle::new()
            .flex_column()
            .gap(space::lg())
            .width(SizeDimension::Percent(1.0)),
        children,
    )?))
}

fn title(theme: NordTheme) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let text = Text::new(
        || telar::t!("mixer.title"),
        LayoutStyle::new(),
        move || {
            theme
                .text_style(FontRole::Title, theme.text)
                .with_font_weight(700)
        },
    )?;
    Ok(Box::new(text))
}

/// The nodes a group lists, in the graph's own order — which is by id, so a machine's own devices stay above the applications that came later and nothing reshuffles under the pointer.
fn listed(graph: &Graph, group: &Group) -> Vec<Row> {
    graph
        .nodes
        .iter()
        .filter(|node| group.kinds.contains(&node.kind))
        .map(|node| Row {
            default: is_default(graph, node),
            node: node.clone(),
        })
        .collect()
}

fn is_default(graph: &Graph, node: &Node) -> bool {
    match node.kind {
        NodeKind::Sink => node.name == graph.default_sink,
        NodeKind::Source => node.name == graph.default_source,
        _ => false,
    }
}

/// One group's subheading and rows. Both are hidden while the group is empty: a "Applications" heading over nothing reads as a mixer that lost the stream it was showing.
fn group_list(
    group: &'static Group,
    graph: RwSignal<Graph>,
    config: AudioConfig,
    theme: NordTheme,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let heading_source = graph.read_only();
    let heading = Text::new(
        move || {
            if listed(&heading_source.get(), group).is_empty() {
                String::new()
            } else {
                telar::i18n::translate(
                    &crate::__rsx_i18n::CATALOG,
                    &format!("mixer.group.{}", group.label),
                    &[],
                )
            }
        },
        LayoutStyle::new(),
        move || {
            theme
                .text_style(FontRole::Caption, theme.muted)
                .with_font_weight(700)
        },
    )?;

    let source = graph.read_only();
    let rows = ReactiveList::new(
        move || listed(&source.get(), group),
        |row: &Row| row.key(),
        move |row: Row| node_row(row, graph, config, theme),
        6.0,
    )?;

    Ok(Box::new(Container::new(
        LayoutStyle::new()
            .flex_column()
            .gap(space::md())
            .width(SizeDimension::Percent(1.0)),
        vec![box_item(heading), Box::new(rows)],
    )?))
}

/// One adjustable node: its glyph, its name, what it is doing, and the slider that sets it.
///
/// Pressing the glyph mutes it; pressing the labels makes a device the default one; dragging the bar sets the level. Three gestures on one row rather than a row of buttons, because a mixer with eight streams on it is a list a user scans, and every control that is not the slider is one they use once.
fn node_row(
    row: Row,
    graph: RwSignal<Graph>,
    config: AudioConfig,
    theme: NordTheme,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let id = row.node.id;
    let kind = row.node.kind;
    let ceiling = config.ceiling().max(1) as f32;

    // Everything that moves is read back out of the graph by id, so the row outlives its own drag.
    let live = {
        let graph = graph.read_only();
        move || graph.get().node(id).cloned()
    };
    let reading = move || {
        live().map(|node| Volume {
            level: node.level,
            muted: node.muted,
        })
    };

    let glyph_reading = reading;
    let icon = icon_view(
        move || {
            let volume = glyph_reading().unwrap_or(Volume {
                level: 0,
                muted: true,
            });
            match kind {
                NodeKind::Source | NodeKind::InputStream => glyph::microphone(volume).to_string(),
                _ => glyph::volume(volume).to_string(),
            }
        },
        move || {
            if reading().is_some_and(|v| v.muted) {
                theme.red
            } else if row.default {
                theme.accent
            } else {
                theme.text
            }
        },
        ROW_ICON,
    )?;
    let mute = StyledContainer::new(
        LayoutStyle::new()
            .flex_shrink(0.0)
            .align_items(AlignItems::CENTER)
            .justify_content(JustifyContent::CENTER),
        |_r| RectStyle::default(),
        vec![icon],
    )?
    .on_press(move || volume::toggle_node_mute(id));

    let label = row.node.label();
    let name = Text::new(
        move || label.clone(),
        LayoutStyle::new(),
        move || theme.text_style(FontRole::Body, theme.text),
    )?;
    let detail = {
        let node = row.node.clone();
        let default = row.default;
        Text::new(
            move || detail_line(&node, default),
            LayoutStyle::new(),
            move || theme.text_style(FontRole::Caption, theme.subtle),
        )?
    };
    let labels = Container::new(
        LayoutStyle::new()
            .flex_column()
            .flex_grow(1.0)
            .gap(space::xs()),
        vec![box_item(name), box_item(detail)],
    )?;
    // Only a device has a default to be made; wrapping a stream's labels in a press target would give a user something to click that answers with nothing.
    let labels: Box<dyn LayoutItem> = if matches!(kind, NodeKind::Sink | NodeKind::Source) {
        Box::new(
            StyledContainer::new(
                LayoutStyle::new().flex_grow(1.0).min_width(0.0),
                |_r| RectStyle::default(),
                vec![Box::new(labels)],
            )?
            .on_press(move || volume::set_default(id)),
        )
    } else {
        Box::new(labels)
    };

    let percent_reading = reading;
    let percent = Text::new(
        move || match percent_reading() {
            Some(v) => format!("{}%", v.level),
            None => String::new(),
        },
        LayoutStyle::new().flex_shrink(0.0),
        move || theme.text_style(FontRole::Caption, theme.muted),
    )?;

    let head = Container::new(
        LayoutStyle::new()
            .flex_row()
            .align_items(AlignItems::CENTER)
            .gap(space::lg())
            .width(SizeDimension::Percent(1.0)),
        vec![Box::new(mute), labels, box_item(percent)],
    )?;

    let fraction = derive(graph.read_only(), move |g: Graph| {
        g.node(id).map(|n| n.level as f32 / ceiling).unwrap_or(0.0)
    });
    let tint = derive(graph.read_only(), move |g: Graph| {
        if g.node(id).is_some_and(|n| n.muted) {
            theme.muted
        } else {
            theme.accent
        }
    });
    let bar = widget::slider(fraction, tint, theme.overlay, METER_HEIGHT, move |f| {
        volume::set_node(id, (f * ceiling).round() as i32);
    })?;

    Ok(Box::new(StyledContainer::new(
        LayoutStyle::new()
            .flex_column()
            .gap(space::xs())
            .padding_horizontal(space::lg())
            .padding_vertical(space::md())
            .width(SizeDimension::Percent(1.0)),
        move |_r| RectStyle::filled(theme.base, ROW_RADIUS),
        vec![Box::new(head), bar],
    )?))
}

/// What a row says about itself under its name: which track a stream is playing, and which device is the one everything else goes to.
fn detail_line(node: &Node, default: bool) -> String {
    if node.kind.is_stream() {
        let media = node.media.trim();
        if !media.is_empty() {
            return media.to_string();
        }
        return node.description.trim().to_string();
    }
    if default {
        return telar::t!("mixer.default");
    }
    telar::t!("mixer.set_default")
}

/// The line under an empty mixer — never blank, so the surface always says why there is nothing on it.
fn empty_line(
    graph: RwSignal<Graph>,
    theme: NordTheme,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let source = graph.read_only();
    let text = Text::new(
        move || {
            if source.get().nodes.is_empty() {
                telar::t!("mixer.unavailable")
            } else {
                String::new()
            }
        },
        LayoutStyle::new(),
        move || theme.text_style(FontRole::Caption, theme.muted),
    )?;
    Ok(box_item(text))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: u32, name: &str, kind: NodeKind) -> Node {
        Node {
            id,
            name: name.to_string(),
            description: name.to_string(),
            app: String::new(),
            media: String::new(),
            icon: String::new(),
            kind,
            level: 50,
            muted: false,
        }
    }

    fn graph() -> Graph {
        Graph {
            nodes: vec![
                node(1, "hdmi", NodeKind::Sink),
                node(2, "analog", NodeKind::Sink),
                node(3, "mic", NodeKind::Source),
                Node {
                    app: "Firefox".into(),
                    media: "A video".into(),
                    ..node(4, "firefox", NodeKind::OutputStream)
                },
            ],
            default_sink: "analog".to_string(),
            default_source: "mic".to_string(),
        }
    }

    #[test]
    fn each_group_lists_its_own_kind_and_marks_the_default() {
        let graph = graph();
        let outputs = listed(&graph, &GROUPS[0]);
        assert_eq!(
            outputs.iter().map(|r| r.node.id).collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert!(
            !outputs[0].default && outputs[1].default,
            "the default is the one the metadata names, not the first one"
        );

        let streams = listed(&graph, &GROUPS[1]);
        assert_eq!(streams.len(), 1);
        assert!(
            !streams[0].default,
            "a stream is never anything's default device"
        );
        assert_eq!(listed(&graph, &GROUPS[2]).len(), 1);
    }

    #[test]
    fn a_row_is_keyed_on_what_it_draws_but_not_on_the_level() {
        // The whole reason the level is read from the graph rather than baked into the row: a key that moved with it would rebuild the row on every pointer move of its own drag, and a rebuilt row has no drag.
        let base = Row {
            node: node(1, "analog", NodeKind::Sink),
            default: true,
        };
        let louder = Row {
            node: Node {
                level: 80,
                muted: true,
                ..base.node.clone()
            },
            ..base.clone()
        };
        assert_eq!(base.key(), louder.key());

        let demoted = Row {
            default: false,
            ..base.clone()
        };
        assert_ne!(
            base.key(),
            demoted.key(),
            "losing the default marker does redraw the row"
        );
    }

    #[test]
    fn a_stream_says_what_it_is_playing_and_a_device_says_whether_it_is_the_default() {
        telar::set_locale("en");
        let stream = Node {
            app: "Firefox".into(),
            media: "A video".into(),
            ..node(4, "firefox", NodeKind::OutputStream)
        };
        assert_eq!(detail_line(&stream, false), "A video");
        let silent = Node {
            media: String::new(),
            ..stream
        };
        assert_eq!(
            detail_line(&silent, false),
            "firefox",
            "a stream with no track falls back to what it is, never to a blank line"
        );

        let device = node(2, "analog", NodeKind::Sink);
        assert_ne!(detail_line(&device, true), detail_line(&device, false));
    }

    /// The re-entrant-borrow guard every panel in this shell carries: a closure that reads a second signal inside another's `with` panics at build time and nowhere else.
    #[test]
    fn the_mixer_builds_without_a_re_entrant_borrow() {
        telar::reset_layout_runtime();
        telar::set_theme(NordTheme::new());
        assert!(mixer_view(AudioConfig::default(), NordTheme::new()).is_ok());
    }
}
