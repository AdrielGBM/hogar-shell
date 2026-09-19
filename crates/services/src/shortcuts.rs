//! Global shortcuts, registered on the desktop portal so the compositor can bind them by name.
//!
//! Keybinds already work without this — `bind = SUPER, N, exec, hogar-shell panel toggle notifications` spawns the client, which talks to the running shell over its socket. What that costs is a process launch per press: a fork, an exec, a dynamic link and a connect, to deliver one line the shell answers in microseconds. A portal shortcut is the same line delivered over a connection that is already open.
//!
//! The trade is that the *binding* moves out of the shell's hands. hogar-shell says "I have an action called `launcher`"; the compositor decides which keys reach it, and the user writes `bind = SUPER, SPACE, global, hogar-shell:launcher`. That is the point of the portal — one place that knows every application's shortcuts, so two applications cannot silently claim the same chord.
//!
//! **Entirely optional.** No portal, no session, a portal that refuses: the service logs once and retires, and every `exec, hogar-shell …` bind keeps working exactly as before. Nothing else in the shell asks it anything.

use std::collections::HashMap;
use std::time::Duration;

use platform_wayland::EventSender;
use zbus::blocking::{Connection, MessageIterator, Proxy};
use zbus::message::Type as MessageType;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

use crate::command::Request;

const PORTAL: &str = "org.freedesktop.portal.Desktop";
const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
const SHORTCUTS: &str = "org.freedesktop.portal.GlobalShortcuts";
const REQUEST: &str = "org.freedesktop.portal.Request";

/// How long a portal call may take before the service gives up. A portal that never answers must not leave a thread parked for the process's life — the same rule every other D-Bus integration here follows.
const PORTAL_TIMEOUT: Duration = Duration::from_secs(10);

/// One action the compositor can bind, and the request line it runs.
///
/// The ids are deliberately the *actions a user binds a key to*, not a mirror of the IPC table: `hogar-shell audio set 40` is a scripting command, not a shortcut, and offering every command here would bury the six that anyone actually binds. `description` is what the compositor's own settings UI shows.
pub struct Shortcut {
    pub id: &'static str,
    pub description: &'static str,
    pub command: &'static str,
}

/// Every action registered with the portal. Adding one here is all it takes; the user then binds it with `bind = <mods>, <key>, global, <appid>:<id>`.
pub const SHORTCUT_TABLE: &[Shortcut] = &[
    Shortcut {
        id: "launcher",
        description: "Open the application launcher",
        command: "launcher toggle",
    },
    Shortcut {
        id: "dashboard",
        description: "Open the dashboard",
        command: "dashboard toggle",
    },
    Shortcut {
        id: "notifications",
        description: "Open the notification history",
        command: "panel toggle notifications",
    },
    Shortcut {
        id: "session",
        description: "Open the session menu",
        command: "panel toggle session",
    },
    Shortcut {
        id: "dnd",
        description: "Toggle do-not-disturb",
        command: "notifs dnd toggle",
    },
    Shortcut {
        id: "volume-up",
        description: "Raise the volume",
        command: "volume up",
    },
    Shortcut {
        id: "volume-down",
        description: "Lower the volume",
        command: "volume down",
    },
    Shortcut {
        id: "volume-mute",
        description: "Mute the volume",
        command: "volume mute",
    },
    Shortcut {
        id: "mic-mute",
        description: "Mute the microphone",
        command: "mic mute",
    },
    Shortcut {
        id: "brightness-up",
        description: "Raise the screen brightness",
        command: "brightness up",
    },
    Shortcut {
        id: "brightness-down",
        description: "Lower the screen brightness",
        command: "brightness down",
    },
];

/// The request line a shortcut id runs, or `None` for an id the shell does not offer.
fn command_for(id: &str) -> Option<&'static str> {
    SHORTCUT_TABLE
        .iter()
        .find(|s| s.id == id)
        .map(|s| s.command)
}

/// The producer for `platform_wayland::watch`: registers the shortcuts, then turns every `Activated` signal into the same [`Request`] the socket would have delivered, so a shortcut and a `hogar-shell …` invocation run through one code path and cannot drift apart.
pub fn serve(tx: EventSender<Request>) {
    let Some(conn) = crate::bus::private_session(None) else {
        tracing::info!(
            "global shortcuts: no session bus; keybinds still work through `exec, hogar-shell …`"
        );
        return;
    };
    let Some(session) = register(&conn) else {
        return;
    };
    tracing::info!(
        "global shortcuts: {} action(s) registered; bind them with `global, <appid>:<id>` \
         (`hyprctl globalshortcuts` lists the exact names)",
        SHORTCUT_TABLE.len()
    );

    let rule = match zbus::MatchRule::builder()
        .msg_type(MessageType::Signal)
        .interface(SHORTCUTS)
        .and_then(|b| b.member("Activated"))
    {
        Ok(builder) => builder.build(),
        Err(e) => {
            tracing::warn!("global shortcuts: cannot watch activations: {e}");
            return;
        }
    };
    let Ok(signals) = MessageIterator::for_match_rule(rule, &conn, None) else {
        tracing::warn!("global shortcuts: cannot subscribe to activations");
        return;
    };
    for message in signals.flatten() {
        let Some((activated_session, id)) = activation(&message) else {
            continue;
        };
        // Another application's shortcuts arrive on the same interface; only this session's are ours to run.
        if activated_session != session {
            continue;
        }
        let Some(command) = command_for(&id) else {
            tracing::warn!("global shortcuts: no command for '{id}'");
            continue;
        };
        if !tx.send(Request::unattended(command)) {
            return; // the driver is gone; the shell is shutting down
        }
    }
}

/// The `(session, shortcut id)` an `Activated` signal names, or `None` if it is not one this can read.
///
/// The whole body has to be named even though only two of it are wanted: the signal is `osta{sv}` — session, id, an event timestamp, and options — and deserializing into a shorter tuple is a signature mismatch, not a truncation. It fails on every activation, silently, which is exactly the kind of bug that looks like "the shortcut does nothing" and sends you hunting in the compositor.
fn activation(message: &zbus::Message) -> Option<(OwnedObjectPath, String)> {
    let body = message.body();
    let (session, id, _timestamp, _options): (
        OwnedObjectPath,
        String,
        u64,
        HashMap<String, OwnedValue>,
    ) = body.deserialize().ok()?;
    Some((session, id))
}

/// Creates a portal session and binds every shortcut in the table to it, answering with the session path.
///
/// The portal's request/response pattern in full: a method returns a `Request` object path and the *real* answer arrives later as a `Response` signal on it. The subscription therefore has to exist before the call is made — subscribing after it returns is a race the portal wins on a warm cache, and the answer is simply missed.
fn register(conn: &Connection) -> Option<OwnedObjectPath> {
    let proxy = Proxy::new(conn, PORTAL, PORTAL_PATH, SHORTCUTS).ok()?;

    let session_token = "hogar_shell_session";
    let mut options: HashMap<&str, Value> = HashMap::new();
    options.insert("handle_token", Value::from("hogar_shell_create"));
    options.insert("session_handle_token", Value::from(session_token));
    let results = call_and_wait(conn, &proxy, "CreateSession", &(options))?;
    let session: OwnedObjectPath = results
        .get("session_handle")
        .and_then(|v| String::try_from(v.clone()).ok())
        .and_then(|s| OwnedObjectPath::try_from(s).ok())?;

    let shortcuts: Vec<(&str, HashMap<&str, Value>)> = SHORTCUT_TABLE
        .iter()
        .map(|s| {
            let mut meta: HashMap<&str, Value> = HashMap::new();
            meta.insert("description", Value::from(s.description));
            (s.id, meta)
        })
        .collect();
    let mut bind_options: HashMap<&str, Value> = HashMap::new();
    bind_options.insert("handle_token", Value::from("hogar_shell_bind"));
    // No parent window: the shell is not an application with one, and the portal treats an empty string as "no parent" rather than as a malformed handle.
    call_and_wait(
        conn,
        &proxy,
        "BindShortcuts",
        &(&session, shortcuts, "", bind_options),
    )?;
    Some(session)
}

/// Calls a portal method and waits for the `Response` signal its `Request` object carries, returning the results map. `None` on any failure, including a portal that answers with a non-zero response code — which is what a user declining the permission dialog looks like.
fn call_and_wait(
    conn: &Connection,
    proxy: &Proxy,
    method: &str,
    args: &(impl serde::Serialize + zbus::zvariant::DynamicType),
) -> Option<HashMap<String, OwnedValue>> {
    let rule = zbus::MatchRule::builder()
        .msg_type(MessageType::Signal)
        .interface(REQUEST)
        .ok()?
        .member("Response")
        .ok()?
        .build();
    let mut responses = MessageIterator::for_match_rule(rule, conn, None).ok()?;

    let request: OwnedObjectPath = proxy.call(method, args).ok()?;

    // Bounded rather than parked forever: a portal that never answers must cost this thread, not the shell.
    let deadline = std::time::Instant::now() + PORTAL_TIMEOUT;
    while std::time::Instant::now() < deadline {
        let Some(Ok(message)) = responses.next() else {
            break;
        };
        if message.header().path().map(|p| p.to_string()) != Some(request.as_str().to_string()) {
            continue;
        }
        let body = message.body();
        let (code, results): (u32, HashMap<String, OwnedValue>) = body.deserialize().ok()?;
        if code != 0 {
            tracing::info!("global shortcuts: portal declined {method} (response {code})");
            return None;
        }
        return Some(results);
    }
    tracing::warn!("global shortcuts: {method} timed out");
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortcut_ids_are_unique_and_bindable() {
        let mut seen: Vec<&str> = Vec::new();
        for shortcut in SHORTCUT_TABLE {
            assert!(
                !seen.contains(&shortcut.id),
                "duplicate shortcut id '{}': the compositor keys on it, so one would shadow the other",
                shortcut.id
            );
            // The compositor addresses a shortcut as `<appid>:<id>`, so an id carrying a colon or a space is one a user cannot write in their config.
            assert!(
                !shortcut.id.contains(':') && !shortcut.id.contains(char::is_whitespace),
                "'{}' cannot appear in a `global, appid:id` bind",
                shortcut.id
            );
            assert!(
                !shortcut.description.is_empty(),
                "'{}' has no description",
                shortcut.id
            );
            seen.push(shortcut.id);
        }
    }

    #[test]
    fn an_unknown_activation_runs_nothing() {
        assert_eq!(command_for("launcher"), Some("launcher toggle"));
        assert_eq!(
            command_for("not-a-shortcut"),
            None,
            "an id the shell never registered must not resolve to some other action"
        );
    }
}
