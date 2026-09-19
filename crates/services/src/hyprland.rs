use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::Deserialize;

use platform_wayland::{EventSender, Interest};

use util::broadcast::{Broadcast, Service};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub workspaces: Vec<Workspace>,
    pub active: i32,
    /// The monitor holding focus, so a per-monitor bar can show only its own workspaces.
    pub focused_monitor: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Workspace {
    pub id: i32,
    /// Hyprland's own name. Numbered workspaces name themselves after their id; a special workspace is `special:<name>`, which is the only place its name is recoverable.
    pub name: String,
    pub windows: u32,
    pub monitor: String,
    /// The window classes on this workspace, in Hyprland's order — what a pill draws app icons from.
    pub clients: Vec<String>,
    /// What the compositor calls this workspace over `ext-workspace-v1`, when it listed it. `None` for a scratchpad, which the protocol does not list, and for every workspace on a compositor without it.
    pub handle: Option<platform_wayland::WorkspaceId>,
}

impl Workspace {
    /// Hyprland gives special workspaces (scratchpads) negative ids and a `special:` name.
    pub fn is_special(&self) -> bool {
        self.id < 0
    }

    /// The bare name of a special workspace (`special:magic` → `magic`), for matching config icons.
    pub fn special_name(&self) -> &str {
        self.name.strip_prefix("special:").unwrap_or(&self.name)
    }

    pub fn is_occupied(&self) -> bool {
        self.windows > 0
    }
}

/// The focused window, or the empty value when the compositor reports none (an empty workspace, a layer surface holding focus). Every field is what Hyprland calls it, so a config regex written against `hyprctl clients` matches.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ActiveWindow {
    pub title: String,
    /// The application id. Hyprland's `class`, and `app_id` in every protocol that reports one.
    pub class: String,
    /// Hyprland's `0x…` handle, and empty on any compositor that is not Hyprland: no Wayland protocol exposes a window's address. Anything reading geometry, a workspace or a process id needs this and therefore needs Hyprland.
    pub address: String,
    /// What the compositor calls this window over `wlr-foreign-toplevel-management`, when it reported one.
    pub handle: Option<platform_wayland::ManagedToplevelId>,
}

impl ActiveWindow {
    /// Whether the compositor reports no focused window at all.
    ///
    /// Both identities, not just the address: off Hyprland every window has an empty address, so asking about that alone would report every window as no window.
    pub fn is_empty(&self) -> bool {
        self.handle.is_none() && self.address.is_empty()
    }
}

/// The active keyboard layout of the main keyboard, as Hyprland names it (`"English (US)"`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KeyboardLayout {
    pub name: String,
    /// The device the layout belongs to, needed to switch it.
    pub device: String,
}

#[derive(Deserialize)]
struct WorkspaceJson {
    id: i32,
    #[serde(default)]
    name: String,
    windows: u32,
    #[serde(default)]
    monitor: String,
}

/// One window the compositor is managing, as `j/clients` reports it. Every field keeps Hyprland's own meaning so a rule written against `hyprctl clients` reads the same here.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Client {
    /// `0x…`, unique and stable for the window's life — the handle every dispatcher takes.
    pub address: String,
    pub class: String,
    pub title: String,
    pub pid: i32,
    pub workspace: i32,
    pub workspace_name: String,
    /// Hyprland's monitor *index*, not its name: `j/clients` reports the id and only `j/monitors` maps it back.
    pub monitor: i32,
    pub at: (i32, i32),
    pub size: (i32, i32),
    pub floating: bool,
    /// Maximized or fullscreen; Hyprland distinguishes the two, a shell asking "is something covering the screen" does not.
    pub fullscreen: bool,
    pub pinned: bool,
    /// An unmapped window is one the compositor is not drawing — a tray-minimized application, mostly. Kept rather than filtered so a window list can show it as hidden instead of losing it.
    pub mapped: bool,
    pub xwayland: bool,
}

#[derive(Deserialize)]
struct ClientJson {
    #[serde(default)]
    address: String,
    #[serde(default)]
    class: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    pid: i32,
    workspace: ClientWorkspaceJson,
    #[serde(default)]
    monitor: i32,
    #[serde(default)]
    at: (i32, i32),
    #[serde(default)]
    size: (i32, i32),
    #[serde(default)]
    floating: bool,
    #[serde(default)]
    fullscreen: Fullscreen,
    #[serde(default)]
    pinned: bool,
    #[serde(default)]
    mapped: bool,
    #[serde(default)]
    xwayland: bool,
}

/// Hyprland reported `fullscreen` as a bool until 0.42 and as a mode integer (0 none, 1 maximized, 2 fullscreen) after it. Accepting both keeps the client list working across the versions a user might be on, instead of failing the whole parse on the field's type.
#[derive(Deserialize, Default, Clone, Copy)]
#[serde(untagged)]
enum Fullscreen {
    Flag(bool),
    Mode(i64),
    #[default]
    Absent,
}

impl Fullscreen {
    fn is_set(self) -> bool {
        match self {
            Self::Flag(on) => on,
            Self::Mode(mode) => mode != 0,
            Self::Absent => false,
        }
    }
}

#[derive(Deserialize)]
struct ClientWorkspaceJson {
    id: i32,
    #[serde(default)]
    name: String,
}

#[derive(Deserialize)]
struct ActiveJson {
    id: i32,
}

/// One output, as `j/monitors` describes it. The connector `name` (`DP-1`) is what per-monitor config, the layer-shell surfaces and the settings app all key on; `make`/`model` are what a human recognises it by.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Screen {
    pub name: String,
    pub description: String,
    pub make: String,
    pub model: String,
    pub serial: String,
    pub width: u32,
    pub height: u32,
    pub refresh: f32,
    /// Position in the compositor's layout space, which is what orders the list left-to-right.
    pub at: (i32, i32),
    pub scale: f32,
    pub transform: i32,
    pub focused: bool,
    pub disabled: bool,
    pub vrr: bool,
    pub dpms: bool,
    pub active_workspace: i32,
}

#[derive(Deserialize)]
struct MonitorJson {
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    make: String,
    #[serde(default)]
    model: String,
    #[serde(default)]
    serial: String,
    #[serde(default)]
    width: u32,
    #[serde(default)]
    height: u32,
    #[serde(default, rename = "refreshRate")]
    refresh_rate: f32,
    #[serde(default)]
    x: i32,
    #[serde(default)]
    y: i32,
    #[serde(default = "one")]
    scale: f32,
    #[serde(default)]
    transform: i32,
    focused: bool,
    #[serde(default)]
    disabled: bool,
    #[serde(default)]
    vrr: bool,
    #[serde(default = "yes", rename = "dpmsStatus")]
    dpms_status: bool,
    #[serde(default, rename = "activeWorkspace")]
    active_workspace: Option<ActiveJson>,
}

fn one() -> f32 {
    1.0
}

fn yes() -> bool {
    true
}

#[derive(Deserialize)]
struct ActiveWindowJson {
    #[serde(default)]
    title: String,
    #[serde(default)]
    class: String,
    #[serde(default)]
    address: String,
}

#[derive(Deserialize)]
struct DevicesJson {
    keyboards: Vec<KeyboardJson>,
}

#[derive(Deserialize)]
struct KeyboardJson {
    name: String,
    main: bool,
    active_keymap: String,
}

/// The per-instance Hyprland socket directory, or `None` when not running under Hyprland — or when this process is sealed from the user's session (see [`util::live`]), which every IPC request and event stream here goes through. Hyprland ≥ 0.40 puts it under `$XDG_RUNTIME_DIR/hypr/$SIG`; older versions used `/tmp/hypr/$SIG`.
pub fn socket_dir() -> Option<PathBuf> {
    if !util::live::installed() {
        return None;
    }
    let sig = std::env::var("HYPRLAND_INSTANCE_SIGNATURE").ok()?;
    if let Ok(runtime) = std::env::var("XDG_RUNTIME_DIR") {
        let path = PathBuf::from(runtime).join("hypr").join(&sig);
        if path.exists() {
            return Some(path);
        }
    }
    let legacy = PathBuf::from("/tmp/hypr").join(&sig);
    legacy.exists().then_some(legacy)
}

fn request(dir: &Path, command: &str) -> std::io::Result<String> {
    let mut stream = UnixStream::connect(dir.join(".socket.sock"))?;
    stream.write_all(command.as_bytes())?;
    let mut response = String::new();
    stream.read_to_string(&mut response)?;
    Ok(response)
}

/// The name of the currently focused monitor, or `None` if Hyprland can't be queried.
pub fn focused_monitor(dir: &Path) -> Option<String> {
    let raw = request(dir, "j/monitors").ok()?;
    serde_json::from_str::<Vec<MonitorJson>>(&raw)
        .ok()?
        .into_iter()
        .find(|m| m.focused)
        .map(|m| m.name)
}

fn query_monitors(dir: &Path, command: &str) -> Option<Vec<MonitorJson>> {
    serde_json::from_str(&request(dir, command).ok()?).ok()
}

/// Every output the compositor knows about, ordered left-to-right then top-to-bottom by their position in the layout — the order a user reads their desk in, and the one a per-monitor settings list wants.
pub fn screens(dir: &Path) -> Vec<Screen> {
    // `all` includes outputs that are connected but switched off, which is the difference between a settings list that can re-enable a monitor and one that cannot see it. Not every Hyprland accepts the argument, so a refusal falls back to the plain query rather than reporting no screens at all.
    let Some(parsed) =
        query_monitors(dir, "j/monitors all").or_else(|| query_monitors(dir, "j/monitors"))
    else {
        return Vec::new();
    };
    let mut screens: Vec<Screen> = parsed
        .into_iter()
        .map(|m| Screen {
            name: m.name,
            description: m.description,
            make: m.make,
            model: m.model,
            serial: m.serial,
            width: m.width,
            height: m.height,
            refresh: m.refresh_rate,
            at: (m.x, m.y),
            scale: m.scale,
            transform: m.transform,
            focused: m.focused,
            disabled: m.disabled,
            vrr: m.vrr,
            dpms: m.dpms_status,
            active_workspace: m.active_workspace.map(|w| w.id).unwrap_or_default(),
        })
        .collect();
    screens.sort_by_key(|s| (s.at.1, s.at.0));
    screens
}

/// The monitor name carried by a `focusedmon>>NAME,WORKSPACE` event line, if that's what it is.
pub fn monitor_from_focus_event(line: &str) -> Option<String> {
    line.strip_prefix("focusedmon>>")
        .and_then(|rest| rest.split(',').next())
        .map(str::to_string)
}

/// Runs a Hyprland dispatcher. Hyprland ≥ 0.55 evaluates socket commands as Lua, so `dispatch workspace N` no longer parses; `call` is the Lua expression the socket wraps as `hl.dispatch(<call>)`, e.g. `hl.dsp.focus({ workspace = 3 })`. Errors are reported rather than returned: every caller is a UI handler with nothing useful to do about a refused dispatch.
fn dispatch(dir: &Path, call: &str) {
    let cmd = format!("dispatch {call}");
    match request(dir, &cmd) {
        Ok(resp) if resp.to_ascii_lowercase().contains("error") => {
            tracing::warn!("hogar-shell: `{cmd}` -> {resp:?}")
        }
        Ok(_) => {}
        Err(e) => tracing::warn!("hogar-shell: `{cmd}` failed: {e}"),
    }
}

pub fn focus_workspace(dir: &Path, id: i32) {
    dispatch(dir, &format!("hl.dsp.focus({{ workspace = {id} }})"));
}

/// Focuses the workspace a pill shows, over whichever route the compositor offers.
///
/// The protocol first, and only for a workspace the compositor actually listed. A `[workspaces] shown` bar draws placeholder pills for workspaces that do not exist yet and pressing one is how you get there — there is no handle to activate for a workspace that does not exist, and Hyprland's dispatcher creates it. That is what makes the fallback more than a fallback on a compositor that has both.
pub fn focus_workspace_id(id: i32) {
    let handle = current_workspaces()
        .and_then(|snapshot| snapshot.workspaces.into_iter().find(|w| w.id == id))
        .and_then(|workspace| workspace.handle);
    if let Some(handle) = handle
        && platform_wayland::activate_workspace(handle)
    {
        return;
    }
    if let Some(dir) = socket_dir() {
        focus_workspace(&dir, id);
    }
}

/// The two ways Hyprland 0.56's `dpms` dispatcher might take its state.
///
/// Every other Lua dispatcher names its arguments when given the wrong ones — `hl.dsp.focus({ nonsense = 1 })` answers with "Expected one of: direction, monitor, window, …" — but `hl.dsp.dpms` accepts anything at all, including a function, without complaint. There is nothing to read the shape off, so it is not guessed: [`set_dpms`] tries these and checks whether the compositor's own `dpmsStatus` moved.
fn dpms_calls(state: &str) -> [String; 2] {
    [
        format!("hl.dsp.dpms(\"{state}\")"),
        format!("hl.dsp.dpms({{ state = \"{state}\" }})"),
    ]
}

/// Switches every monitor's output on or off, and reports whether it worked.
///
/// Verified rather than trusted, for the reason above: the call is made and `dpmsStatus` read back, so an idle stage that blanks the screen either does or says it could not. DPMS is idempotent, so a second shape landing after a first one already worked costs nothing.
pub fn set_dpms(dir: &Path, on: bool) -> bool {
    let state = if on { "on" } else { "off" };
    if dpms_is(dir, on) {
        return true;
    }
    for call in dpms_calls(state) {
        dispatch(dir, &call);
        if dpms_is(dir, on) {
            return true;
        }
    }
    tracing::warn!("hogar-shell: no `hl.dsp.dpms` call shape turned the outputs {state}");
    false
}

/// Whether every enabled monitor's output is in the requested state. Read from the compositor rather than remembered, since a `hyprctl` or a keybind can move it behind the shell's back.
fn dpms_is(dir: &Path, on: bool) -> bool {
    let Some(monitors) = query_monitors(dir, "j/monitors") else {
        return false;
    };
    !monitors.is_empty() && monitors.iter().all(|m| m.dpms_status == on)
}

/// Every window the compositor is managing, in Hyprland's own order. One parse feeds both readers of it — the workspace pills, which want the classes grouped by workspace, and the client list itself — so the two can't disagree about what is open.
pub fn clients(dir: &Path) -> Vec<Client> {
    let Ok(raw) = request(dir, "j/clients") else {
        return Vec::new();
    };
    let Ok(parsed) = serde_json::from_str::<Vec<ClientJson>>(&raw) else {
        return Vec::new();
    };
    parsed
        .into_iter()
        .map(|c| Client {
            address: c.address,
            class: c.class,
            title: c.title,
            pid: c.pid,
            workspace: c.workspace.id,
            workspace_name: c.workspace.name,
            monitor: c.monitor,
            at: c.at,
            size: c.size,
            floating: c.floating,
            fullscreen: c.fullscreen.is_set(),
            pinned: c.pinned,
            mapped: c.mapped,
            xwayland: c.xwayland,
        })
        .collect()
}

/// The window classes on each workspace, keyed by workspace id. A window with no class draws no icon, so it is dropped here rather than leaving a gap in the pill.
fn classes_by_workspace(clients: &[Client]) -> HashMap<i32, Vec<String>> {
    let mut by_workspace: HashMap<i32, Vec<String>> = HashMap::new();
    for client in clients {
        if client.class.is_empty() {
            continue;
        }
        by_workspace
            .entry(client.workspace)
            .or_default()
            .push(client.class.clone());
    }
    by_workspace
}

/// Special workspaces are kept rather than filtered out — a scratchpad is something a bar wants to indicate — and sorted after the numbered ones so their negative ids don't put them at the front.
fn query_snapshot(dir: &Path) -> Option<Snapshot> {
    let workspaces_raw = request(dir, "j/workspaces").ok()?;
    let active_raw = request(dir, "j/activeworkspace").ok()?;
    let mut clients = classes_by_workspace(&clients(dir));

    let mut workspaces: Vec<Workspace> =
        serde_json::from_str::<Vec<WorkspaceJson>>(&workspaces_raw)
            .ok()?
            .into_iter()
            .map(|w| Workspace {
                windows: w.windows,
                clients: clients.remove(&w.id).unwrap_or_default(),
                name: w.name,
                monitor: w.monitor,
                id: w.id,
                handle: None,
            })
            .collect();
    workspaces.sort_by_key(|w| (w.is_special(), w.id));

    let active = serde_json::from_str::<ActiveJson>(&active_raw).ok()?.id;
    Some(Snapshot {
        workspaces,
        active,
        focused_monitor: focused_monitor(dir).unwrap_or_default(),
    })
}

fn affects_workspaces(line: &str) -> bool {
    const PREFIXES: &[&str] = &[
        "workspace>>",
        "workspacev2>>",
        "createworkspace",
        "destroyworkspace",
        "focusedmon>>",
        "moveworkspace",
        "openwindow>>",
        "closewindow>>",
        "movewindow>>",
        // A scratchpad being shown or hidden. `ext-workspace-v1` does not list special workspaces at all, so this is the only event that reports one becoming active.
        "activespecial",
    ];
    PREFIXES.iter().any(|prefix| line.starts_with(prefix))
}

/// A reader of the raw Hyprland event stream, and the claim that says whether it still wants to be one.
type EventHandler = Box<dyn FnMut(&str) + Send>;

struct Registration {
    interest: Interest,
    handler: EventHandler,
}

static HANDLERS: Mutex<Vec<Registration>> = Mutex::new(Vec::new());
/// Whether the socket reader is running. Not a `OnceLock`, because it has to be able to say "no" again: the thread gives it back when the last registration goes, and a later producer starts a fresh one.
static EVENT_THREAD: Mutex<bool> = Mutex::new(false);

/// Registers `handler` on the compositor's event stream for as long as `interest` is alive, opening the socket on first use.
///
/// Hyprland's `.socket2.sock` is a single-consumer firehose, and every derived reading — workspaces, the focused window, the keyboard layout — is driven by the same lines. One connection with a list of handlers keeps that at one socket and one read per event no matter how many services read from it, rather than a connection per service on top of the connection-per-bar the shared-source design already rules out.
///
/// The running flag is held across the registration, and taken again by [`retire_event_stream`]: that overlap is what stops a registration landing on a reader already on its way out and never being called.
fn on_events(interest: &Interest, handler: EventHandler) {
    let mut running = EVENT_THREAD.lock().unwrap();
    HANDLERS.lock().unwrap().push(Registration {
        interest: interest.clone(),
        handler,
    });
    if !*running {
        *running = std::thread::Builder::new()
            .name("hogar-shell-hypr-events".to_string())
            .spawn(run_event_stream)
            .is_ok();
    }
}

/// Registers `handler` for as long as anything is listening to `service`.
///
/// A producer that registers a callback and returns has two questions to keep answering — did this line change my reading, and is anyone still there — and only the first is interesting enough to be written at each call site. Asked here in one place so that no registration can answer only that one and go on reading the compositor for nobody.
///
/// **After the handler, never before.** `Broadcast::wanted` releases the producer slot the moment it answers `false`, so asking first would retire a service between the line arriving and the reading it implies.
fn on_events_while_wanted<T: Clone + Send + 'static>(
    service: &Arc<Broadcast<T>>,
    interest: &Interest,
    mut handler: impl FnMut(&str) + Send + 'static,
) {
    let service = Arc::clone(service);
    let owned = interest.clone();
    on_events(
        interest,
        Box::new(move |line| {
            handler(line);
            if !service.wanted() {
                owned.retire();
            }
        }),
    );
}

/// Drops the registrations whose owner has retired, and says whether any are left.
fn anyone_reading() -> bool {
    let mut handlers = HANDLERS.lock().unwrap();
    handlers.retain(|registration| registration.interest.alive());
    !handlers.is_empty()
}

/// Gives up the reader, for a thread about to return. `false` is a registration having landed since the last one went, which has to keep the socket open — it is already in the list and nothing else would call it.
fn retire_event_stream() -> bool {
    let mut running = EVENT_THREAD.lock().unwrap();
    if !HANDLERS.lock().unwrap().is_empty() {
        return false;
    }
    *running = false;
    true
}

/// Gives the reader up whatever is registered, for a stream that has failed or ended under it: every registration is waiting on a socket that is not coming back, and leaving the flag set would stop any later producer from opening a working one.
fn forget_event_stream() {
    let mut running = EVENT_THREAD.lock().unwrap();
    HANDLERS.lock().unwrap().clear();
    *running = false;
}

fn run_event_stream() {
    let Some(dir) = socket_dir() else {
        return forget_event_stream();
    };
    let Ok(stream) = UnixStream::connect(dir.join(".socket2.sock")) else {
        tracing::warn!("cannot open the Hyprland event socket; live updates are off");
        return forget_event_stream();
    };
    for line in BufReader::new(stream).lines() {
        let Ok(line) = line else { break };
        // Pruned before the line is delivered rather than after, so a registration retired since the last one is not called again — `Broadcast::wanted` answering `false` is final, and calling its handler once more would ask a service that has already given up its producer slot.
        {
            let mut handlers = HANDLERS.lock().unwrap();
            handlers.retain(|registration| registration.interest.alive());
            for registration in handlers.iter_mut() {
                (registration.handler)(&line);
            }
        }
        // The bound on teardown, and the same one a polling producer has: the reader is asleep on the socket until the compositor says something, and gives itself up the next time it wakes.
        if !anyone_reading() && retire_event_stream() {
            return;
        }
    }
    forget_event_stream();
}

/// The number a user calls a workspace, recovered from a protocol that has no numeric id.
///
/// `ext-workspace-v1` carries an optional opaque *string* id, which Hyprland does not send at all. What it does send is the name — on Hyprland, the number written out — and one coordinate carrying the same number. Either answers this; a compositor naming its workspaces "web" and "mail" answers it with the coordinate. Failing both, the position in the list, so two workspaces never collide on zero.
fn numbered_id(workspace: &platform_wayland::Workspace, position: usize) -> i32 {
    workspace
        .name
        .parse()
        .ok()
        .or_else(|| workspace.coordinates.first().map(|c| *c as i32))
        .unwrap_or(position as i32 + 1)
}

/// What only the compositor's own IPC can answer about a workspace list.
#[derive(Default)]
struct Facts {
    windows: HashMap<i32, u32>,
    classes: HashMap<i32, Vec<String>>,
    /// The scratchpads, which `ext-workspace-v1` does not list at all.
    specials: Vec<Workspace>,
    focused_monitor: String,
    /// The active workspace when it is a scratchpad — the one case the protocol reports something else, since it goes on reporting the numbered workspace underneath.
    active_special: Option<i32>,
}

fn read_facts(dir: &Path) -> Facts {
    let classes = classes_by_workspace(&clients(dir));
    let mut windows = HashMap::new();
    let mut specials = Vec::new();
    if let Ok(raw) = request(dir, "j/workspaces")
        && let Ok(parsed) = serde_json::from_str::<Vec<WorkspaceJson>>(&raw)
    {
        for workspace in parsed {
            windows.insert(workspace.id, workspace.windows);
            if workspace.id < 0 {
                specials.push(Workspace {
                    clients: classes.get(&workspace.id).cloned().unwrap_or_default(),
                    id: workspace.id,
                    name: workspace.name,
                    windows: workspace.windows,
                    monitor: workspace.monitor,
                    handle: None,
                });
            }
        }
    }
    let active_special = request(dir, "j/activeworkspace")
        .ok()
        .and_then(|raw| serde_json::from_str::<ActiveJson>(&raw).ok())
        .map(|active| active.id)
        .filter(|id| *id < 0);
    Facts {
        windows,
        classes,
        specials,
        focused_monitor: focused_monitor(dir).unwrap_or_default(),
        active_special,
    }
}

/// The compositor's own workspace list, carrying whatever the compositor's IPC can add to it.
///
/// One field, one owner. The protocol owns which workspaces exist, what they are called, their order, which is active and the output each sits on — every compositor with workspaces can say that much. It cannot say how many windows are on one, which applications those are, or that a scratchpad exists, and no other protocol can either: no toplevel protocol reports the workspace a window is on. Those come off Hyprland where there is a Hyprland, and are absent where there is not, rather than two sources disagreeing about one field.
fn merge(protocol: &[platform_wayland::Workspace], dir: Option<&Path>) -> Snapshot {
    merge_with(protocol, dir.map(read_facts).unwrap_or_default())
}

fn merge_with(protocol: &[platform_wayland::Workspace], facts: Facts) -> Snapshot {
    let mut workspaces: Vec<Workspace> = protocol
        .iter()
        .enumerate()
        // `hidden` is the compositor asking that a workspace not be drawn, which is its business and not a bar's.
        .filter(|(_, workspace)| !workspace.hidden)
        .map(|(position, workspace)| {
            let id = numbered_id(workspace, position);
            Workspace {
                id,
                name: workspace.name.clone(),
                windows: facts.windows.get(&id).copied().unwrap_or_default(),
                monitor: workspace.outputs.first().cloned().unwrap_or_default(),
                clients: facts.classes.get(&id).cloned().unwrap_or_default(),
                handle: Some(workspace.id),
            }
        })
        .collect();
    workspaces.extend(facts.specials);
    workspaces.sort_by_key(|w| (w.is_special(), w.id));

    let active = facts
        .active_special
        .or_else(|| {
            protocol
                .iter()
                .enumerate()
                .find(|(_, workspace)| workspace.active)
                .map(|(position, workspace)| numbered_id(workspace, position))
        })
        .unwrap_or_default();
    // Failing an IPC that names it, the output holding the active workspace: the only thing this side of the protocol that answers "which monitor is the user on".
    let focused_monitor = if facts.focused_monitor.is_empty() {
        protocol
            .iter()
            .find(|workspace| workspace.active)
            .and_then(|workspace| workspace.outputs.first().cloned())
            .unwrap_or_default()
    } else {
        facts.focused_monitor
    };
    Snapshot {
        workspaces,
        active,
        focused_monitor,
    }
}

static WORKSPACES: Service<Snapshot> = Service::new("hogar-shell-workspaces", run_workspaces);

/// The single shared workspaces source: publishes the current layout, then republishes on every event that could have changed it. Fanned out to every bar that subscribed, so N bars cost one parse per change (the M3 "one producer, N readers"), not N.
///
/// `ext-workspace-v1` first and Hyprland's socket only for what it cannot answer, which is the shell's standing preference for a protocol over one compositor's IPC. Both feed the same snapshot, so both republish it — hence the deduplication: a workspace switch is reported by the protocol *and* by the event stream, and publishing it twice would wake every subscribed surface for a reading that did not change.
fn run_workspaces(service: &Arc<Broadcast<Snapshot>>) {
    let dir = socket_dir();
    let last: Arc<Mutex<Option<Snapshot>>> = Arc::new(Mutex::new(None));
    // One claim for both registrations below, so the two retire together. Retiring them one at a time leaves a window in which a new subscriber arrives, `wanted` answers `true` again to whichever has not yet asked, it stays — and the fresh producer this service then starts registers a second reader of the same stream.
    let interest = Interest::new();

    let over_protocol = {
        let published = Arc::clone(service);
        let last = Arc::clone(&last);
        let dir = dir.clone();
        let owned = interest.clone();
        platform_wayland::watch_workspaces(&interest, move |workspaces: &[_]| {
            publish_changed(&published, &last, merge(workspaces, dir.as_deref()));
            if !published.wanted() {
                owned.retire();
            }
        })
    };

    // The broadcast outlives this call: the handler owns a clone of the `Arc` the service holds, so the producer thread can return once it has registered instead of parking on a socket of its own.
    let Some(dir) = dir else { return };
    let published = Arc::clone(service);
    if over_protocol {
        // Nothing the protocol publishes moves when a window opens or closes, and occupancy and the app icons are read from exactly that. The event stream is what republishes them.
        on_events_while_wanted(service, &interest, move |line| {
            if affects_workspaces(line) {
                let merged = merge(&platform_wayland::current_workspaces(), Some(&dir));
                publish_changed(&published, &last, merged);
            }
        });
        return;
    }

    if let Some(snapshot) = query_snapshot(&dir) {
        service.publish(snapshot);
    }
    on_events_while_wanted(service, &interest, move |line| {
        if affects_workspaces(line)
            && let Some(snapshot) = query_snapshot(&dir)
        {
            published.publish(snapshot);
        }
    });
}

/// Publishes a reading unless it is the one already published.
///
/// Every service here has two producers now — a protocol and a compositor's event stream — and the two report overlapping facts, so the same reading arrives twice for one change. Publishing it twice would wake every subscribed surface for something that did not move.
fn publish_changed<T: Clone + PartialEq>(
    service: &Broadcast<T>,
    last: &Mutex<Option<T>>,
    reading: T,
) {
    let mut last = last.lock().unwrap();
    if last.as_ref() == Some(&reading) {
        return;
    }
    *last = Some(reading.clone());
    service.publish(reading);
}

/// The last published workspace snapshot, with no socket round-trip — what a scroll handler steps from.
pub fn current_workspaces() -> Option<Snapshot> {
    WORKSPACES.current()
}

/// Registers `tx` (bound to a bar's event loop) for live workspace snapshots and sends the current one, spinning up the single shared Hyprland listener on first use. Called from a bar's `watch` producer.
pub fn subscribe(tx: EventSender<Snapshot>) {
    WORKSPACES.subscribe(tx);
}

/// Stands `snapshot` in for the compositor's own, without starting the listener — so a `[preview]` draws the workspaces it describes whether or not Hyprland is running. See [`util::broadcast::Service::seed`].
pub fn seed_workspaces(snapshot: Snapshot) {
    WORKSPACES.seed(snapshot);
}

/// Whether a line reports something that could have changed which window has focus, or its title.
fn affects_active_window(line: &str) -> bool {
    const PREFIXES: &[&str] = &[
        "activewindow>>",
        "activewindowv2>>",
        "windowtitle>>",
        "windowtitlev2>>",
        "closewindow>>",
        "openwindow>>",
        "workspace>>",
        "focusedmon>>",
        "fullscreen>>",
    ];
    PREFIXES.iter().any(|prefix| line.starts_with(prefix))
}

/// The focused window, or the empty value when nothing is focused. Hyprland answers `j/activewindow` with `{}` on an empty workspace, which deserializes to the default rather than failing.
pub fn active_window(dir: &Path) -> ActiveWindow {
    let Ok(raw) = request(dir, "j/activewindow") else {
        return ActiveWindow::default();
    };
    serde_json::from_str::<ActiveWindowJson>(&raw)
        .map(|w| ActiveWindow {
            title: w.title,
            class: w.class,
            address: w.address,
            handle: None,
        })
        .unwrap_or_default()
}

/// The focused window as `wlr-foreign-toplevel-management` reports it, carrying Hyprland's address where there is a Hyprland to ask.
///
/// The same rule the workspace list follows. The protocol owns which window has focus and what it is called — no other portable route answers the first at all, since `ext-foreign-toplevel-list-v1` lists windows without ever saying which is active. The address is Hyprland's alone, so Hyprland is asked for it, and it stays empty elsewhere rather than being invented. Whether what was last published already describes `focused`, so nothing has to be read or fanned out.
///
/// The protocol publishes the whole window list whenever *any* window commits a change, and a terminal retypes its title on nearly every keystroke. Without this, someone typing in a background window would cost a socket round trip per keystroke — exactly the cost reading a protocol was meant to remove.
fn already_published(
    published: Option<&ActiveWindow>,
    focused: Option<&platform_wayland::ManagedToplevel>,
) -> bool {
    match (published, focused) {
        (Some(previous), Some(current)) => {
            previous.handle == Some(current.id)
                && previous.title == current.title
                && previous.class == current.app_id
        }
        (Some(previous), None) => previous.is_empty(),
        (None, _) => false,
    }
}

fn active_from(focused: platform_wayland::ManagedToplevel, address: String) -> ActiveWindow {
    ActiveWindow {
        title: focused.title,
        class: focused.app_id,
        address,
        handle: Some(focused.id),
    }
}

static ACTIVE_WINDOW: Service<ActiveWindow> =
    Service::new("hogar-shell-active-window", run_active_window);

/// The focused window, published on every change and never twice for the same reading: a title changes on nearly every keystroke in a terminal or a browser, and most of those land on a window nobody is showing.
fn run_active_window(service: &Arc<Broadcast<ActiveWindow>>) {
    let dir = socket_dir();
    let last: Arc<Mutex<Option<ActiveWindow>>> = Arc::new(Mutex::new(None));

    let interest = Interest::new();
    let over_protocol = {
        let published = Arc::clone(service);
        let last = Arc::clone(&last);
        let dir = dir.clone();
        let owned = interest.clone();
        platform_wayland::watch_managed_toplevels(&interest, move |windows: &[_]| {
            let focused = windows.iter().find(|window| window.activated);
            if !already_published(last.lock().unwrap().as_ref(), focused) {
                let window = match focused {
                    Some(focused) => active_from(
                        focused.clone(),
                        dir.as_deref()
                            .map(|dir| active_window(dir).address)
                            .unwrap_or_default(),
                    ),
                    None => ActiveWindow::default(),
                };
                publish_changed(&published, &last, window);
            }
            // After the publishing and on every reading, not only the ones that moved: `Broadcast::wanted` releases the producer slot the moment it answers `false`, and a reading nobody wanted is exactly when this needs asking.
            if !published.wanted() {
                owned.retire();
            }
        })
    };
    if over_protocol {
        return;
    }

    let Some(dir) = dir else { return };
    let published = Arc::clone(service);
    publish_changed(&published, &last, active_window(&dir));
    on_events_while_wanted(service, &Interest::new(), move |line| {
        if affects_active_window(line) {
            publish_changed(&published, &last, active_window(&dir));
        }
    });
}

pub fn subscribe_active_window(tx: EventSender<ActiveWindow>) {
    ACTIVE_WINDOW.subscribe(tx);
}

/// The last published focused window, without a round trip to anything.
pub fn current_active_window() -> Option<ActiveWindow> {
    ACTIVE_WINDOW.current()
}

/// Stands `window` in for the focused one, without starting the listener — so a `[preview]` titles its chip the same whichever window has focus while it renders.
pub fn seed_active_window(window: ActiveWindow) {
    ACTIVE_WINDOW.seed(window);
}

/// Focuses a window over whichever route the compositor offers, preferring the protocol.
pub fn focus_active_window() {
    let Some(window) = current_active_window().filter(|window| !window.is_empty()) else {
        return;
    };
    if let Some(handle) = window.handle
        && platform_wayland::focus_toplevel(handle)
    {
        return;
    }
    if let Some(dir) = socket_dir() {
        focus_window(&dir, &window.address);
    }
}

/// Focuses a window by its Hyprland address (`0x…`), for clicking the active-window chip or a window list.
pub fn focus_window(dir: &Path, address: &str) {
    dispatch(
        dir,
        &format!("hl.dsp.focus({{ window = \"address:{address}\" }})"),
    );
}

/// The window actions live under `hl.dsp.window.<action>`, and which *shape* each takes cannot be read off the compositor: called outside a dispatch context they refuse to build at all, so the usual trick of passing a nonsense field and reading the "expected one of" reply gets nothing. The same rule the `dpms` dispatcher taught applies — try a shape, then check whether the compositor's own state moved. See [`set_dpms`].
///
/// The table form first, because it is the one shape a verified dispatcher (`focus`) is known to take.
fn window_calls(action: &str, address: &str, extra: &str) -> [String; 2] {
    [
        format!("hl.dsp.window.{action}({{ window = \"address:{address}\"{extra} }})"),
        format!("hl.dsp.window.{action}(\"address:{address}\")"),
    ]
}

/// Runs each shape until `moved` reports the compositor did what was asked. The state is re-read after each attempt rather than slept on: a dispatch reply arrives after the action, so the next query already sees it.
fn dispatch_until(dir: &Path, calls: [String; 2], moved: impl Fn(&Path) -> bool) -> bool {
    if moved(dir) {
        return true;
    }
    for call in calls {
        dispatch(dir, &call);
        if moved(dir) {
            return true;
        }
    }
    false
}

fn client_of(dir: &Path, address: &str) -> Option<Client> {
    clients(dir).into_iter().find(|c| c.address == address)
}

/// Closes a window.
///
/// The one action with no second shape to fall back on: a close either happened or it did not, and trying another spelling of it afterwards is how the wrong window gets closed twice. A refused dispatch is a log line.
pub fn close_window(dir: &Path, address: &str) {
    dispatch(
        dir,
        &format!("hl.dsp.window.close({{ window = \"address:{address}\" }})"),
    );
}

/// Floats or tiles a window, reporting whether the compositor agreed.
pub fn set_floating(dir: &Path, address: &str, floating: bool) -> bool {
    let address = address.to_string();
    dispatch_until(
        dir,
        window_calls("float", &address, &format!(", state = {floating}")),
        move |dir| client_of(dir, &address).is_some_and(|client| client.floating == floating),
    )
}

/// Puts a window into or out of fullscreen, reporting whether the compositor agreed.
pub fn set_fullscreen(dir: &Path, address: &str, fullscreen: bool) -> bool {
    let address = address.to_string();
    dispatch_until(
        dir,
        window_calls("fullscreen", &address, &format!(", state = {fullscreen}")),
        move |dir| client_of(dir, &address).is_some_and(|client| client.fullscreen == fullscreen),
    )
}

/// Moves a window to a workspace by id, reporting whether it arrived.
pub fn move_window_to_workspace(dir: &Path, address: &str, workspace: i32) -> bool {
    let address = address.to_string();
    let calls = [
        format!(
            "hl.dsp.window.move({{ window = \"address:{address}\", workspace = \"{workspace}\" }})"
        ),
        format!("hl.dsp.window.move(\"address:{address}\", \"{workspace}\")"),
    ];
    dispatch_until(dir, calls, move |dir| {
        client_of(dir, &address).is_some_and(|client| client.workspace == workspace)
    })
}

/// Whether a line reports something that could have changed the set of open windows, where they are, or how they are laid out. Deliberately wider than [`affects_workspaces`]: a window moving between monitors or toggling float changes nothing about the workspace pills and everything about a window list.
fn affects_clients(line: &str) -> bool {
    const PREFIXES: &[&str] = &[
        "openwindow>>",
        "closewindow>>",
        "movewindow>>",
        "movewindowv2>>",
        "windowtitle>>",
        "windowtitlev2>>",
        "fullscreen>>",
        "changefloatingmode>>",
        "pin>>",
        "minimize>>",
        "monitorremoved>>",
    ];
    PREFIXES.iter().any(|prefix| line.starts_with(prefix))
}

static CLIENTS: Service<Vec<Client>> = Service::new("hogar-shell-clients", run_clients);

/// The window list, republished whenever the compositor reports something that could have changed it. Costs one `j/clients` round-trip per such event — the same one the workspace pills already pay — and nothing at rest.
fn run_clients(service: &Arc<Broadcast<Vec<Client>>>) {
    let Some(dir) = socket_dir() else { return };
    let mut last = clients(&dir);
    service.publish(last.clone());
    let published = Arc::clone(service);
    on_events_while_wanted(service, &Interest::new(), move |line| {
        if !affects_clients(line) {
            return;
        }
        // A title changes on nearly every keystroke in a terminal, and most of those land on a window nobody is listing; republishing an identical list would wake every subscriber for nothing.
        let current = clients(&dir);
        if current != last {
            last = current.clone();
            published.publish(current);
        }
    });
}

pub fn subscribe_clients(tx: EventSender<Vec<Client>>) {
    CLIENTS.subscribe(tx);
}

/// The last published window list, with no socket round-trip — what a click handler acts on.
pub fn current_clients() -> Option<Vec<Client>> {
    CLIENTS.current()
}

fn affects_screens(line: &str) -> bool {
    const PREFIXES: &[&str] = &[
        "monitoradded>>",
        "monitoraddedv2>>",
        "monitorremoved>>",
        "monitorremovedv2>>",
        "focusedmon>>",
        "configreloaded",
    ];
    PREFIXES.iter().any(|prefix| line.starts_with(prefix))
}

static SCREENS: Service<Vec<Screen>> = Service::new("hogar-shell-screens", run_screens);

/// The output list. Separate from the compositor-agnostic `platform_wayland::outputs()` the surface layer reconciles against, which knows a Wayland output's name and nothing else: mode, scale, make and model only exist on this side, and a settings page listing monitors needs them.
fn run_screens(service: &Arc<Broadcast<Vec<Screen>>>) {
    let Some(dir) = socket_dir() else { return };
    let mut last = screens(&dir);
    service.publish(last.clone());
    let published = Arc::clone(service);
    on_events_while_wanted(service, &Interest::new(), move |line| {
        if !affects_screens(line) {
            return;
        }
        let current = screens(&dir);
        if current != last {
            last = current.clone();
            published.publish(current);
        }
    });
}

pub fn subscribe_screens(tx: EventSender<Vec<Screen>>) {
    SCREENS.subscribe(tx);
}

/// The last published output list, without a socket round-trip.
pub fn current_screens() -> Option<Vec<Screen>> {
    SCREENS.current()
}

/// The main keyboard's active layout, or `None` when Hyprland reports no keyboard.
pub fn keyboard_layout(dir: &Path) -> Option<KeyboardLayout> {
    let raw = request(dir, "j/devices").ok()?;
    let devices: DevicesJson = serde_json::from_str(&raw).ok()?;
    let keyboard = devices
        .keyboards
        .iter()
        .find(|k| k.main)
        .or_else(|| devices.keyboards.first())?;
    Some(KeyboardLayout {
        name: keyboard.active_keymap.clone(),
        device: keyboard.name.clone(),
    })
}

static KEYBOARD: Service<KeyboardLayout> = Service::new("hogar-shell-keyboard", run_keyboard);

fn run_keyboard(service: &Arc<Broadcast<KeyboardLayout>>) {
    let Some(dir) = socket_dir() else { return };
    if let Some(layout) = keyboard_layout(&dir) {
        service.publish(layout);
    }
    let published = Arc::clone(service);
    on_events_while_wanted(service, &Interest::new(), move |line| {
        if line.starts_with("activelayout>>")
            && let Some(layout) = keyboard_layout(&dir)
        {
            published.publish(layout);
        }
    });
}

pub fn subscribe_keyboard(tx: EventSender<KeyboardLayout>) {
    KEYBOARD.subscribe(tx);
}

/// The wire form of a layout switch: a bare top-level command with space-separated arguments, *not* a `dispatch` payload. Separated from the call so a test can hold the shape without switching anyone's keyboard.
fn switch_layout_command(device: &str, to: &str) -> String {
    format!("switchxkblayout {device} {to}")
}

/// Moves `device` to its next configured keyboard layout.
///
/// **Not a dispatcher, which is why this looked impossible.** `switchxkblayout` was a hyprlang dispatcher and was not carried into the Lua API: `hl.dsp` has no keyboard entry on 0.56, and `hl.device` is a config setter that returns nothing for a keyboard name. It survives as a *top-level IPC command* — the same kind as `devices` or `keyword` — so it goes over the socket verbatim rather than through `dispatch`. Looking only at `hl.dsp` is what made this read as unsupported.
///
/// Verified against the running compositor rather than assumed, the same rule `set_dpms` follows: `next` and an explicit index both answer `ok`, and a name no keyboard has answers `device not found`. That last one is what makes the reply worth reading — a wrong device fails silently otherwise.
///
/// Nothing is read back here. The compositor emits `activelayout>>` on a real change, which the keyboard service already watches, so the chip follows a switch made from a keybind exactly as it follows this one.
pub fn cycle_keyboard_layout(dir: &Path, device: &str) {
    let command = switch_layout_command(device, "next");
    match request(dir, &command) {
        Ok(reply) if reply.trim().eq_ignore_ascii_case("ok") => {}
        Ok(reply) => tracing::warn!("hogar-shell: `{command}` -> {reply:?}"),
        Err(e) => tracing::warn!("hogar-shell: `{command}` failed: {e}"),
    }
}

/// Cycles the layout of whichever keyboard [`keyboard_layout`] reports, for a caller that has no device in hand — a bar chip, a keybind. Resolved per call rather than remembered: the main keyboard is exactly the thing that changes when one is plugged in.
pub fn cycle_main_keyboard_layout() {
    let Some(dir) = socket_dir() else { return };
    let Some(layout) = keyboard_layout(&dir) else {
        tracing::warn!("hogar-shell: no keyboard to switch the layout of");
        return;
    };
    cycle_keyboard_layout(&dir, &layout.device);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The event stream is one socket shared by six services, so it stops only when the *last* of them has gone — and having stopped, it has to let the next one open a working socket rather than believing one is already open.
    #[test]
    fn the_event_stream_lives_exactly_as_long_as_its_readers() {
        *EVENT_THREAD.lock().unwrap() = true;
        let interest = Interest::new();
        HANDLERS.lock().unwrap().push(Registration {
            interest: interest.clone(),
            handler: Box::new(|_| {}),
        });
        assert!(anyone_reading(), "a live registration is reading");
        assert!(
            !retire_event_stream(),
            "a service is still reading, so the socket stays open"
        );

        interest.retire();
        assert!(!anyone_reading(), "a retired registration was kept");
        assert!(retire_event_stream(), "nothing is reading the stream");
        assert!(
            !*EVENT_THREAD.lock().unwrap(),
            "the flag has to say 'no' again, or no later producer could open a socket"
        );
    }

    /// A layout switch is a top-level command, not a dispatch, and the difference is the whole feature.
    ///
    /// `switchxkblayout` was a hyprlang dispatcher and did not survive into the Lua API, which is what made this read as impossible for a release: `hl.dsp` has no keyboard entry and `hl.device` answers nothing for a keyboard name. It is still there as a bare IPC command, the same kind as `devices`. Wrapping it in `dispatch …` — the shape every other mutation here takes — is the one plausible way to get this wrong, and the compositor answers a Lua parse error rather than anything that looks like a missing feature.
    #[test]
    fn a_layout_switch_goes_over_the_socket_bare_rather_than_as_a_dispatch() {
        let command = switch_layout_command("at-translated-set-2-keyboard", "next");
        assert_eq!(command, "switchxkblayout at-translated-set-2-keyboard next");
        assert!(
            !command.contains("dispatch") && !command.contains("hl.dsp"),
            "a dispatch payload would be answered with a Lua parse error, not a missing-feature error: {command}"
        );
        // An index is the other spelling the compositor takes, and the caller builds it the same way.
        assert_eq!(switch_layout_command("kbd", "0"), "switchxkblayout kbd 0");
    }

    #[test]
    fn event_filters_match_their_own_events_and_nothing_else() {
        assert!(affects_workspaces("workspacev2>>3,3"));
        assert!(affects_workspaces("openwindow>>0x1,3,kitty,term"));
        assert!(!affects_workspaces("activelayout>>kbd,English (US)"));

        assert!(affects_active_window("activewindowv2>>0x55f1"));
        assert!(affects_active_window("windowtitle>>0x55f1"));
        assert!(!affects_active_window("activelayout>>kbd,English (US)"));
        assert!(
            !affects_active_window("createworkspace>>4"),
            "a workspace appearing does not change which window has focus"
        );
    }

    #[test]
    fn an_empty_workspace_reports_no_active_window_rather_than_failing() {
        // Hyprland answers `j/activewindow` with `{}` when nothing is focused.
        let parsed: ActiveWindowJson = serde_json::from_str("{}").expect("an empty object parses");
        let window = ActiveWindow {
            title: parsed.title,
            class: parsed.class,
            address: parsed.address,
            handle: None,
        };
        assert!(window.is_empty());
        assert_eq!(window, ActiveWindow::default());
    }

    #[test]
    fn active_window_json_keeps_the_fields_a_chip_shows() {
        let raw = r#"{"address":"0x55f1","class":"firefox","title":"Docs — Firefox","pid":42}"#;
        let parsed: ActiveWindowJson = serde_json::from_str(raw).unwrap();
        assert_eq!(parsed.class, "firefox");
        assert_eq!(parsed.title, "Docs — Firefox");
        assert_eq!(parsed.address, "0x55f1", "unknown fields are ignored");
    }

    #[test]
    fn keyboard_layout_prefers_the_main_keyboard() {
        let raw = r#"{"keyboards":[
            {"name":"usb-kbd","main":false,"active_keymap":"German"},
            {"name":"builtin","main":true,"active_keymap":"English (US)"}
        ]}"#;
        let devices: DevicesJson = serde_json::from_str(raw).unwrap();
        let main = devices.keyboards.iter().find(|k| k.main).unwrap();
        assert_eq!(main.active_keymap, "English (US)");
        assert_eq!(main.name, "builtin", "the device name is what switches it");
    }

    #[test]
    fn the_client_list_and_the_workspace_pills_come_from_one_parse() {
        let raw = r#"[
            {"address":"0x1","class":"kitty","title":"nvim","pid":10,"workspace":{"id":3,"name":"3"},
             "monitor":0,"at":[10,20],"size":[800,600],"floating":false,"fullscreen":0,"mapped":true,
             "pinned":false,"xwayland":false},
            {"address":"0x2","class":"firefox","title":"Docs","pid":11,"workspace":{"id":3,"name":"3"},
             "monitor":0,"at":[0,0],"size":[1920,1080],"floating":true,"fullscreen":2,"mapped":true,
             "pinned":true,"xwayland":true},
            {"address":"0x3","class":"","title":"","pid":12,"workspace":{"id":-99,"name":"special:magic"},
             "monitor":1,"at":[0,0],"size":[1,1],"floating":false,"fullscreen":0,"mapped":false,
             "pinned":false,"xwayland":false}
        ]"#;
        let parsed: Vec<ClientJson> = serde_json::from_str(raw).expect("the client list parses");
        let list: Vec<Client> = parsed
            .into_iter()
            .map(|c| Client {
                address: c.address,
                class: c.class,
                title: c.title,
                pid: c.pid,
                workspace: c.workspace.id,
                workspace_name: c.workspace.name,
                monitor: c.monitor,
                at: c.at,
                size: c.size,
                floating: c.floating,
                fullscreen: c.fullscreen.is_set(),
                pinned: c.pinned,
                mapped: c.mapped,
                xwayland: c.xwayland,
            })
            .collect();

        assert_eq!(list[0].at, (10, 20));
        assert_eq!(list[0].size, (800, 600));
        assert!(!list[0].fullscreen);
        assert!(list[1].fullscreen, "mode 2 is fullscreen");
        assert!(list[1].pinned && list[1].xwayland);
        assert_eq!(list[2].workspace_name, "special:magic");
        assert!(!list[2].mapped, "an unmapped window stays in the list");

        // The pills read the same parse, and a window with no class draws no icon rather than an empty slot.
        let grouped = classes_by_workspace(&list);
        assert_eq!(
            grouped[&3],
            vec!["kitty".to_string(), "firefox".to_string()]
        );
        assert!(
            !grouped.contains_key(&-99),
            "the only window there has no class"
        );
    }

    #[test]
    fn the_fullscreen_field_is_read_as_both_a_flag_and_a_mode() {
        // Hyprland changed the type in 0.42; a shell that only understood one would fail the whole parse on the other, losing the window list rather than one field.
        let flag: ClientJson =
            serde_json::from_str(r#"{"workspace":{"id":1},"fullscreen":true}"#).unwrap();
        assert!(flag.fullscreen.is_set());
        let mode: ClientJson =
            serde_json::from_str(r#"{"workspace":{"id":1},"fullscreen":1}"#).unwrap();
        assert!(
            mode.fullscreen.is_set(),
            "1 is maximized, which still covers"
        );
        let none: ClientJson = serde_json::from_str(r#"{"workspace":{"id":1}}"#).unwrap();
        assert!(
            !none.fullscreen.is_set(),
            "an absent field is not fullscreen"
        );
    }

    #[test]
    fn screens_keep_the_fields_a_monitor_list_shows() {
        let raw = r#"[
            {"name":"DP-1","description":"Dell U2720Q","make":"Dell","model":"U2720Q","serial":"ABC",
             "width":3840,"height":2160,"refreshRate":59.997,"x":1920,"y":0,"scale":2.0,"transform":0,
             "focused":true,"disabled":false,"vrr":false,"dpmsStatus":true,
             "activeWorkspace":{"id":3,"name":"3"}},
            {"name":"eDP-1","focused":false}
        ]"#;
        let parsed: Vec<MonitorJson> = serde_json::from_str(raw).expect("the monitor list parses");
        assert_eq!(parsed[0].refresh_rate, 59.997);
        assert_eq!(parsed[0].active_workspace.as_ref().unwrap().id, 3);
        // A minimal entry still parses: the fields a per-monitor list needs are optional, the connector is not.
        assert_eq!(parsed[1].name, "eDP-1");
        assert_eq!(parsed[1].scale, 1.0, "an absent scale is 1, not 0");
        assert!(
            parsed[1].dpms_status,
            "an absent dpms state means the output is on"
        );
    }

    #[test]
    fn the_client_and_screen_filters_take_the_events_their_lists_depend_on() {
        assert!(affects_clients("changefloatingmode>>0x1,1"));
        assert!(
            affects_clients("movewindowv2>>0x1,3,3"),
            "a window changing workspace changes the list"
        );
        assert!(
            !affects_clients("activelayout>>kbd,English (US)"),
            "a keyboard layout does not move a window"
        );
        assert!(
            !affects_clients("createworkspace>>4"),
            "an empty workspace has no windows to list"
        );

        assert!(affects_screens("monitoraddedv2>>1,DP-2,Dell"));
        assert!(affects_screens("focusedmon>>DP-1,3"));
        assert!(
            !affects_screens("openwindow>>0x1,3,kitty,term"),
            "opening a window does not change the output list"
        );
    }

    fn protocol_workspace(
        name: &str,
        coordinate: u32,
        active: bool,
    ) -> platform_wayland::Workspace {
        platform_wayland::Workspace {
            name: name.to_string(),
            coordinates: vec![coordinate],
            outputs: vec!["eDP-1".to_string()],
            active,
            ..platform_wayland::Workspace::default()
        }
    }

    /// The reading a compositor that is not Hyprland produces: everything the protocol carries, and nothing invented for what it does not.
    #[test]
    fn without_an_ipc_the_protocol_is_the_whole_snapshot() {
        let protocol = vec![
            protocol_workspace("1", 1, false),
            protocol_workspace("2", 2, true),
        ];
        let snapshot = merge_with(&protocol, Facts::default());

        assert_eq!(snapshot.active, 2);
        assert_eq!(snapshot.workspaces.len(), 2);
        assert_eq!(snapshot.workspaces[0].id, 1);
        assert_eq!(snapshot.workspaces[0].monitor, "eDP-1");
        assert!(
            snapshot.workspaces.iter().all(|w| !w.is_occupied()),
            "occupancy has no protocol source, and an unknown count is not a full workspace"
        );
        assert!(snapshot.workspaces.iter().all(|w| w.clients.is_empty()));
        assert!(
            snapshot.workspaces.iter().all(|w| w.handle.is_some()),
            "a listed workspace is activatable over the protocol that listed it"
        );
        assert_eq!(
            snapshot.focused_monitor, "eDP-1",
            "failing an IPC, the output holding the active workspace is what answers this"
        );
    }

    /// One field, one owner: the protocol's rows carry the IPC's counts and classes, and the scratchpads the protocol never lists are the IPC's own rows.
    #[test]
    fn the_ipc_fills_in_only_what_the_protocol_cannot_say() {
        let protocol = vec![
            protocol_workspace("1", 1, true),
            protocol_workspace("2", 2, false),
        ];
        let facts = Facts {
            windows: HashMap::from([(1, 2), (2, 0), (-99, 1)]),
            classes: HashMap::from([(1, vec!["kitty".to_string(), "code".to_string()])]),
            specials: vec![Workspace {
                id: -99,
                name: "special:magic".to_string(),
                windows: 1,
                monitor: "eDP-1".to_string(),
                clients: vec!["helium".to_string()],
                handle: None,
            }],
            focused_monitor: "DP-2".to_string(),
            active_special: None,
        };
        let snapshot = merge_with(&protocol, facts);

        assert_eq!(snapshot.workspaces[0].windows, 2);
        assert_eq!(snapshot.workspaces[0].clients, vec!["kitty", "code"]);
        assert_eq!(
            snapshot.focused_monitor, "DP-2",
            "the compositor that can name the focused monitor owns that field"
        );

        let special = snapshot
            .workspaces
            .last()
            .expect("the scratchpad is listed");
        assert_eq!(special.id, -99);
        assert!(
            special.is_special() && special.handle.is_none(),
            "a scratchpad has no protocol handle because the protocol never listed it"
        );
        assert_eq!(
            snapshot.workspaces.len(),
            3,
            "the protocol's rows plus the ones only the IPC knows about"
        );
    }

    /// Hyprland goes on reporting the numbered workspace as active while a scratchpad is up, so the one thing the IPC has to be allowed to override is which workspace is active.
    #[test]
    fn an_active_scratchpad_wins_over_the_workspace_beneath_it() {
        let protocol = vec![protocol_workspace("2", 2, true)];
        let snapshot = merge_with(
            &protocol,
            Facts {
                active_special: Some(-99),
                ..Facts::default()
            },
        );
        assert_eq!(snapshot.active, -99);
    }

    /// The focused window off Hyprland: everything a chip draws, and no address, because no protocol has one.
    #[test]
    fn a_window_with_no_address_is_still_a_focused_window() {
        let focused = platform_wayland::ManagedToplevel {
            title: "nvim".to_string(),
            app_id: "kitty".to_string(),
            activated: true,
            ..platform_wayland::ManagedToplevel::default()
        };
        let window = active_from(focused, String::new());

        assert_eq!(window.title, "nvim");
        assert_eq!(window.class, "kitty", "`class` is the protocol's `app_id`");
        assert!(
            !window.is_empty(),
            "asking only about the address would report every window off Hyprland as no window at all"
        );
        assert!(window.address.is_empty());
    }

    /// The guard that keeps someone typing in a background window from costing a socket round trip a keystroke.
    #[test]
    fn a_reading_that_did_not_move_is_not_read_again() {
        let focused = platform_wayland::ManagedToplevel {
            id: platform_wayland::ManagedToplevelId::default(),
            title: "nvim".to_string(),
            app_id: "kitty".to_string(),
            activated: true,
            ..platform_wayland::ManagedToplevel::default()
        };
        let published = active_from(focused.clone(), "0x1".to_string());

        assert!(already_published(Some(&published), Some(&focused)));

        let retitled = platform_wayland::ManagedToplevel {
            title: "cargo test".to_string(),
            ..focused.clone()
        };
        assert!(
            !already_published(Some(&published), Some(&retitled)),
            "the focused window's own title changing is the case that must get through"
        );
        assert!(
            !already_published(Some(&published), None),
            "and so is focus leaving every window"
        );
        assert!(
            already_published(Some(&ActiveWindow::default()), None),
            "nothing focused, and nothing focused already published"
        );
        assert!(
            !already_published(None, None),
            "the first reading is always published, even an empty one"
        );
    }

    #[test]
    fn the_address_is_carried_through_where_there_is_one() {
        let window = active_from(
            platform_wayland::ManagedToplevel::default(),
            "0x5fc3".to_string(),
        );
        assert_eq!(window.address, "0x5fc3");
        assert!(!window.is_empty());
    }

    #[test]
    fn a_hidden_workspace_is_not_drawn() {
        let mut hidden = protocol_workspace("3", 3, false);
        hidden.hidden = true;
        let snapshot = merge_with(
            &[protocol_workspace("1", 1, true), hidden],
            Facts::default(),
        );
        assert_eq!(snapshot.workspaces.len(), 1);
    }

    /// The protocol has no numeric id — Hyprland sends none at all — so the number a pill shows is recovered.
    #[test]
    fn the_workspace_number_is_recovered_from_whatever_the_compositor_did_send() {
        let named = protocol_workspace("7", 7, false);
        assert_eq!(numbered_id(&named, 0), 7, "the name, when it is a number");

        let mut lettered = protocol_workspace("web", 4, false);
        assert_eq!(
            numbered_id(&lettered, 0),
            4,
            "otherwise the coordinate, which is the compositor's own ordering"
        );

        lettered.coordinates.clear();
        assert_eq!(
            numbered_id(&lettered, 2),
            3,
            "and failing both, the position — so two workspaces never collide on zero"
        );
    }

    #[test]
    fn monitor_from_focus_event_reads_the_name_before_the_comma() {
        assert_eq!(
            monitor_from_focus_event("focusedmon>>DP-1,3"),
            Some("DP-1".to_string())
        );
        assert_eq!(monitor_from_focus_event("workspace>>3"), None);
    }
}
