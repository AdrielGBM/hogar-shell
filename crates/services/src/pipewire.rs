//! The audio graph as one shared source, read from PipeWire itself.
//!
//! This replaces a `wpctl` fork every two seconds. Two things were wrong with that beyond the cost: a poll answers one question per fork, so a level the user changed from another mixer took up to two seconds to reach the bar; and `wpctl get-volume` returns a single number, which is why the shell could show a level and a mute state and nothing else — no device list, no per-application stream.
//!
//! `pw-dump --monitor` is the whole graph, once, and then a line per change. One subprocess for the shell, parsed on one thread, published as one snapshot. `wpctl` stays for *mutations*: it already resolves `@DEFAULT_AUDIO_SINK@` and does the volume curve, and writing was never the part that needed fixing.
//!
//! A machine with no PipeWire publishes nothing at all, which is the same thing the poll did when `wpctl` was missing — every consumer already treats `None` as "no audio here".

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::process::Stdio;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use platform_wayland::EventSender;
use serde_json::Value;
use util::deps::{self, Dep};

use util::broadcast::{Broadcast, Service};

/// How long to wait before re-attaching after the monitor exits. Only ever reached when PipeWire itself restarted, which is when the shell most needs to come back rather than stay blank.
const REATTACH: Duration = Duration::from_secs(3);

/// What a node is to a mixer. Everything else in the graph — ports, links, the MIDI bridge, the dummy driver — is not something a user adjusts, so it never reaches a snapshot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeKind {
    /// An output device: speakers, headphones, an HDMI port.
    Sink,
    /// An input device: a microphone.
    Source,
    /// An application playing audio.
    OutputStream,
    /// An application recording audio.
    InputStream,
}

impl NodeKind {
    fn parse(media_class: &str) -> Option<Self> {
        match media_class {
            "Audio/Sink" => Some(Self::Sink),
            "Audio/Source" | "Audio/Source/Virtual" => Some(Self::Source),
            "Stream/Output/Audio" => Some(Self::OutputStream),
            "Stream/Input/Audio" => Some(Self::InputStream),
            _ => None,
        }
    }

    pub fn is_stream(self) -> bool {
        matches!(self, Self::OutputStream | Self::InputStream)
    }
}

/// One adjustable thing in the graph.
#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    /// PipeWire's object id — what a mutation targets, and stable for the node's lifetime.
    pub id: u32,
    /// `node.name`: the stable identifier the default-device metadata refers to.
    pub name: String,
    /// `node.description`, else `node.nick`, else the name — what a device row shows.
    pub description: String,
    /// `application.name` for a stream; empty for a device.
    pub app: String,
    /// `media.name` for a stream — usually the track or the tab title.
    pub media: String,
    /// The icon the application asked to be drawn with, if any.
    pub icon: String,
    pub kind: NodeKind,
    /// 0–100+, on the same curve `wpctl` and every graphical mixer show.
    pub level: i32,
    /// Whether this node is silenced, by *either* of the two mutes PipeWire keeps.
    ///
    /// The node carries one; the card its node sits on carries another, per route, and they are independent — a laptop's mic-mute key and several mixers set the route's while leaving the node's alone. Reading only the node meant the shell drew a live microphone for one that was off. [`Mirror::snapshot`] folds the route's in before anything sees this, so a consumer asks "is it silenced" and never has to know there were two answers.
    pub muted: bool,
}

impl Node {
    /// What to call this node on screen: the application's name for a stream, the device description for a device, and never an empty string — a row with no label is a row a user cannot choose between.
    pub fn label(&self) -> String {
        for candidate in [&self.app, &self.description, &self.name] {
            let candidate = candidate.trim();
            if !candidate.is_empty() {
                return candidate.to_string();
            }
        }
        format!("#{}", self.id)
    }
}

/// One reading of the graph: everything adjustable, plus which device is the default.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Graph {
    pub nodes: Vec<Node>,
    /// `node.name` of the default sink, from PipeWire's `default` metadata.
    pub default_sink: String,
    pub default_source: String,
}

impl Graph {
    pub fn of_kind(&self, kind: NodeKind) -> impl Iterator<Item = &Node> {
        self.nodes.iter().filter(move |node| node.kind == kind)
    }

    pub fn sinks(&self) -> impl Iterator<Item = &Node> {
        self.of_kind(NodeKind::Sink)
    }

    pub fn sources(&self) -> impl Iterator<Item = &Node> {
        self.of_kind(NodeKind::Source)
    }

    /// The applications currently playing, loudest-lived first in graph order.
    pub fn playback_streams(&self) -> impl Iterator<Item = &Node> {
        self.of_kind(NodeKind::OutputStream)
    }

    pub fn default_sink(&self) -> Option<&Node> {
        self.sinks().find(|node| node.name == self.default_sink)
    }

    pub fn default_source(&self) -> Option<&Node> {
        self.sources().find(|node| node.name == self.default_source)
    }

    pub fn node(&self, id: u32) -> Option<&Node> {
        self.nodes.iter().find(|node| node.id == id)
    }
}

/// A reader of the audio graph.
type GraphHandler = Box<dyn FnMut(&Graph) + Send>;

static HANDLERS: Mutex<Vec<GraphHandler>> = Mutex::new(Vec::new());
static MONITOR_THREAD: OnceLock<()> = OnceLock::new();
/// The last batch the monitor produced, replayed to whoever registers next. See [`on_graph`].
static LAST: Mutex<Option<Graph>> = Mutex::new(None);

/// Registers `handler` on the audio graph, attaching to PipeWire on first use.
///
/// The same shape the Hyprland event stream uses, and for the same reason: `pw-dump --monitor` is one subprocess and one parse, and every derived reading — the default sink's level, the microphone, a device list, a per-application stream — comes off the same batches. A monitor per consumer would be a subprocess per consumer on top of the shared-source design that already rules out one per bar.
pub fn on_graph(mut handler: GraphHandler) {
    // **A late registration is handed the graph as it stands.** `pw-dump --monitor` speaks only when something changes, so a handler attached after the monitor was already running heard nothing at all until the user happened to move a slider somewhere else — the default sink's level, the microphone, every reading derived from this one stream, silently absent on a machine whose audio was simply sitting still. It is the contract `Service::subscribe` already keeps for its subscribers, and it was the one thing missing here.
    if let Some(graph) = LAST.lock().unwrap().clone() {
        handler(&graph);
    }
    HANDLERS.lock().unwrap().push(handler);
    MONITOR_THREAD.get_or_init(|| {
        let _ = std::thread::Builder::new()
            .name("hogar-shell-pipewire".to_string())
            .spawn(run);
    });
}

static GRAPH: Service<Graph> = Service::new("hogar-shell-pipewire-graph", run_graph);

/// Mirrors the shared stream into a broadcast, so a surface can `watch` the graph like any other service. The producer registers and returns; the handler owns the `Arc`, so no thread parks here.
fn run_graph(service: &Arc<Broadcast<Graph>>) {
    let published = Arc::clone(service);
    on_graph(Box::new(move |graph| published.publish(graph.clone())));
}

pub fn subscribe(tx: EventSender<Graph>) {
    GRAPH.subscribe(tx);
}

/// The last published graph, without touching PipeWire — what a click handler acts on.
pub fn current() -> Option<Graph> {
    GRAPH.current()
}

/// Publishes a graph the shell itself just caused, so a chip moves on the same frame instead of waiting for the monitor to report back. The reading that follows reconciles whatever PipeWire actually accepted.
pub fn publish(graph: Graph) {
    GRAPH.publish(graph);
}

fn run() {
    let mut attached = false;
    loop {
        match monitor() {
            Ok(()) => {
                attached = true;
                tracing::warn!("pw-dump exited; re-attaching to the audio graph");
            }
            // Nothing to attach to and nothing that will change that: a machine without PipeWire is not one that grows it while the shell runs, so retrying forever would just fork a doomed process a thousand times a day. Retiring here is what a shell with no audio at all already does.
            Err(e) if !attached => {
                tracing::info!("no PipeWire audio graph ({e}); the audio modules will stay empty");
                return;
            }
            Err(e) => tracing::warn!("cannot re-attach to PipeWire ({e}); retrying"),
        }
        std::thread::sleep(REATTACH);
    }
}

/// Runs one `pw-dump --monitor` to completion, publishing a snapshot per batch that changed something.
///
/// `--raw` is what makes this streamable: it prints one JSON array per line — the whole graph first, then a batch per change — so a line is a complete message and no bracket counting is needed.
fn monitor() -> std::io::Result<()> {
    let mut child = deps::command(Dep::PwDump)
        .ok_or_else(|| std::io::Error::other("pw-dump has no row"))?
        .args(["--monitor", "--raw", "--no-colors"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| std::io::Error::other("pw-dump gave no stdout"))?;

    let mut mirror = Mirror::default();
    let mut published = Graph::default();
    for line in BufReader::new(stdout).lines() {
        let line = line?;
        if !mirror.apply(&line) {
            continue;
        }
        let graph = mirror.snapshot();
        if graph != published {
            published = graph.clone();
            // Kept before the handlers run, so one registering from another thread mid-batch replays this rather than the batch before it.
            *LAST.lock().unwrap() = Some(graph.clone());
            for handler in HANDLERS.lock().unwrap().iter_mut() {
                handler(&graph);
            }
        }
    }
    let _ = child.wait();
    Ok(())
}

/// The graph as the shell currently believes it to be.
///
/// Kept as parsed nodes rather than raw JSON: a batch carries each changed object's *complete* `info`, so an update is a replacement and there is nothing to merge — but it also carries objects of every type, and holding onto the ports, links and clients would be most of the memory for none of the answers.
#[derive(Default)]
struct Mirror {
    nodes: HashMap<u32, Node>,
    /// Route mutes, keyed by `(device.id, card.profile.device)` — the card's own mute, which is what a laptop's mic-mute key sets and about which the node's own `mute` says nothing.
    route_mutes: HashMap<(u32, u32), bool>,
    /// Which route each node sits on, so [`snapshot`](Self::snapshot) can join the two. Kept beside the nodes rather than on `Node`, because which card route a node belongs to is how this mirror answers "is it muted" and not something a consumer of a reading has any use for.
    node_routes: HashMap<u32, (u32, u32)>,
    default_sink: String,
    default_source: String,
}

impl Mirror {
    /// Applies one batch. Returns whether anything the shell draws could have changed, so a graph full of link and port churn — which is most of what a busy machine emits — costs a parse and nothing more.
    fn apply(&mut self, line: &str) -> bool {
        let Ok(Value::Array(batch)) = serde_json::from_str::<Value>(line) else {
            return false;
        };
        let mut touched = false;
        for object in batch {
            let Some(id) = object.get("id").and_then(Value::as_u64) else {
                continue;
            };
            let id = id as u32;
            // A removed object arrives as its id with a null `info` — an application closing, a device unplugged. Nothing else distinguishes it from an update.
            if object.get("info").is_some_and(Value::is_null) {
                self.node_routes.remove(&id);
                touched |= self.nodes.remove(&id).is_some();
                // A card going away takes its routes with it, or a re-plugged device inherits the mute state of the one before it.
                let had_routes = self.route_mutes.keys().any(|(device, _)| *device == id);
                self.route_mutes.retain(|(device, _), _| *device != id);
                touched |= had_routes;
                continue;
            }
            match object.get("type").and_then(Value::as_str) {
                Some("PipeWire:Interface:Node") => touched |= self.apply_node(id, &object),
                Some("PipeWire:Interface:Device") => touched |= self.apply_device(id, &object),
                Some("PipeWire:Interface:Metadata") => touched |= self.apply_metadata(&object),
                _ => {}
            }
        }
        touched
    }

    fn apply_node(&mut self, id: u32, object: &Value) -> bool {
        match parse_node(id, object) {
            Some(node) => {
                let route = route_key(object);
                let changed = self.nodes.get(&id) != Some(&node)
                    || self.node_routes.get(&id) != route.as_ref();
                self.nodes.insert(id, node);
                match route {
                    Some(key) => self.node_routes.insert(id, key),
                    None => self.node_routes.remove(&id),
                };
                changed
            }
            // A node the shell does not adjust, or one whose params have not arrived yet. Dropping a previously-known node here matters: a stream keeps its id while its media class is rewritten.
            None => {
                self.node_routes.remove(&id);
                self.nodes.remove(&id).is_some()
            }
        }
    }

    /// Records the card's per-route mutes. A `Device` object carries one `Route` per jack, each naming the `device` index its nodes report as `card.profile.device` — so the pair `(card id, route device)` is what joins a route to the node it silences.
    ///
    /// A device whose `Route` params have not arrived yet leaves the map alone rather than clearing it: an update about something else on the card is not a statement that nothing is muted.
    fn apply_device(&mut self, id: u32, object: &Value) -> bool {
        let Some(routes) = object
            .get("info")
            .and_then(|info| info.get("params"))
            .and_then(|params| params.get("Route"))
            .and_then(Value::as_array)
        else {
            return false;
        };
        let mut changed = false;
        for route in routes {
            let Some(index) = number(route, "device") else {
                continue;
            };
            let muted = route
                .get("props")
                .and_then(|props| props.get("mute"))
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if self.route_mutes.insert((id, index), muted) != Some(muted) {
                changed = true;
            }
        }
        changed
    }

    /// PipeWire keeps the default device in a metadata object rather than on the node, because "default" is a property of the session and not of the hardware. It names the node, not its id.
    fn apply_metadata(&mut self, object: &Value) -> bool {
        if object
            .get("props")
            .and_then(|props| props.get("metadata.name"))
            .and_then(Value::as_str)
            != Some("default")
        {
            return false;
        }
        let Some(entries) = object.get("metadata").and_then(Value::as_array) else {
            return false;
        };
        let mut changed = false;
        for entry in entries {
            let key = entry.get("key").and_then(Value::as_str).unwrap_or_default();
            let name = entry
                .get("value")
                .and_then(|value| value.get("name"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let slot = match key {
                "default.audio.sink" => &mut self.default_sink,
                "default.audio.source" => &mut self.default_source,
                _ => continue,
            };
            if *slot != name {
                *slot = name;
                changed = true;
            }
        }
        changed
    }

    fn snapshot(&self) -> Graph {
        // The route's mute is folded in here, once, rather than at each of the places that ask whether something is silenced. Either mute silences the node, so the effective answer is their `or`.
        let mut nodes: Vec<Node> = self
            .nodes
            .values()
            .cloned()
            .map(|mut node| {
                if let Some(key) = self.node_routes.get(&node.id)
                    && self.route_mutes.get(key) == Some(&true)
                {
                    node.muted = true;
                }
                node
            })
            .collect();
        // Stable order, so a redraw never reshuffles a device list under the pointer. Ids ascend with creation, which puts the machine's own devices above the applications that came later.
        nodes.sort_by_key(|node| node.id);
        Graph {
            nodes,
            default_sink: self.default_sink.clone(),
            default_source: self.default_source.clone(),
        }
    }
}

fn parse_node(id: u32, object: &Value) -> Option<Node> {
    let info = object.get("info")?;
    let props = info.get("props")?;
    let text = |key: &str| {
        props
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let kind = NodeKind::parse(props.get("media.class").and_then(Value::as_str)?)?;
    let volume = volume_props(info);

    let description = [
        text("node.description"),
        text("node.nick"),
        text("node.name"),
    ]
    .into_iter()
    .find(|candidate| !candidate.trim().is_empty())
    .unwrap_or_default();

    Some(Node {
        id,
        name: text("node.name"),
        description,
        app: text("application.name"),
        media: text("media.name"),
        icon: text("application.icon-name"),
        kind,
        level: volume.map(level_of).unwrap_or(0),
        muted: volume
            .and_then(|v| v.get("mute"))
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

/// The `(device.id, card.profile.device)` pair naming this node's route on its card, or `None` for a stream — which belongs to an application rather than to hardware and has no route to be muted by.
fn route_key(object: &Value) -> Option<(u32, u32)> {
    let props = object.get("info")?.get("props")?;
    number(props, "device.id").zip(number(props, "card.profile.device"))
}

/// A `props` value that is a number, whatever JSON type it arrived as — PipeWire writes these as bare numbers in `pw-dump` but as strings through some of its other paths, and a route join that silently misses is a mute that silently does not apply.
fn number(props: &Value, key: &str) -> Option<u32> {
    let value = props.get(key)?;
    value
        .as_u64()
        .or_else(|| value.as_str()?.trim().parse::<u64>().ok())
        .and_then(|n| u32::try_from(n).ok())
}

/// The volume entry of a node's `Props`, which is **not always the first**: `params.Props` carries a second entry holding `cardName`/`device`/`deviceName` and no volume at all. Taking `.first()` worked only because PipeWire happens to emit them in this order.
fn volume_props(info: &Value) -> Option<&Value> {
    info.get("params")?
        .get("Props")?
        .as_array()?
        .iter()
        .find(|entry| entry.get("mute").is_some() || entry.get("channelVolumes").is_some())
}

/// The node's level on the curve a user has seen everywhere else.
///
/// PipeWire stores a linear amplitude, and every mixer on the machine — `wpctl`, pavucontrol, anything built on PulseAudio's API — shows its cube root: 40% on screen is 0.064 in the graph. Reading `channelVolumes` as a percentage would have the shell report 6% for a sink every other tool calls 40%.
///
/// The loudest channel rather than the first: a node with one channel turned down is not at that channel's level, and a balance a user set elsewhere should not read as the volume having dropped.
fn level_of(props: &Value) -> i32 {
    let loudest = props
        .get("channelVolumes")
        .and_then(Value::as_array)
        .map(|channels| {
            channels
                .iter()
                .filter_map(Value::as_f64)
                .fold(0.0, f64::max)
        })
        .or_else(|| props.get("volume").and_then(Value::as_f64))
        .unwrap_or(0.0);
    (loudest.max(0.0).cbrt() * 100.0).round() as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn batch(json: &str) -> String {
        json.to_string()
    }

    const SINK: &str = r#"[{"id":54,"type":"PipeWire:Interface:Node","info":{
        "props":{"media.class":"Audio/Sink","node.name":"alsa_output.analog-stereo",
                 "node.description":"Built-in Audio"},
        "params":{"Props":[{"channelVolumes":[0.064012,0.064012],"mute":false}]}}}]"#;

    /// The shapes below are copied from a live `pw-dump` (PipeWire 1.6.8), not invented: a source node that names its card and route, and the card carrying the mute that node knows nothing about.
    const MIC: &str = r#"[{"id":55,"type":"PipeWire:Interface:Node","info":{
        "props":{"media.class":"Audio/Source","node.name":"alsa_input.analog-stereo",
                 "node.description":"Microphone","device.id":47,"card.profile.device":0},
        "params":{"Props":[
            {"channelVolumes":[0.020685,0.020685],"mute":false,"softMute":false},
            {"cardName":"acp63","device":0,"deviceName":"hw:1,0"}]}}}]"#;

    const CARD_MIC_MUTED: &str = r#"[{"id":47,"type":"PipeWire:Interface:Device","info":{
        "props":{"device.name":"alsa_card.pci-0000_04_00.6"},
        "params":{"Route":[
            {"index":0,"device":0,"direction":"Input","name":"analog-input-mic",
             "props":{"mute":true,"channelVolumes":[0.020685,0.020685]}},
            {"index":1,"device":3,"direction":"Output","name":"analog-output-speaker",
             "props":{"mute":false}}]}}}]"#;

    /// The bug this exists for: a laptop's mic-mute key sets the *card route's* mute and leaves the node's alone, so a shell reading only the node drew a live microphone for one that was switched off.
    #[test]
    fn a_route_mute_silences_the_node_sitting_on_it() {
        let mut mirror = Mirror::default();
        mirror.apply(&batch(MIC));
        let node = mirror.snapshot().sources().next().cloned().unwrap();
        assert!(
            !node.muted,
            "the node's own mute is off, and that is all it says"
        );

        assert!(
            mirror.apply(&batch(CARD_MIC_MUTED)),
            "a route mute is a change the shell draws"
        );
        let node = mirror.snapshot().sources().next().cloned().unwrap();
        assert!(
            node.muted,
            "either mute silences it — the node still reads false and the card says otherwise"
        );
    }

    /// The output route on the same card is muted independently; joining on the card alone rather than on `(card, route device)` would have one jack's mute silence the other's.
    #[test]
    fn a_route_only_mutes_the_device_index_it_names() {
        let mut mirror = Mirror::default();
        mirror.apply(&batch(SINK));
        mirror.apply(&batch(CARD_MIC_MUTED));
        let sink = mirror.snapshot().sinks().next().cloned().unwrap();
        assert!(
            !sink.muted,
            "the sink names no route here, so a muted input route cannot reach it"
        );
    }

    /// `params.Props` carries a second entry with no volume in it. Reading `.first()` worked only because PipeWire happens to emit the volume entry first; nothing promises that order.
    #[test]
    fn the_volume_entry_is_found_wherever_it_sits_in_props() {
        let reordered = batch(
            r#"[{"id":55,"type":"PipeWire:Interface:Node","info":{
            "props":{"media.class":"Audio/Source","node.name":"mic"},
            "params":{"Props":[
                {"cardName":"acp63","device":0,"deviceName":"hw:1,0"},
                {"channelVolumes":[1.0],"mute":true}]}}}]"#,
        );
        let mut mirror = Mirror::default();
        mirror.apply(&reordered);
        let node = mirror.snapshot().sources().next().cloned().unwrap();
        assert!(node.muted, "the mute is real wherever the entry sits");
        assert_eq!(node.level, 100, "and so is the level beside it");
    }

    #[test]
    fn a_card_going_away_takes_its_route_mutes_with_it() {
        let mut mirror = Mirror::default();
        mirror.apply(&batch(MIC));
        mirror.apply(&batch(CARD_MIC_MUTED));
        assert!(mirror.snapshot().sources().next().unwrap().muted);

        mirror.apply(&batch(r#"[{"id":47,"info":null}]"#));
        assert!(
            !mirror.snapshot().sources().next().unwrap().muted,
            "a re-plugged card must not inherit the mute of the one before it"
        );
    }

    #[test]
    fn a_level_reads_on_the_curve_every_other_mixer_shows() {
        // The regression this exists for: 0.064 is what PipeWire stores for the 40% `wpctl` reports.
        let props: Value =
            serde_json::from_str(r#"{"channelVolumes":[0.064012,0.064012]}"#).unwrap();
        assert_eq!(level_of(&props), 40);
        let silent: Value = serde_json::from_str(r#"{"channelVolumes":[0.0]}"#).unwrap();
        assert_eq!(level_of(&silent), 0);
        let full: Value = serde_json::from_str(r#"{"channelVolumes":[1.0]}"#).unwrap();
        assert_eq!(level_of(&full), 100);
        // A boosted sink reads past 100 rather than clamping here; callers clamp for display.
        let boosted: Value = serde_json::from_str(r#"{"channelVolumes":[2.744]}"#).unwrap();
        assert_eq!(level_of(&boosted), 140);
    }

    #[test]
    fn the_loudest_channel_is_the_level() {
        let unbalanced: Value = serde_json::from_str(r#"{"channelVolumes":[0.064,0.0]}"#).unwrap();
        assert_eq!(
            level_of(&unbalanced),
            40,
            "a channel turned down is a balance, not a quieter sink"
        );
    }

    #[test]
    fn a_node_parses_into_something_adjustable() {
        let mut mirror = Mirror::default();
        assert!(mirror.apply(&batch(SINK)), "a new sink is a change");
        let graph = mirror.snapshot();
        let sink = graph.sinks().next().expect("the sink is listed");
        assert_eq!(sink.id, 54);
        assert_eq!(sink.level, 40);
        assert!(!sink.muted);
        assert_eq!(sink.label(), "Built-in Audio");
        assert!(
            !mirror.apply(&batch(SINK)),
            "the same reading twice is not a change, so nothing republishes"
        );
    }

    #[test]
    fn a_null_info_removes_the_node_it_names() {
        let mut mirror = Mirror::default();
        mirror.apply(&batch(SINK));
        assert!(
            mirror.apply(r#"[{"id":54,"info":null}]"#),
            "removal is a change"
        );
        assert!(mirror.snapshot().nodes.is_empty());
        assert!(
            !mirror.apply(r#"[{"id":54,"info":null}]"#),
            "removing what is already gone changes nothing"
        );
    }

    #[test]
    fn the_default_device_comes_from_the_metadata_not_the_node() {
        let mut mirror = Mirror::default();
        mirror.apply(&batch(SINK));
        assert!(mirror.apply(
            r#"[{"id":36,"type":"PipeWire:Interface:Metadata","props":{"metadata.name":"default"},
                 "metadata":[{"key":"default.audio.sink","value":{"name":"alsa_output.analog-stereo"}}]}]"#
        ));
        let graph = mirror.snapshot();
        assert_eq!(
            graph.default_sink().map(|n| n.id),
            Some(54),
            "the metadata names the node, and the graph resolves it"
        );
        // Another session's metadata must not be read as this one's defaults.
        assert!(!mirror.apply(
            r#"[{"id":37,"type":"PipeWire:Interface:Metadata","props":{"metadata.name":"route-settings"},
                 "metadata":[{"key":"default.audio.sink","value":{"name":"nonsense"}}]}]"#
        ));
        assert_eq!(mirror.snapshot().default_sink, "alsa_output.analog-stereo");
    }

    #[test]
    fn everything_that_is_not_a_mixer_control_is_dropped() {
        let mut mirror = Mirror::default();
        assert!(
            !mirror.apply(
                r#"[{"id":48,"type":"PipeWire:Interface:Node","info":{"props":{"media.class":"Midi/Bridge","node.name":"Midi-Bridge"}}},
                    {"id":9,"type":"PipeWire:Interface:Port","info":{"props":{}}},
                    {"id":31,"type":"PipeWire:Interface:Node","info":{"props":{"node.name":"Dummy-Driver"}}}]"#
            ),
            "ports, drivers and the MIDI bridge are not things a user adjusts"
        );
        assert!(mirror.snapshot().nodes.is_empty());
    }

    #[test]
    fn a_stream_is_labelled_by_its_application() {
        let mut mirror = Mirror::default();
        mirror.apply(
            r#"[{"id":73,"type":"PipeWire:Interface:Node","info":{
                "props":{"media.class":"Stream/Output/Audio","node.name":"Firefox",
                         "application.name":"Firefox","media.name":"A video"},
                "params":{"Props":[{"channelVolumes":[1.0],"mute":true}]}}}]"#,
        );
        let graph = mirror.snapshot();
        let stream = graph
            .playback_streams()
            .next()
            .expect("the stream is listed");
        assert_eq!(stream.label(), "Firefox");
        assert_eq!(stream.media, "A video");
        assert!(stream.muted);
        assert!(stream.kind.is_stream());
    }

    /// Parses this machine's actual graph. Gated because it shells out — the suite must stay hermetic — but it is the only check that the shapes above still match what `pw-dump` emits, which is a contract PipeWire owns and can change under us.
    ///
    /// `HOGAR_SHELL_PIPEWIRE_LIVE=1 cargo test -p hogar-shell --lib live_graph -- --nocapture`
    #[test]
    fn live_graph_parses_on_this_machine() {
        if std::env::var("HOGAR_SHELL_PIPEWIRE_LIVE").is_err() {
            eprintln!("set HOGAR_SHELL_PIPEWIRE_LIVE to parse the real graph; skipping");
            return;
        }
        let out = deps::command(Dep::PwDump)
            .expect("pw-dump is a program")
            .args(["--raw", "--no-colors"])
            .output()
            .expect("pw-dump runs");
        let dump = String::from_utf8_lossy(&out.stdout);
        let mut mirror = Mirror::default();
        assert!(mirror.apply(dump.trim()), "the initial dump is a change");

        let graph = mirror.snapshot();
        for node in &graph.nodes {
            eprintln!(
                "{:>4}  {:<14} {:>4}%{}  {}",
                node.id,
                format!("{:?}", node.kind),
                node.level,
                if node.muted { " muted" } else { "      " },
                node.label()
            );
        }
        eprintln!(
            "default sink: {:?}  source: {:?}",
            graph.default_sink().map(Node::label),
            graph.default_source().map(Node::label)
        );
        assert!(
            graph.sinks().next().is_some(),
            "a machine running PipeWire has at least one sink"
        );
        assert!(
            graph.default_sink().is_some(),
            "the default metadata resolves to a node that is in the graph"
        );
    }

    #[test]
    fn a_malformed_line_is_ignored_rather_than_ending_the_monitor() {
        let mut mirror = Mirror::default();
        mirror.apply(&batch(SINK));
        assert!(!mirror.apply("not json at all"));
        assert!(!mirror.apply("{}"), "a bare object is not a batch");
        assert_eq!(mirror.snapshot().nodes.len(), 1, "the graph survives it");
    }

    /// **A handler that arrives after the monitor is given the graph as it stands.**
    ///
    /// `pw-dump --monitor` speaks only when something changes, so a reading derived from it — the default sink's level, the microphone — was silently absent on a machine whose audio was sitting still: the handler had been registered and simply never called. `volume get` answered "no audio sink available" about a machine with one, and went on answering it until someone happened to move a slider elsewhere.
    #[test]
    fn a_handler_registered_after_the_first_batch_is_replayed_the_last_one() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let mut graph = Graph::default();
        graph.nodes.push(Node {
            id: 7,
            name: "sink".to_string(),
            description: String::new(),
            app: String::new(),
            media: String::new(),
            icon: String::new(),
            kind: NodeKind::Sink,
            level: 42,
            muted: false,
        });
        *LAST.lock().unwrap() = Some(graph);

        static SEEN: AtomicUsize = AtomicUsize::new(0);
        on_graph(Box::new(|graph| {
            SEEN.store(graph.nodes.len(), Ordering::Relaxed);
        }));
        assert_eq!(
            SEEN.load(Ordering::Relaxed),
            1,
            "the handler was handed the graph on registration rather than waiting for a change"
        );
        *LAST.lock().unwrap() = None;
    }
}
