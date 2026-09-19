//! The shell's command surface: one Unix socket, a flat `<target> <command> [args…]` protocol.
//!
//! Everything the shell can be told to do from outside — a Hyprland keybind, a script, another shell — arrives here. Commands run on the driver thread, the same thread every surface lives on, so a handler can open a panel or publish to a service exactly as a click handler would.
//!
//! The protocol is one request line in and one reply out, so `hogar-shell panel toggle clock` is also `printf 'panel toggle clock\n' | socat - UNIX-CONNECT:$sock`. Replies are prefixed `ok` or `err` so a script can branch without parsing prose. A reply is usually one line but need not be — a census, a palette or a list of monitors is a table — so its end is marked by the shell closing its side, not by a newline.

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

use platform_wayland::EventSender;

use surfaces::shell;
use util::paths;

/// How long the socket thread waits for the driver thread to answer before giving up. Long enough for a command that opens a surface, short enough that a wedged UI thread doesn't hang a script forever.
const REPLY_TIMEOUT: Duration = Duration::from_secs(5);

/// The socket's file name for a compositor instance. Keyed by the Hyprland instance signature so two compositors on one login session get one socket each instead of fighting over a shared name; outside Hyprland the name is still stable, so the CLI can find a shell running under any compositor.
fn socket_name(instance: Option<String>) -> String {
    let instance = instance.filter(|s| !s.is_empty());
    format!("{}.sock", instance.as_deref().unwrap_or("default"))
}

/// The IPC socket: `$XDG_RUNTIME_DIR/hogar-shell/<instance>.sock`.
pub fn socket_path() -> PathBuf {
    paths::runtime_dir().join(socket_name(
        std::env::var("HYPRLAND_INSTANCE_SIGNATURE").ok(),
    ))
}

/// Shuts the shell down: closes every surface, then exits. Surfaces are dropped first so the compositor sees them unmapped rather than the connection simply dying, and the IPC socket is removed on the way out — which is why this lives beside the socket rather than beside the surface registry it empties.
pub(crate) fn request_quit() {
    shell::close_all();
    let _ = std::fs::remove_file(socket_path());
    tracing::info!("shutting down on request");
    // A detached exit lets queued file writes land and the in-flight IPC reply reach the client before the process goes away.
    let _ = std::thread::Builder::new()
        .name("hogar-shell-quit".to_string())
        .spawn(|| {
            util::writer::flush();
            std::thread::sleep(std::time::Duration::from_millis(100));
            std::process::exit(0);
        });
}

/// A shortcut and a `hogar-shell …` invocation must produce the same thing, and only one of the two can see this socket, so the type they share is defined below the pair of them.
pub use services::command::Request;

/// The socket producer: binds, then hands every request line to the driver thread and writes back its reply. Runs on its own thread via `platform_wayland::watch`, so a slow or hostile client never blocks the UI.
pub fn serve(tx: EventSender<Request>) {
    let path = socket_path();
    paths::ensure_dir(path.parent().map(PathBuf::from).unwrap_or_default());
    // A socket left behind by a killed shell would refuse the bind; the caller has already established that nothing is listening on it (see `another_instance_is_running`), so removing it is safe here.
    let _ = std::fs::remove_file(&path);
    let listener = match UnixListener::bind(&path) {
        Ok(listener) => listener,
        Err(e) => {
            tracing::error!("IPC unavailable: cannot bind {}: {e}", path.display());
            return;
        }
    };
    tracing::info!("IPC listening on {}", path.display());

    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        if !handle_client(stream, &tx) {
            break; // the driver is gone (the shell is shutting down)
        }
    }
    let _ = std::fs::remove_file(&path);
}

/// Serves one connection, which may carry several request lines. Returns `false` once the driver stops answering, which is the socket thread's signal to wind itself down.
fn handle_client(stream: UnixStream, tx: &EventSender<Request>) -> bool {
    let Ok(mut out) = stream.try_clone() else {
        return true;
    };
    for line in BufReader::new(stream).lines() {
        let Ok(line) = line else { return true };
        if line.trim().is_empty() {
            continue;
        }
        let (reply_tx, reply_rx) = mpsc::channel();
        if !tx.send(Request::attended(line, reply_tx)) {
            return false;
        }
        let reply = match reply_rx.recv_timeout(REPLY_TIMEOUT) {
            Ok(reply) => reply,
            Err(_) => "err the shell did not answer in time".to_string(),
        };
        if writeln!(out, "{reply}").is_err() {
            return true; // client hung up mid-request; the shell carries on
        }
    }
    true
}

/// Runs a request that arrived over the socket. Called on the driver thread by the `watch` consumer.
pub fn handle(request: Request) {
    let reply = super::commands::dispatch(request.line());
    request.answer(reply);
}

/// Sends one request to a running shell and returns its reply. The client half of the protocol, used by the CLI.
///
/// The write half is closed before reading because a reply is not always one line: a census, a palette or a list of monitors is a table, and `read_line` would take its first row and silently drop the rest — which is what `shell status`, `shell screens`, `shell clients` and `scheme colors` all did. Half-closing tells the shell the request is complete, so it answers and closes its side, and that EOF is what bounds the read. The wire format is untouched: still a request line in, still `ok`/`err` leading the reply.
pub fn call(line: &str) -> std::io::Result<String> {
    call_at(&socket_path(), line)
}

/// [`call`] against a given socket, so the client's framing can be tested against a real one.
fn call_at(path: &std::path::Path, line: &str) -> std::io::Result<String> {
    let mut stream = UnixStream::connect(path)?;
    writeln!(stream, "{line}")?;
    stream.flush()?;
    stream.shutdown(std::net::Shutdown::Write)?;
    let mut reply = String::new();
    BufReader::new(stream).read_to_string(&mut reply)?;
    Ok(reply.trim_end().to_string())
}

/// Whether a shell is already listening on this session's socket. A stale socket file from a killed process fails to connect, so this answers "is one actually running", not "does the file exist".
pub fn another_instance_is_running() -> bool {
    UnixStream::connect(socket_path()).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn socket_name_is_scoped_to_the_compositor_instance() {
        assert_eq!(socket_name(Some("abc123".into())), "abc123.sock");
        assert_eq!(
            socket_name(None),
            "default.sock",
            "outside Hyprland it still has a stable name"
        );
        assert_eq!(
            socket_name(Some(String::new())),
            "default.sock",
            "an empty signature is the same as none, not a bare '.sock'"
        );
    }

    #[test]
    fn a_round_trip_through_the_socket_answers_on_the_driver_thread() {
        // The whole client → socket thread → driver → reply path, minus the driver's real event loop: a stand-in consumer runs `handle` exactly as the `watch` callback does.
        let (tx, rx) = mpsc::channel::<Request>();
        let driver = std::thread::spawn(move || {
            while let Ok(request) = rx.recv() {
                handle(request);
            }
        });

        let (reply_tx, reply_rx) = mpsc::channel();
        tx.send(Request::attended("shell ping", reply_tx)).unwrap();
        assert_eq!(reply_rx.recv_timeout(REPLY_TIMEOUT).unwrap(), "ok pong");

        drop(tx);
        driver.join().unwrap();
    }

    /// A tabular reply has to survive the socket whole.
    ///
    /// It did not: the client read one line and dropped the rest, so `shell status` reported its first row and nothing else — a census that looked like an answer while omitting most of it. Exercised end to end, over a real socket, because the truncation was in the client's framing and no test of the command itself could have seen it.
    #[test]
    fn a_reply_spanning_several_lines_survives_the_socket() {
        let dir = util::paths::isolated_root()
            .expect("a test process resolves under its scratch root")
            .join("ipc");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("multiline.sock");
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).unwrap();

        let body = "ok first\nsecond\nthird";
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut out = stream.try_clone().unwrap();
            // Mirrors `handle_client`: read the request line, write the reply, then let the connection close.
            let mut request = String::new();
            BufReader::new(stream).read_line(&mut request).unwrap();
            writeln!(out, "{body}").unwrap();
        });

        let reply = call_at(&path, "shell status").unwrap();

        server.join().unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(
            reply.trim_end(),
            body,
            "every line of the reply must arrive"
        );
    }
}
