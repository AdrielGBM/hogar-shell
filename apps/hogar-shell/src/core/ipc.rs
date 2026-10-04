//! The shell's command surface: one Unix socket, a flat `<target> <command> [args…]` protocol.
//!
//! Everything the shell can be told to do from outside — a Hyprland keybind, a script, another shell — arrives here. Commands run on the driver thread, the same thread every surface lives on, so a handler can open a panel or publish to a service exactly as a click handler would.
//!
//! The protocol is one request line in and one reply out, so `hogar-shell panel toggle clock` is also `printf 'panel toggle clock\n' | socat - UNIX-CONNECT:$sock`. A request is either a line as a person writes it, or a JSON array of words — what the `hogar-shell …` client sends, its own arguments, so `shell run notify-send "a  b"` reaches the shell as the words the user's shell made of it rather than rejoined with single spaces. No target starts with `[`, so the two cannot be mistaken for each other. Replies are prefixed `ok` or `err` so a script can branch without parsing prose. A reply is usually one line but need not be — a census, a palette or a list of monitors is a table — so its end is marked by the shell closing its side, not by a newline.
//!
//! A command may answer later ([`services::command::defer`]): one whose work belongs off the driver thread — an import reading somebody else's bundle — leaves the driver thread free and the client waiting, up to the time it gave, for the reply it sends once the work is done. That wait happens on a thread of its own, so it holds up neither the driver thread nor another client, and a connection that then sends nothing more is closed once it has been idle for `IDLE_TIMEOUT`, as one that never sends anything is: no client can pin a thread by holding a connection open.

use std::fs::{File, TryLockError};
use std::io::{BufRead, BufReader, Lines, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::{OnceLock, mpsc};
use std::time::Duration;

use platform_wayland::EventSender;
use services::command::Reply;

use surfaces::transient;
use util::paths;

/// How long the socket thread waits for the driver thread to answer before giving up. Long enough for a command that opens a surface, short enough that a wedged UI thread doesn't hang a script forever. A command that defers its answer says itself how long to wait for it.
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

/// Shuts the shell down: drops every open transient, then exits. Transients are dropped first so the compositor sees them unmapped rather than the connection simply dying, and the IPC socket is removed on the way out — which is why this lives beside the socket rather than beside the transient registry it empties.
pub(crate) fn request_quit() {
    // Before the transients go: a layout edit made in the last quarter second is still waiting for the store to settle, and a shell on its way out is not going to settle it (TA-7).
    surfaces::layouts::flush();
    transient::close_all();
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
    // A socket left behind by a killed shell would refuse the bind; this process holds the instance lock (see `claim_instance`), so nothing else is listening on it and removing it is safe.
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
        if !handle_client(stream, &tx, IDLE_TIMEOUT) {
            break; // the driver is gone (the shell is shutting down)
        }
    }
    let _ = std::fs::remove_file(&path);
}

/// How long a connection may sit without sending a request line, or without taking the reply it is sent, before the shell closes it. The client sends its line and half-closes at once, and a script piping lines to the socket sends them as fast as it makes them; a connection idle this long is holding a thread — the listener's, or one waiting out a deferred reply — that every other client is owed.
const IDLE_TIMEOUT: Duration = Duration::from_secs(10);

/// Serves one connection, which may carry several request lines, closing it once it has been idle for `idle`. Returns `false` once the driver stops answering, which is the socket thread's signal to wind itself down.
fn handle_client(stream: UnixStream, tx: &EventSender<Request>, idle: Duration) -> bool {
    let timed = stream
        .set_read_timeout(Some(idle))
        .and_then(|()| stream.set_write_timeout(Some(idle)));
    let Ok(out) = timed.and_then(|()| stream.try_clone()) else {
        return true;
    };
    serve_lines(BufReader::new(stream).lines(), out, tx)
}

/// Answers a connection's request lines in order. A command that answers later ([`services::command::defer`]) is waited for on a thread of its own, which then goes on with the rest of the connection, so one slow answer never holds up another client.
fn serve_lines(
    mut lines: Lines<BufReader<UnixStream>>,
    mut out: UnixStream,
    tx: &EventSender<Request>,
) -> bool {
    while let Some(line) = lines.next() {
        let Ok(line) = line else { return true };
        if line.trim().is_empty() {
            continue;
        }
        let (reply_tx, reply_rx) = mpsc::channel();
        if !tx.send(Request::attended(line, reply_tx)) {
            return false;
        }
        let reply = match reply_rx.recv_timeout(REPLY_TIMEOUT) {
            Ok(Reply::Now(reply)) => reply,
            Ok(Reply::Later(deferred)) => {
                let tx = tx.clone();
                let waiting = std::thread::Builder::new()
                    .name("hogar-shell-ipc-reply".to_string())
                    .spawn(move || {
                        if writeln!(out, "{}", deferred.wait()).is_ok() {
                            serve_lines(lines, out, &tx);
                        }
                    });
                if let Err(why) = waiting {
                    tracing::warn!("cannot wait for a deferred reply: {why}");
                }
                return true;
            }
            Err(_) => "err the shell did not answer in time".to_string(),
        };
        if writeln!(out, "{reply}").is_err() {
            return true; // client hung up mid-request; the shell carries on
        }
    }
    true
}

/// Runs a request that arrived over the socket: a JSON array as the words it holds, anything else as a line. Called on the driver thread by the `watch` consumer.
pub fn handle(request: Request) {
    request.answer_with(|line| match line.trim_start().starts_with('[') {
        true => match serde_json::from_str::<Vec<String>>(line) {
            Ok(words) => super::commands::dispatch_words(&words),
            Err(e) => format!("err a request that starts with `[` is a JSON array of words: {e}"),
        },
        false => super::commands::dispatch(line),
    });
}

/// Sends one request, as its words, to a running shell and returns its reply. The client half of the protocol, used by the CLI.
///
/// The write half is closed before reading because a reply is not always one line: a census, a palette or a list of monitors is a table, and `read_line` would take its first row and silently drop the rest — which is what `shell status`, `shell screens`, `shell clients` and `scheme colors` all did. Half-closing tells the shell the request is complete, so it answers and closes its side, and that EOF is what bounds the read.
pub fn call(words: &[String]) -> std::io::Result<String> {
    call_at(&socket_path(), words)
}

/// [`call`] against a given socket, so the client's framing can be tested against a real one.
fn call_at(path: &std::path::Path, words: &[String]) -> std::io::Result<String> {
    let mut stream = UnixStream::connect(path)?;
    writeln!(stream, "{}", serde_json::to_string(words)?)?;
    stream.flush()?;
    stream.shutdown(std::net::Shutdown::Write)?;
    let mut reply = String::new();
    BufReader::new(stream).read_to_string(&mut reply)?;
    Ok(reply.trim_end().to_string())
}

/// Makes this process the one shell for its compositor instance, or answers `false` when another already is.
///
/// Asking whether the socket answers and binding it later left a window in which two shells started together — a hot-reloading dev watcher restarting beside another — both saw nothing listening and both came up, reserving every edge twice. An exclusive `flock` is taken atomically and released by the kernel when the process dies, so a killed shell leaves nothing that refuses the next one.
pub fn claim_instance() -> bool {
    static CLAIM: OnceLock<File> = OnceLock::new();
    if CLAIM.get().is_some() {
        return true;
    }
    match claim_at(&socket_path().with_extension("lock")) {
        Ok(Some(lock)) => CLAIM.set(lock).is_ok(),
        Ok(None) => false,
        Err(e) => {
            tracing::warn!("cannot take the single-instance lock: {e}; starting without it");
            true
        }
    }
}

fn claim_at(path: &Path) -> std::io::Result<Option<File>> {
    paths::ensure_dir(path.parent().map(PathBuf::from).unwrap_or_default());
    let lock = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)?;
    match lock.try_lock() {
        Ok(()) => Ok(Some(lock)),
        Err(TryLockError::WouldBlock) => Ok(None),
        Err(TryLockError::Error(e)) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_shell_cannot_claim_the_instance_until_the_first_lets_go() {
        let path = paths::runtime_dir().join("claim-test.lock");
        let first = claim_at(&path).unwrap().expect("nothing holds it yet");
        assert!(
            claim_at(&path).unwrap().is_none(),
            "a second claim while the first is held is refused"
        );
        drop(first);
        assert!(
            claim_at(&path).unwrap().is_some(),
            "the lock goes with the process that held it, so a killed shell never blocks the next"
        );
    }

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

        let ask = |frame: &str| {
            let (reply_tx, reply_rx) = mpsc::channel();
            tx.send(Request::attended(frame, reply_tx)).unwrap();
            match reply_rx.recv_timeout(REPLY_TIMEOUT).unwrap() {
                Reply::Now(reply) => reply,
                Reply::Later(_) => panic!("`{frame}` answers at once"),
            }
        };
        assert_eq!(ask("shell ping"), "ok pong");
        assert_eq!(
            ask(r#"["shell", "ping"]"#),
            "ok pong",
            "the same request as words"
        );
        assert!(
            ask(r#"["shell", "ping""#).starts_with("err a request that starts with `[`"),
            "a broken array is refused, not read as a line"
        );

        drop(tx);
        driver.join().unwrap();
    }

    /// A command that answers later keeps its client waiting for the real answer, over a real socket, while another client is answered in the meantime — the wait holds up neither the driver thread nor the socket.
    #[test]
    fn a_deferred_reply_reaches_its_client_without_holding_up_another() {
        let dir = util::paths::isolated_root()
            .expect("a test process resolves under its scratch root")
            .join("ipc");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("deferred.sock");
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).unwrap();
        let (tx, requests) = platform_wayland::detached::<Request>();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                if !handle_client(stream, &tx, REPLY_TIMEOUT) {
                    return;
                }
            }
        });
        let (promises, promised) = mpsc::channel();
        std::thread::spawn(move || {
            loop {
                let Some(request) = requests.try_recv() else {
                    std::thread::sleep(Duration::from_millis(2));
                    continue;
                };
                request.answer_with(|line| match line.contains("slow") {
                    true => {
                        let later = services::command::defer(Duration::from_secs(30))
                            .expect("a client waits on it");
                        promises.send(later).unwrap();
                        "ok still working".to_string()
                    }
                    false => "ok fast".to_string(),
                });
            }
        });

        let slow_path = path.clone();
        let slow = std::thread::spawn(move || call_at(&slow_path, &["slow".to_string()]));
        let later = promised
            .recv_timeout(REPLY_TIMEOUT)
            .expect("the slow request reached the driver");
        assert_eq!(
            call_at(&path, &["fast".to_string()]).unwrap(),
            "ok fast",
            "another client is answered while the first waits"
        );
        assert!(later.answer("ok done\nwith a table".to_string()));
        assert_eq!(slow.join().unwrap().unwrap(), "ok done\nwith a table");
        let _ = std::fs::remove_file(&path);
    }

    /// A client cannot pin a thread: one that connects and never sends is closed once idle, so the listener goes on to the next client, and one that keeps its connection open after a deferred reply is closed too, ending the thread that waited it out.
    #[test]
    fn an_idle_connection_is_closed_and_pins_no_thread() {
        let dir = util::paths::isolated_root()
            .expect("a test process resolves under its scratch root")
            .join("ipc");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("idle.sock");
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).unwrap();
        let (tx, requests) = platform_wayland::detached::<Request>();
        let idle = Duration::from_millis(200);
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                if !handle_client(stream, &tx, idle) {
                    return;
                }
            }
        });
        std::thread::spawn(move || {
            loop {
                let Some(request) = requests.try_recv() else {
                    std::thread::sleep(Duration::from_millis(2));
                    continue;
                };
                request.answer_with(|line| match line.contains("slow") {
                    true => {
                        let later = services::command::defer(Duration::from_secs(30))
                            .expect("a client waits on it");
                        std::thread::spawn(move || {
                            std::thread::sleep(Duration::from_millis(20));
                            later.answer("ok done".to_string());
                        });
                        "ok still working".to_string()
                    }
                    false => "ok fast".to_string(),
                });
            }
        });

        let silent = UnixStream::connect(&path).unwrap();
        assert_eq!(
            call_at(&path, &["fast".to_string()]).unwrap(),
            "ok fast",
            "the listener is free again once the silent client has been idle"
        );
        let mut rest = String::new();
        BufReader::new(&silent).read_to_string(&mut rest).unwrap();
        assert!(
            rest.is_empty(),
            "the silent connection is closed with nothing said"
        );

        let mut waiting = UnixStream::connect(&path).unwrap();
        writeln!(waiting, "{}", serde_json::to_string(&["slow"]).unwrap()).unwrap();
        waiting
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut reply = String::new();
        BufReader::new(&waiting)
            .read_to_string(&mut reply)
            .expect("the shell closes its side once the connection idles after the reply");
        assert_eq!(reply, "ok done\n");
        let _ = std::fs::remove_file(&path);
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
            request
        });

        let words = ["shell", "run", "notify-send", "a  b"].map(String::from);
        let reply = call_at(&path, &words).unwrap();

        let request = server.join().unwrap();
        assert_eq!(
            serde_json::from_str::<Vec<String>>(&request).unwrap(),
            words,
            "the client sends its words, each one whole"
        );
        let _ = std::fs::remove_file(&path);
        assert_eq!(
            reply.trim_end(),
            body,
            "every line of the reply must arrive"
        );
    }
}
