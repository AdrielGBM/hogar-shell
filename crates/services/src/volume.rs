//! The default sink and the default source, derived from the audio graph.
//!
//! This used to fork `wpctl get-volume` every two seconds per node. It now reads [`pipewire`](super::pipewire), so a level changed from another mixer reaches the bar as PipeWire reports it rather than up to two seconds later, and the shell runs no timer for audio at all.
//!
//! Mutations still go through `wpctl`: it resolves `@DEFAULT_AUDIO_SINK@` and applies the same volume curve the graph stores, and writing was never the part that needed fixing. What did change is that a mutation no longer re-reads afterwards — the monitor reports the real value on its own, so a set costs one fork instead of two.

use std::sync::Arc;

use platform_wayland::EventSender;
use util::deps::{self, Dep};

use crate::pipewire::{self, Graph, Node, NodeKind};
use config::AudioConfig;
use util::broadcast::{Broadcast, Service};

const SINK: &str = "@DEFAULT_AUDIO_SINK@";
const SOURCE: &str = "@DEFAULT_AUDIO_SOURCE@";

/// A node's level as a percentage and its mute state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Volume {
    /// 0–100 (may read above 100 if boosted; callers clamp for display).
    pub level: i32,
    pub muted: bool,
}

impl From<&Node> for Volume {
    fn from(node: &Node) -> Self {
        Self {
            level: node.level,
            muted: node.muted,
        }
    }
}

static VOLUME: Service<Volume> = Service::new("hogar-shell-volume", run);
static MIC: Service<Volume> = Service::new("hogar-shell-mic", run_mic);

/// Publishes one node's reading off every graph batch, skipping the batches that did not move it.
///
/// A batch in which `pick` finds nothing publishes nothing, so the last reading stands — which is right for the churn this skips and wrong the day the default device disappears, where the chip goes on showing a device that is gone. Saying so needs an `Option` through the broadcast; until then a surface that must not assert a state it has not been told (the mic chip, whose wrong guess is someone speaking into a live microphone) starts from `current_mic()` being `None` rather than from a stand-in reading.
///
/// The graph republishes whenever anything in it changes — an application opening a stream, a device being plugged in — and most of that says nothing about the default sink. Without this, opening a browser tab would redraw every volume chip in the shell.
fn derive(out: &Arc<Broadcast<Volume>>, pick: fn(&Graph) -> Option<&Node>) {
    let published = Arc::clone(out);
    let mut last: Option<Volume> = None;
    pipewire::on_graph(Box::new(move |graph| {
        let current = pick(graph).map(Volume::from);
        if let Some(volume) = current
            && current != last
        {
            published.publish(volume);
        }
        last = current;
    }));
}

fn run(out: &Arc<Broadcast<Volume>>) {
    derive(out, Graph::default_sink);
}

fn run_mic(out: &Arc<Broadcast<Volume>>) {
    derive(out, Graph::default_source);
}

/// Registers `tx` for live volume readings, attaching to the audio graph on first use. Called from a bar chip's `watch` producer.
pub fn subscribe(tx: EventSender<Volume>) {
    VOLUME.subscribe(tx);
}

pub fn subscribe_mic(tx: EventSender<Volume>) {
    MIC.subscribe(tx);
}

/// The last known reading, with no subprocess — what a UI handler steps from.
pub fn current() -> Option<Volume> {
    VOLUME.current()
}

pub fn current_mic() -> Option<Volume> {
    MIC.current()
}

/// How long a caller that started the graph listener itself will wait for its first reading.
///
/// Short, and it has to be: an IPC command runs on the driver thread, so this is a wait the whole shell takes. It only ever covers the moment between starting the listener and its thread registering — the graph replays its last batch on registration ([`pipewire::on_graph`]), so there is nothing else to wait for. A machine with no PipeWire at all spends the whole budget once and then answers honestly.
const COLD_START: std::time::Duration = std::time::Duration::from_millis(200);

/// The default sink's level for a caller that is *asking* rather than drawing — an IPC command, a keybind.
///
/// Not [`current`]: that starts the graph listener and reads in the same breath, so the first `volume get` after a shell came up answered `None` and the shell reported "no audio sink available" about a machine with one. A UI handler is right to take that answer — it has a live subscription or it has nothing to draw — but a command is the reason the listener started and can afford to wait for it.
pub fn reading() -> Option<Volume> {
    VOLUME.awaited(COLD_START)
}

/// The default source's level, on the same terms as [`reading`].
pub fn mic_reading() -> Option<Volume> {
    MIC.awaited(COLD_START)
}

/// Stands a reading in for the graph's, without starting the PipeWire listener — what a `[preview]` draws its meter from. See [`util::broadcast::Service::seed`].
pub fn seed(volume: Volume) {
    VOLUME.seed(volume);
}

/// The running `[audio]` settings, or the defaults outside a started shell (a unit test, a service thread — [`config::config`] lives on the driver thread, which is where every caller of this runs).
pub fn settings() -> AudioConfig {
    config::config().map(|c| c.audio).unwrap_or_default()
}

/// Steps the volume by `delta` percentage points from the last known level.
pub fn step(delta: i32) {
    if let Some(v) = current() {
        set(v.level + delta);
    }
}

pub fn step_mic(delta: i32) {
    if let Some(v) = current_mic() {
        set_mic(v.level + delta);
    }
}

/// Runs a `wpctl` mutation off the UI thread — a blocking `fork`/`exec` in a click handler would stall the frame. Nothing is read back: the monitor reports what PipeWire actually did.
fn apply(args: Vec<String>) {
    let _ = std::thread::Builder::new()
        .name("hogar-shell-volume-set".to_string())
        .spawn(move || {
            if let Some(mut wpctl) = deps::command(Dep::Wpctl) {
                let _ = wpctl.args(&args).status();
            }
        });
}

pub fn toggle_mute() {
    if let Some(v) = current() {
        VOLUME.publish(Volume {
            muted: !v.muted,
            ..v
        });
    }
    apply(vec!["set-mute".into(), SINK.into(), "toggle".into()]);
}

pub fn toggle_mic_mute() {
    if let Some(v) = current_mic() {
        MIC.publish(Volume {
            muted: !v.muted,
            ..v
        });
    }
    apply(vec!["set-mute".into(), SOURCE.into(), "toggle".into()]);
}

/// Sets the default sink's volume to `level` percent, clamped to `[audio] max_volume`.
///
/// Publishes the target before `wpctl` has run, so a scroll notch moves the chip and the OSD on the same frame instead of a round-trip later; the reading the graph reports next reconciles what the sink accepted.
pub fn set(level: i32) -> i32 {
    let level = level.clamp(0, settings().ceiling());
    let muted = current().is_some_and(|v| v.muted);
    VOLUME.publish(Volume { level, muted });
    apply(vec!["set-volume".into(), SINK.into(), format!("{level}%")]);
    level
}

/// A microphone has no reason to be boosted past its own maximum, so this clamps to 0–100 rather than the sink's 0–150.
pub fn set_mic(level: i32) -> i32 {
    let level = level.clamp(0, 100);
    let muted = current_mic().is_some_and(|v| v.muted);
    MIC.publish(Volume { level, muted });
    apply(vec![
        "set-volume".into(),
        SOURCE.into(),
        format!("{level}%"),
    ]);
    level
}

/// Republishes the graph with `edit` applied to one node, so a mixer's slider follows the pointer instead of waiting for `pw-dump` to report the change back.
///
/// The same optimism [`set`] has, and needed more here: a drag emits a mutation per pointer move, and a bar that only moved once the monitor answered would trail the finger by a round trip each time. The reading that follows is authoritative — it reconciles whatever PipeWire actually accepted.
fn optimistically(id: u32, edit: impl FnOnce(&mut Node)) {
    let Some(mut graph) = pipewire::current() else {
        return;
    };
    let Some(node) = graph.nodes.iter_mut().find(|node| node.id == id) else {
        return;
    };
    edit(node);
    pipewire::publish(graph);
}

/// Sets one node's volume by id — an output device, an input device, or a single application's stream.
pub fn set_node(id: u32, level: i32) {
    let level = level.clamp(0, settings().ceiling());
    optimistically(id, |node| node.level = level);
    apply(vec![
        "set-volume".into(),
        id.to_string(),
        format!("{level}%"),
    ]);
}

pub fn toggle_node_mute(id: u32) {
    optimistically(id, |node| node.muted = !node.muted);
    apply(vec!["set-mute".into(), id.to_string(), "toggle".into()]);
}

/// Makes `id` the default sink or source. WirePlumber writes the choice into PipeWire's `default` metadata, which is what the graph reads back, so nothing here has to guess whether it took.
pub fn set_default(id: u32) {
    if let Some(mut graph) = pipewire::current()
        && let Some(node) = graph.node(id)
    {
        // The metadata names the node, so the optimistic edit has to as well — writing the id would leave the graph naming a default no sink answers to.
        let (name, kind) = (node.name.clone(), node.kind);
        match kind {
            NodeKind::Sink => graph.default_sink = name,
            NodeKind::Source => graph.default_source = name,
            // A stream has no "default" to be; `wpctl` would refuse it too.
            _ => return,
        }
        pipewire::publish(graph);
    }
    apply(vec!["set-default".into(), id.to_string()]);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: u32, name: &str, kind: NodeKind, level: i32) -> Node {
        Node {
            id,
            name: name.to_string(),
            description: name.to_string(),
            app: String::new(),
            media: String::new(),
            icon: String::new(),
            kind,
            level,
            muted: false,
        }
    }

    #[test]
    fn a_reading_follows_the_default_device_not_the_first_one() {
        let graph = Graph {
            nodes: vec![
                node(1, "hdmi", NodeKind::Sink, 10),
                node(2, "analog", NodeKind::Sink, 70),
                node(3, "mic", NodeKind::Source, 55),
            ],
            default_sink: "analog".to_string(),
            default_source: "mic".to_string(),
        };
        assert_eq!(
            graph.default_sink().map(Volume::from),
            Some(Volume {
                level: 70,
                muted: false
            }),
            "the default sink is the one named by the metadata, not the lowest id"
        );
        assert_eq!(graph.default_source().map(Volume::from).unwrap().level, 55);
    }

    #[test]
    fn a_machine_with_no_default_device_reports_nothing_rather_than_zero() {
        // A desktop mid-boot, or one whose only sink was just unplugged. Zero would read as "muted at 0", which is a state the user could act on; `None` is what every consumer already draws as "no audio".
        let empty = Graph::default();
        assert_eq!(empty.default_sink().map(Volume::from), None);
        assert_eq!(empty.default_source().map(Volume::from), None);

        let orphaned = Graph {
            nodes: vec![node(1, "analog", NodeKind::Sink, 70)],
            default_sink: "a-sink-that-went-away".to_string(),
            default_source: String::new(),
        };
        assert_eq!(
            orphaned.default_sink().map(Volume::from),
            None,
            "the metadata naming a node that is gone is not a reason to report another one"
        );
    }
}
