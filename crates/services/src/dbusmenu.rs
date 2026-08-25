//! A `com.canonical.dbusmenu` client: the menu behind a tray icon.
//!
//! Not a [`Service`](util::broadcast::Service), because a menu is not ambient state — it is fetched when the user asks to see one and thrown away when it closes. What it *is* is a D-Bus round trip to another application, so every entry point here runs off the UI thread; the fetch is a `watch` producer whose result lands back on the driver thread, which is the one place a surface may be opened.
//!
//! This is the only way to interact with a good part of the tray. Applications built on libappindicator — Steam among them — implement no `Activate` at all and expose a menu instead, so without this their icon is decoration.

use std::collections::HashMap;
use std::time::Duration;

use platform_wayland::EventSender;
use zbus::blocking::Connection;
use zbus::names::BusName;
use zbus::zvariant::{OwnedValue, Value};

const MENU_IFACE: &str = "com.canonical.dbusmenu";

/// An application that accepts a call and never replies must not strand the menu behind it. Applied to the whole connection, so it bounds `AboutToShow` and `GetLayout` alike — a method the application simply does not implement errors immediately and never reaches this.
const METHOD_TIMEOUT: Duration = Duration::from_secs(2);

/// What a row's tick renders as, per the spec's `toggle-type`/`toggle-state` pair.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Toggle {
    #[default]
    None,
    Checkmark(bool),
    Radio(bool),
}

impl Toggle {
    fn parse(kind: &str, state: i32) -> Self {
        // `-1` is the spec's "indeterminate", which reads better as off than as a third glyph nobody expects.
        let on = state == 1;
        match kind {
            "checkmark" => Self::Checkmark(on),
            "radio" => Self::Radio(on),
            _ => Self::None,
        }
    }

    pub fn is_on(self) -> bool {
        matches!(self, Self::Checkmark(true) | Self::Radio(true))
    }
}

/// One row of a menu, and its submenu if it opens one.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MenuItem {
    pub id: i32,
    pub label: String,
    pub enabled: bool,
    pub separator: bool,
    pub toggle: Toggle,
    /// A themed icon name for the row, resolved through the same freedesktop lookup as an app icon.
    pub icon_name: String,
    /// Raw PNG bytes the application supplied instead of a name (`icon-data`).
    pub icon_data: Option<Vec<u8>>,
    pub children: Vec<MenuItem>,
}

impl MenuItem {
    pub fn has_submenu(&self) -> bool {
        !self.children.is_empty()
    }

    /// Whether this row does something when clicked. A separator, a disabled row, and a row that only opens a submenu are all "not an action".
    pub fn is_actionable(&self) -> bool {
        self.enabled && !self.separator && !self.has_submenu()
    }
}

/// Strips GTK mnemonic underscores: `_Store` is "Store" with S underlined, and `__` is one literal underscore. Rendering them raw is the difference between a menu that looks native and one that looks broken.
fn strip_mnemonics(label: &str) -> String {
    let mut out = String::with_capacity(label.len());
    let mut chars = label.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '_' {
            if chars.peek() == Some(&'_') {
                chars.next();
                out.push('_');
            }
            continue;
        }
        out.push(c);
    }
    out
}

/// The wire shape of one node: `(ia{sv}av)` — id, properties, children as variants.
type RawNode = (i32, HashMap<String, OwnedValue>, Vec<OwnedValue>);

/// A node's properties as borrowed values. Deliberately not `OwnedValue`: converting each one is fallible, and a conversion that quietly fails turns every label into an empty string rather than into an error anybody would notice.
type Props<'a> = [(String, Value<'a>)];

fn lookup<'a, 'v>(props: &'a Props<'v>, key: &str) -> Option<&'a Value<'v>> {
    props.iter().find(|(k, _)| k == key).map(|(_, v)| v)
}

fn string_property(props: &Props<'_>, key: &str) -> String {
    match lookup(props, key) {
        Some(Value::Str(s)) => s.to_string(),
        // Some applications hand a property over still wrapped in a variant.
        Some(Value::Value(inner)) => match inner.as_ref() {
            Value::Str(s) => s.to_string(),
            _ => String::new(),
        },
        _ => String::new(),
    }
}

fn bool_property(props: &Props<'_>, key: &str, default: bool) -> bool {
    match lookup(props, key) {
        Some(Value::Bool(b)) => *b,
        Some(Value::Value(inner)) => match inner.as_ref() {
            Value::Bool(b) => *b,
            _ => default,
        },
        _ => default,
    }
}

fn i32_property(props: &Props<'_>, key: &str) -> i32 {
    match lookup(props, key) {
        Some(Value::I32(v)) => *v,
        Some(Value::Value(inner)) => match inner.as_ref() {
            Value::I32(v) => *v,
            _ => 0,
        },
        _ => 0,
    }
}

fn bytes_property(props: &Props<'_>, key: &str) -> Option<Vec<u8>> {
    let value = match lookup(props, key)? {
        Value::Value(inner) => inner.as_ref(),
        other => other,
    };
    let Value::Array(array) = value else {
        return None;
    };
    let bytes: Vec<u8> = array.iter().filter_map(|b| u8::try_from(b).ok()).collect();
    (!bytes.is_empty()).then_some(bytes)
}

/// Turns one `(ia{sv}av)` node — id, properties, children — into a [`MenuItem`], recursively.
///
/// Invisible rows are dropped here rather than at render time: an application uses `visible` to switch whole blocks of its menu on and off, and carrying them into the view only to skip them would leave the separators around them stranded. Builds a node from its own properties and its already-parsed children. Shared by the root, which arrives deserialized as a tuple, and by every child, which arrives as a raw variant.
fn build_item(id: i32, props: &Props<'_>, children: Vec<MenuItem>) -> Option<MenuItem> {
    if !bool_property(props, "visible", true) {
        return None;
    }
    Some(MenuItem {
        id,
        label: strip_mnemonics(&string_property(props, "label")),
        enabled: bool_property(props, "enabled", true),
        separator: string_property(props, "type") == "separator",
        toggle: Toggle::parse(
            &string_property(props, "toggle-type"),
            i32_property(props, "toggle-state"),
        ),
        icon_name: string_property(props, "icon-name"),
        icon_data: bytes_property(props, "icon-data"),
        children,
    })
}

fn parse_node((id, props, children): RawNode) -> Option<MenuItem> {
    let children: Vec<MenuItem> = children
        .iter()
        .filter_map(|child| parse_child(&Value::from(child.clone())))
        .collect();
    let props: Vec<(String, Value<'_>)> = props
        .iter()
        .map(|(k, v)| (k.clone(), Value::from(v.clone())))
        .collect();
    build_item(id, &props, children)
}

/// A child arrives as a variant wrapping the same `(ia{sv}av)` shape, so it is taken apart by hand rather than deserialized: the type is recursive, and `Value` is where that recursion bottoms out.
fn parse_child(value: &Value<'_>) -> Option<MenuItem> {
    let node = match value {
        Value::Value(inner) => return parse_child(inner),
        Value::Structure(node) => node,
        _ => return None,
    };
    let fields = node.fields();
    if fields.len() < 3 {
        return None;
    }
    let id = i32::try_from(&fields[0]).ok()?;
    let Value::Dict(dict) = &fields[1] else {
        return None;
    };
    let props: Vec<(String, Value<'_>)> = dict
        .iter()
        .filter_map(|(k, v)| match k {
            Value::Str(key) => Some((key.to_string(), v.clone())),
            _ => None,
        })
        .collect();
    let children: Vec<MenuItem> = match &fields[2] {
        Value::Array(list) => list.iter().filter_map(parse_child).collect(),
        _ => Vec::new(),
    };
    build_item(id, &props, children)
}

fn connect(bus: &str) -> Option<(Connection, BusName<'static>)> {
    let conn = crate::bus::session(Some(METHOD_TIMEOUT))?;
    let name = BusName::try_from(bus.to_string()).ok()?;
    Some((conn, name))
}

/// Asks the application to refresh the menu it is about to show. Applications populate lazily — Steam's recent games are filled in here — so skipping this shows a stale or empty menu on the first open.
fn about_to_show(conn: &Connection, name: &BusName<'_>, path: &str, id: i32) {
    let reply = conn.call_method(Some(name), path, Some(MENU_IFACE), "AboutToShow", &id);
    if let Err(e) = reply {
        tracing::debug!("dbusmenu AboutToShow on {name}: {e}");
    }
}

/// The whole menu tree under `path`, or `None` when the application does not answer.
pub fn fetch(bus: &str, path: &str) -> Option<MenuItem> {
    let (conn, name) = connect(bus)?;
    about_to_show(&conn, &name, path, 0);
    // Depth -1 is the whole tree in one call: a submenu opened later would otherwise cost another round trip while the pointer waits on it.
    let reply = conn
        .call_method(
            Some(&name),
            path,
            Some(MENU_IFACE),
            "GetLayout",
            &(0i32, -1i32, Vec::<String>::new()),
        )
        .map_err(|e| tracing::warn!("dbusmenu GetLayout on {bus}{path}: {e}"))
        .ok()?;
    let (_revision, layout): (u32, RawNode) = reply
        .body()
        .deserialize()
        .map_err(|e| tracing::warn!("dbusmenu layout from {bus}{path} did not parse: {e}"))
        .ok()?;
    parse_node(layout)
}

/// Fetches the menu on a worker thread and delivers it to `tx`, which [`platform_wayland::watch`] drains on the driver thread — the only place a surface may be opened. A one-shot producer: it sends once and returns, so the channel closes and the watch source retires with it.
pub fn fetch_into(bus: String, path: String) -> impl FnOnce(EventSender<Option<MenuItem>>) {
    move |tx| {
        tx.send(fetch(&bus, &path));
    }
}

/// Reports a row's activation back to the application. Fire-and-forget on a thread of its own: the application may take as long as it likes to act on it, and the click that triggered this happened on the UI thread.
pub fn activate(bus: &str, path: &str, id: i32) {
    let (bus, path) = (bus.to_string(), path.to_string());
    let _ = std::thread::Builder::new()
        .name("hogar-shell-dbusmenu-event".to_string())
        .spawn(move || {
            let Some((conn, name)) = connect(&bus) else {
                return;
            };
            // The spec's signature is (id, eventId, data, timestamp); `data` is unused for a click, and a timestamp of 0 is what every other host sends.
            let data = Value::I32(0);
            if let Err(e) = conn.call_method(
                Some(&name),
                path.as_str(),
                Some(MENU_IFACE),
                "Event",
                &(id, "clicked", &data, 0u32),
            ) {
                tracing::warn!("dbusmenu Event on {bus}: {e}");
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mnemonic_underscores_are_stripped_but_a_doubled_one_survives() {
        assert_eq!(strip_mnemonics("_Store"), "Store");
        assert_eq!(strip_mnemonics("E_xit"), "Exit");
        assert_eq!(strip_mnemonics("Save __as"), "Save _as");
        assert_eq!(strip_mnemonics("no mnemonic"), "no mnemonic");
        assert_eq!(strip_mnemonics(""), "");
    }

    #[test]
    fn a_toggle_reads_its_type_and_state() {
        assert_eq!(Toggle::parse("checkmark", 1), Toggle::Checkmark(true));
        assert_eq!(Toggle::parse("checkmark", 0), Toggle::Checkmark(false));
        assert_eq!(Toggle::parse("radio", 1), Toggle::Radio(true));
        assert_eq!(Toggle::parse("", 1), Toggle::None, "no type means no tick");
        assert_eq!(
            Toggle::parse("checkmark", -1),
            Toggle::Checkmark(false),
            "indeterminate reads as off rather than as a glyph nobody expects"
        );
        assert!(Toggle::Radio(true).is_on());
        assert!(!Toggle::None.is_on());
    }

    // Reads a real menu off the session bus, gated behind an env var so it never runs in headless CI: run with `HOGAR_SHELL_TEST_DBUSMENU=<bus><path> cargo test -p hogar-shell --lib dbusmenu_reads -- --nocapture`, e.g. `HOGAR_SHELL_TEST_DBUSMENU=":1.502/org/ayatana/NotificationItem/steam/Menu"`.
    #[test]
    fn dbusmenu_reads_a_live_menu() {
        let Ok(target) = std::env::var("HOGAR_SHELL_TEST_DBUSMENU") else {
            eprintln!("set HOGAR_SHELL_TEST_DBUSMENU to read a live menu; skipping");
            return;
        };
        let (bus, path) = target.split_once('/').expect("bus/path");
        let root = fetch(bus, &format!("/{path}")).expect("the application answered GetLayout");
        eprintln!("root id {} with {} children", root.id, root.children.len());
        for child in &root.children {
            eprintln!(
                "  [{}] {:?} enabled={} separator={} submenu={} toggle={:?}",
                child.id,
                child.label,
                child.enabled,
                child.separator,
                child.has_submenu(),
                child.toggle
            );
        }
        assert!(!root.children.is_empty(), "a real menu has rows");
    }

    #[test]
    fn only_a_leaf_that_is_enabled_and_not_a_separator_is_actionable() {
        let leaf = MenuItem {
            enabled: true,
            ..MenuItem::default()
        };
        assert!(leaf.is_actionable());

        let disabled = MenuItem {
            enabled: false,
            ..leaf.clone()
        };
        assert!(!disabled.is_actionable());

        let separator = MenuItem {
            separator: true,
            ..leaf.clone()
        };
        assert!(!separator.is_actionable());

        let parent = MenuItem {
            children: vec![leaf.clone()],
            ..leaf.clone()
        };
        assert!(
            !parent.is_actionable(),
            "a row that opens a submenu is navigation, not an action"
        );
        assert!(parent.has_submenu());
    }
}
