use std::process::ExitCode;

const EXAMPLES: &str = "
Examples:
  hogar-shell panel toggle clock
  hogar-shell volume step -5
  hogar-shell notifs dnd toggle

Bind them in hyprland.conf:
  bind = SUPER, N, exec, hogar-shell panel toggle notifications
";

/// The usage block, from the same invocation forms the manual's synopsis is built from — a new way to call the binary appears in both or in neither.
fn usage() -> String {
    let mut out = String::from("hogar-shell — a Wayland shell for Hyprland\n\nUsage:\n");
    for (form, help) in hogar_shell::USAGE_FORMS {
        out.push_str(format!("  hogar-shell {form:22}{help}").trim_end());
        out.push('\n');
    }
    out.push_str(EXAMPLES);
    out
}

// glibc hands each thread its own malloc arena, up to 8×ncores of them, each reserving 64 MB of address space and never returning what it fragments; a shell running one thread per service reaches that ceiling and pays for it in RSS, so cap it before any of those threads start. An explicit `MALLOC_ARENA_MAX` still wins.
#[cfg(target_env = "gnu")]
fn cap_malloc_arenas() {
    if std::env::var_os("MALLOC_ARENA_MAX").is_none() {
        unsafe { libc::mallopt(libc::M_ARENA_MAX, 4) };
    }
}

#[cfg(not(target_env = "gnu"))]
fn cap_malloc_arenas() {}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None | Some("run") => {
            cap_malloc_arenas();
            // Held for the whole run: dropping the guard stops the writer thread and flushes what it has.
            let _logging = init_tracing();
            hogar_shell::run();
            ExitCode::SUCCESS
        }
        Some("--help" | "-h") => {
            print!("{}", usage());
            ExitCode::SUCCESS
        }
        Some("--version" | "-V") => {
            println!("hogar-shell {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Some("--list" | "-s") => {
            print!("{}", hogar_shell::ipc_describe());
            ExitCode::SUCCESS
        }
        // Answered here rather than over the socket: the schema is a function of the binary, not of a running shell, and generating the docs on a build machine must not need one started first.
        Some("config") if args.get(1).map(String::as_str) == Some("schema") => {
            match hogar_shell::config_schema(args.get(2).map(String::as_str)) {
                Ok(text) => {
                    print!("{text}");
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("hogar-shell: {e}");
                    ExitCode::FAILURE
                }
            }
        }
        // Same reason as the schema, and one more for `deps`: what a dependency panel is *for* is the machine where something is missing, and "the shell will not start" is exactly the case where there is no shell to ask. Probing is a function of the machine, not of a running process.
        Some("deps" | "man") => match hogar_shell::dispatch_locally(&args.join(" ")) {
            Ok(text) => {
                print!("{text}");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("hogar-shell: {e}");
                ExitCode::FAILURE
            }
        },
        _ => send(&args),
    }
}

/// Forwards a command to the running shell and mirrors its verdict into the exit code, so a keybind or script can tell a refused command from one that worked without parsing the reply.
fn send(args: &[String]) -> ExitCode {
    // `hogar-shell toggle x` is the one shorthand worth having: opening a panel is what a keybind almost always wants, and `panel toggle x` in every hyprland.conf line is noise.
    let request = match args.first().map(String::as_str) {
        Some("toggle") => format!("panel {}", args.join(" ")),
        // The launcher is the one surface people bind a key to before anything else.
        Some("launcher") if args.len() == 1 => "launcher toggle".to_string(),
        _ => args.join(" "),
    };
    match hogar_shell::ipc_call(&request) {
        Ok(reply) => match reply.strip_prefix("ok") {
            Some(payload) => {
                let payload = payload.trim();
                if !payload.is_empty() {
                    println!("{payload}");
                }
                ExitCode::SUCCESS
            }
            None => {
                eprintln!(
                    "hogar-shell: {}",
                    reply.strip_prefix("err ").unwrap_or(&reply)
                );
                ExitCode::FAILURE
            }
        },
        Err(e) => {
            eprintln!("hogar-shell: no running shell to talk to ({e})");
            ExitCode::FAILURE
        }
    }
}

/// Logging that a stalled reader cannot stop, which is the only kind a shell may have.
///
/// The subscriber writes from whichever thread logged, and that includes the driver thread — the one that mounts every surface and paints every frame. Writing straight to stdout ties that thread's progress to whoever is draining the pipe: a dev harness that stopped reading, a terminal paused with Ctrl-S, a logger that died. Once the pipe's 64 KB fill, `write` blocks and never returns, and the shell is parked mid-log holding the stdout lock — bars half-mounted, IPC unanswered, nothing on screen and no message saying why. Hands the bytes to a writer thread instead, dropping them when its queue is full: a reader that stops costs log lines, never frames. `shell quit` exits the process rather than unwinding, so the last lines can go with it; that is the trade this makes deliberately.
fn init_tracing() -> tracing_appender::non_blocking::WorkerGuard {
    let (writer, guard) = tracing_appender::non_blocking::NonBlockingBuilder::default()
        .lossy(true)
        .buffered_lines_limit(4096)
        .finish(std::io::stdout());
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .with_writer(writer)
        .init();
    guard
}
