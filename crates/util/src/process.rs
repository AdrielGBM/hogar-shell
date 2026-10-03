//! Running another program and waiting for it, without ever waiting for ever.
//!
//! `Command::output` blocks until the child exits, which for a shell means one wedged helper parks the thread that was calling it — and every question queued behind it. Three callers wanted the same deadline (`qalc`, `ddcutil` twice over), so the wait lives here once.
//!
//! Output is read on the thread that asked for it, by polling the child's pipes together with a descriptor that turns readable when it exits. There is no thread per pipe, so nothing a command leaves behind holding a pipe can strand one.
//!
//! Only ever called off the UI thread: even with a deadline, this is a process start.

use std::fmt;
use std::io::{self, Read};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::process::CommandExt;
use std::process::{Child, ChildStderr, ChildStdout, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

/// How long a wait that cannot poll sleeps before looking again.
const POLL: Duration = Duration::from_millis(20);

/// The one place in the tree a child process is constructed.
///
/// Everything else goes through [`deps::command`](crate::deps::command), which takes a declared dependency rather than a name — so the list of what this shell reaches for cannot be incomplete. This raw form is for the two things that are *not* dependencies: a command the **user** wrote (a launcher action, a scheme hook, the configured annotator or `howdy` line), and the helpers in this module. `deps::tests::nothing_reaches_outside_this_process_without_a_row` is what keeps that true.
pub fn command(program: &str) -> Command {
    Command::new(program)
}

/// `line` the way every line a user wrote is run: through `sh -c`.
fn sh(line: &str) -> Command {
    let mut command = command("sh");
    command.args(["-c", line]);
    command
}

/// Launches `command` through a shell and forgets about it — a session of its own so it survives the shell exiting, and every stream nulled so it can neither block on a pipe nor write over the shell's own output.
///
/// The opposite trade to [`output`]: nothing here reads a result, so the child is disowned rather than waited on.
///
/// This used to spawn `setsid --fork`, which made launching anything at all depend on a program from util-linux — and not gracefully: with `setsid` absent the spawn failed and the application never started, which is not something a user could have diagnosed from the shell. The double fork below is what `--fork` was doing, so the intermediate child is still reaped here and the application is still reparented to init.
pub fn run_detached(line: String) {
    let _ = std::thread::Builder::new()
        .name("hogar-shell-launch".to_string())
        .spawn(move || {
            let mut child = sh(&line);
            child
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            // SAFETY: runs in the forked child before exec, so only async-signal-safe calls are allowed; `fork`, `_exit` and `setsid` all are, and nothing here allocates or takes a lock.
            unsafe {
                child.pre_exec(|| {
                    match libc::fork() {
                        -1 => return Err(std::io::Error::last_os_error()),
                        // The intermediate leaves at once, so whoever spawned it has something to reap and the process below is orphaned onto init rather than held by a shell that may exit first.
                        0 => {}
                        _ => libc::_exit(0),
                    }
                    // Leading a session of its own is what detaches it from the shell's terminal and process group, so a signal sent to the shell is not delivered to everything it ever launched.
                    if libc::setsid() == -1 {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
            match child.status() {
                Ok(_) => {}
                Err(e) => tracing::warn!("launching `{line}`: {e}"),
            }
        });
}

/// Runs `program args…` and returns its standard output.
///
/// `None` covers every way this can fail to produce an answer — the program is not installed, it exited non-zero, it said more than a caller reading a value could want, or it outstayed `timeout` and was killed — because a caller reading a value has the same fallback for all of them. What it must never do is return late.
pub fn output(program: &str, args: &[&str], timeout: Duration) -> Option<String> {
    let mut child = command(program);
    child.args(args);
    let limits = Limits {
        timeout,
        max_line: OUTPUT_CAP,
        max_run: OUTPUT_CAP,
    };
    match capture(child, limits) {
        Outcome::Ok(text) => Some(text),
        Outcome::TimedOut => {
            tracing::warn!("{program} did not answer within {timeout:?}");
            None
        }
        _ => None,
    }
}

/// The most [`output`] will read: its callers ask for a line or two, so a program that says more than this is not answering the question.
const OUTPUT_CAP: usize = 1 << 20;

/// How much of a failing command's standard error is kept, enough to say why and no more.
const STDERR_EXCERPT: usize = 2048;

const READ_CHUNK: usize = 8192;

/// How long output is still read once the command itself has exited.
///
/// Whatever holds a pipe open after that is something the command left behind — `setsid foo &` keeps it in a session of its own, out of reach of the group kill — and reading on would be waiting for that instead of for the command.
const DRAIN_GRACE: Duration = Duration::from_millis(200);

/// What a user's command line is allowed to cost. Parameters rather than constants: the shell's config owns the defaults.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// How long a one-shot run may take before it is killed.
    pub timeout: Duration,
    /// The longest a single line may be, in bytes, newline excluded.
    pub max_line: usize,
    /// The most a one-shot run may print, in bytes.
    pub max_run: usize,
}

/// How a command ended on its own: its status, and the start of what it said on standard error.
#[derive(Debug)]
pub struct Exit {
    pub status: ExitStatus,
    pub stderr: String,
}

impl fmt::Display for Exit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.stderr.is_empty() {
            true => write!(f, "{}", self.status),
            false => write!(f, "{}: {}", self.status, self.stderr),
        }
    }
}

/// How a one-shot run ended.
#[derive(Debug)]
pub enum Outcome {
    /// Exited zero; everything it wrote to standard output, lossily decoded.
    Ok(String),
    /// Outstayed [`Limits::timeout`] and was killed.
    TimedOut,
    /// Wrote a line longer than [`Limits::max_line`] or more than [`Limits::max_run`] in all, and was killed.
    TooLong,
    /// Exited non-zero.
    Failed(Exit),
    /// The process could not be started.
    Spawn(io::Error),
    /// The process ran, but its exit status could not be collected.
    Wait(io::Error),
}

/// Runs `line` through `sh -c` and waits for it within `limits`.
///
/// Output is read *while* the child runs, so one that prints more than a pipe holds is read rather than left blocked on a full pipe until the deadline. The child leads a process group of its own, and a kill takes the group — a line such as `a | b` has no single process whose death ends it. A run ends with its group: whatever it started in the background goes when it exits.
///
/// Only ever called off the UI thread.
pub fn run_line(line: &str, limits: Limits) -> Outcome {
    capture(sh(line), limits)
}

/// How a [`stream_line`] ended, delivered last and only if its owner did not stop it first.
#[derive(Debug)]
pub enum StreamEnd {
    /// The command exited, successfully or not. A listener is expected to run until stopped, so the owner usually treats either as a failure to restart with backoff.
    Exited(Exit),
    /// A line outgrew the cap and the command was killed.
    LineTooLong,
    /// The command ran, but its exit status could not be collected.
    Wait(io::Error),
}

/// What a [`stream_line`] delivers to its callback, in order, on the thread that reads the command.
#[derive(Debug)]
pub enum StreamEvent {
    Line(String),
    Ended(StreamEnd),
}

/// A running [`stream_line`]. Dropping it stops the command.
pub struct Stream {
    group: Arc<Group>,
}

impl Stream {
    /// Kills the command. The callback is not called again once this returns — not for a line already read, and not for the end.
    pub fn stop(&self) {
        self.group.stop();
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        self.group.stop();
    }
}

/// Runs `line` through `sh -c` for as long as it keeps running, calling `on_event` with each line it prints and once more when it ends.
///
/// Only the line cap applies: a line longer than `max_line` bytes kills the command. A timeout and a per-run cap are about a run that is meant to finish, and this one is not.
///
/// `on_event` runs on the reading thread while holding off [`Stream::stop`], which is what lets a stop promise silence once it returns; so `on_event` must never wait on whoever stops the stream.
pub fn stream_line(
    line: &str,
    max_line: usize,
    mut on_event: impl FnMut(StreamEvent) + Send + 'static,
) -> io::Result<Stream> {
    let mut running = Running::spawn(sh(line))?;
    let group = Arc::clone(&running.group);
    std::thread::Builder::new()
        .name("hogar-shell-listen".to_string())
        .spawn(move || {
            let group = Arc::clone(&running.group);
            let mut lines = Lines::new(max_line);
            let mut stderr = Vec::new();
            let drained = running.drain(None, &mut stderr, |chunk| {
                group.deliver(|| lines.feed(chunk, &mut |line| on_event(StreamEvent::Line(line))))
            });
            let too_long = drained == Drained::Refused && !group.stopped();
            if too_long {
                group.kill();
            } else {
                group.deliver(|| {
                    lines.finish(&mut |line| on_event(StreamEvent::Line(line)));
                    true
                });
            }
            let end = match running.wait() {
                _ if too_long => StreamEnd::LineTooLong,
                Ok(status) => StreamEnd::Exited(Exit {
                    status,
                    stderr: excerpt(&stderr),
                }),
                Err(e) => StreamEnd::Wait(e),
            };
            group.deliver(|| {
                on_event(StreamEvent::Ended(end));
                true
            });
        })?;
    Ok(Stream { group })
}

fn capture(command: Command, limits: Limits) -> Outcome {
    let mut running = match Running::spawn(command) {
        Ok(running) => running,
        Err(e) => return Outcome::Spawn(e),
    };
    let deadline = Instant::now() + limits.timeout;
    let mut text = Vec::new();
    let mut stderr = Vec::new();
    let mut lines = Lines::new(limits.max_line);
    let drained = running.drain(Some(deadline), &mut stderr, |chunk| {
        text.extend_from_slice(chunk);
        text.len() <= limits.max_run && lines.feed(chunk, &mut |_| {})
    });
    match drained {
        Drained::Finished => {}
        Drained::Deadline => return Outcome::TimedOut,
        Drained::Refused => return Outcome::TooLong,
    }
    if !running.exited_by(deadline) {
        return Outcome::TimedOut;
    }
    match running.wait() {
        Ok(status) if status.success() => Outcome::Ok(String::from_utf8_lossy(&text).into_owned()),
        Ok(status) => Outcome::Failed(Exit {
            status,
            stderr: excerpt(&stderr),
        }),
        Err(e) => Outcome::Wait(e),
    }
}

#[derive(PartialEq, Eq)]
enum Drained {
    Finished,
    Deadline,
    Refused,
}

fn excerpt(stderr: &[u8]) -> String {
    String::from_utf8_lossy(stderr).trim().to_string()
}

/// Splits a byte stream into lines, refusing one longer than `max`.
struct Lines {
    pending: Vec<u8>,
    max: usize,
}

impl Lines {
    fn new(max: usize) -> Self {
        Self {
            pending: Vec::new(),
            max,
        }
    }

    /// Emits each completed line. `false` when a line outgrew the cap.
    fn feed(&mut self, chunk: &[u8], emit: &mut impl FnMut(String)) -> bool {
        for &byte in chunk {
            if byte == b'\n' {
                self.finish(emit);
            } else if self.pending.len() == self.max {
                return false;
            } else {
                self.pending.push(byte);
            }
        }
        true
    }

    fn finish(&mut self, emit: &mut impl FnMut(String)) {
        if self.pending.last() == Some(&b'\r') {
            self.pending.pop();
        }
        if !self.pending.is_empty() {
            emit(String::from_utf8_lossy(&self.pending).into_owned());
        }
        self.pending.clear();
    }
}

/// A started command as the thread reading it holds it: the child, our ends of its pipes, and a descriptor that turns readable when it exits. Dropping it before [`Running::wait`] kills the group and collects the child.
struct Running {
    child: Child,
    out: Option<ChildStdout>,
    err: Option<ChildStderr>,
    exited: OwnedFd,
    group: Arc<Group>,
}

impl Running {
    fn spawn(mut command: Command) -> io::Result<Self> {
        let mut child = command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .spawn()?;
        let leader = child.id() as libc::pid_t;
        let exited = match exit_descriptor(leader) {
            Ok(exited) => exited,
            Err(e) => {
                kill_group(leader);
                let _ = child.wait();
                return Err(e);
            }
        };
        Ok(Self {
            out: child.stdout.take(),
            err: child.stderr.take(),
            child,
            exited,
            group: Arc::new(Group {
                leader,
                reaped: Mutex::new(false),
                stopped: Mutex::new(false),
            }),
        })
    }

    /// Reads standard output into `on_out` and keeps standard error, up to the excerpt, in `stderr` — until both pipes close, `on_out` answers `false` to refuse a chunk, `deadline` passes, or the command has been gone for [`DRAIN_GRACE`]. Our ends of the pipes are closed when it returns.
    fn drain(
        &mut self,
        deadline: Option<Instant>,
        stderr: &mut Vec<u8>,
        mut on_out: impl FnMut(&[u8]) -> bool,
    ) -> Drained {
        let mut buffer = [0u8; READ_CHUNK];
        let mut cutoff: Option<Instant> = None;
        let drained = loop {
            if self.out.is_none() && self.err.is_none() {
                break Drained::Finished;
            }
            let now = Instant::now();
            if deadline.is_some_and(|deadline| now >= deadline) {
                break Drained::Deadline;
            }
            if cutoff.is_some_and(|cutoff| now >= cutoff) {
                break Drained::Finished;
            }
            let until = deadline.into_iter().chain(cutoff).min();
            let mut fds = [
                watched(self.out.as_ref()),
                watched(self.err.as_ref()),
                watched(cutoff.is_none().then_some(&self.exited)),
            ];
            poll(
                &mut fds,
                until.map(|until| until.saturating_duration_since(now)),
            );
            if is_ready(&fds[0]) {
                let chunk = read_some(&mut self.out, &mut buffer);
                if !chunk.is_empty() && !on_out(chunk) {
                    break Drained::Refused;
                }
            }
            if is_ready(&fds[1]) {
                let chunk = read_some(&mut self.err, &mut buffer);
                let room = STDERR_EXCERPT.saturating_sub(stderr.len());
                stderr.extend_from_slice(&chunk[..chunk.len().min(room)]);
            }
            if is_ready(&fds[2]) {
                cutoff = Some(Instant::now() + DRAIN_GRACE);
            }
        };
        self.out = None;
        self.err = None;
        drained
    }

    /// Whether the command has exited by `deadline`, waiting until then for it to.
    fn exited_by(&self, deadline: Instant) -> bool {
        loop {
            let now = Instant::now();
            if self.exits_within(Some(deadline.saturating_duration_since(now))) {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
        }
    }

    /// Waits for the command to exit and collects its status. Whatever it left running in its group goes with it: until the leader is collected it still holds the group's number, so the kill cannot reach anything else.
    fn wait(&mut self) -> io::Result<ExitStatus> {
        while !self.exits_within(None) {}
        let mut reaped = lock(&self.group.reaped);
        kill_group(self.group.leader);
        *reaped = true;
        self.child.wait()
    }

    fn exits_within(&self, timeout: Option<Duration>) -> bool {
        let mut fds = [watched(Some(&self.exited))];
        poll(&mut fds, timeout);
        is_ready(&fds[0])
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        let mut reaped = lock(&self.group.reaped);
        if !*reaped {
            kill_group(self.group.leader);
            let _ = self.child.wait();
            *reaped = true;
        }
    }
}

/// What anyone holding a running command may do to it: kill it, and stop hearing from it.
///
/// The command leads a process group of its own, and every kill is of the group: `sh -c 'a | b'` runs the pipeline as children of the shell, and killing only the shell would leave them running. A kill and the collecting of the leader take the same lock, so a kill can never be aimed at a group number the system has already handed to something else.
struct Group {
    leader: libc::pid_t,
    reaped: Mutex<bool>,
    stopped: Mutex<bool>,
}

impl Group {
    fn kill(&self) {
        let reaped = lock(&self.reaped);
        if !*reaped {
            kill_group(self.leader);
        }
    }

    /// Kills the command for its owner, after which [`Group::deliver`] delivers nothing.
    fn stop(&self) {
        *lock(&self.stopped) = true;
        self.kill();
    }

    fn stopped(&self) -> bool {
        *lock(&self.stopped)
    }

    /// Runs `deliver` unless the owner has stopped the command, holding [`Group::stop`] off until it returns. `false` when stopped, or when `deliver` answers it.
    fn deliver(&self, deliver: impl FnOnce() -> bool) -> bool {
        let stopped = lock(&self.stopped);
        !*stopped && deliver()
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A descriptor that turns readable once the process `pid` exits, without collecting it.
fn exit_descriptor(pid: libc::pid_t) -> io::Result<OwnedFd> {
    // SAFETY: `pidfd_open` takes a pid and no flags and answers a new close-on-exec descriptor or -1; the child has not been collected, so `pid` still names it.
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the descriptor was just opened and nothing else owns it.
    Ok(unsafe { OwnedFd::from_raw_fd(fd as RawFd) })
}

fn watched(fd: Option<&impl AsRawFd>) -> libc::pollfd {
    libc::pollfd {
        fd: fd.map_or(-1, AsRawFd::as_raw_fd),
        events: libc::POLLIN,
        revents: 0,
    }
}

fn is_ready(fd: &libc::pollfd) -> bool {
    fd.revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR) != 0
}

/// Waits until one of `fds` is ready or `timeout` passes, for ever without one. A negative descriptor is skipped.
fn poll(fds: &mut [libc::pollfd], timeout: Option<Duration>) {
    let millis = timeout.map_or(-1, |timeout| {
        timeout
            .as_micros()
            .div_ceil(1000)
            .min(libc::c_int::MAX as u128) as libc::c_int
    });
    // SAFETY: `fds` is a live, exclusively borrowed slice of `pollfd`, and its length is the one passed.
    let result = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, millis) };
    if result < 0 && io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
        std::thread::sleep(timeout.map_or(POLL, |timeout| timeout.min(POLL)));
    }
}

/// What one read of a ready pipe gave, closing it at its end.
fn read_some<'a>(pipe: &mut Option<impl Read>, buffer: &'a mut [u8]) -> &'a [u8] {
    let Some(reading) = pipe else {
        return &[];
    };
    match reading.read(buffer) {
        Ok(0) => {
            *pipe = None;
            &[]
        }
        Ok(n) => &buffer[..n],
        Err(e) if e.kind() == io::ErrorKind::Interrupted => &[],
        Err(_) => {
            *pipe = None;
            &[]
        }
    }
}

fn kill_group(leader: libc::pid_t) {
    // SAFETY: `killpg` only sends a signal; every caller holds the lock that keeps the leader uncollected, so the group number is still this command's.
    unsafe {
        libc::killpg(leader, libc::SIGKILL);
    }
}

/// Whether `program` is on the `PATH` at all, asked by running it with `args` (usually a `--version`).
///
/// A missing helper is the common case on a machine that simply does not have it, and the answer decides whether a service bothers to start — so it is worth one cheap call rather than a failure per reading.
pub fn available(program: &str, args: &[&str], timeout: Duration) -> bool {
    output(program, args, timeout).is_some()
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::{self, RecvTimeoutError};

    use super::*;

    #[test]
    fn a_program_that_is_not_there_answers_none_rather_than_waiting() {
        assert_eq!(
            output(
                "hogar-shell-no-such-program-9e3f",
                &[],
                Duration::from_secs(1)
            ),
            None
        );
        assert!(!available(
            "hogar-shell-no-such-program-9e3f",
            &["--version"],
            Duration::from_secs(1)
        ));
    }

    #[test]
    fn stdout_comes_back_and_a_failure_does_not() {
        assert_eq!(
            output("true", &[], Duration::from_secs(2)).as_deref(),
            Some("")
        );
        assert_eq!(output("false", &[], Duration::from_secs(2)), None);
        assert_eq!(
            output("echo", &["hello"], Duration::from_secs(2)).as_deref(),
            Some("hello\n")
        );
    }

    // The guard that keeps `deps::ALL` complete lives beside the list it protects, as `deps::tests::nothing_reaches_outside_this_process_without_a_row`: it is what forbids constructing a child anywhere but this module.

    fn limits(timeout_ms: u64) -> Limits {
        Limits {
            timeout: Duration::from_millis(timeout_ms),
            max_line: 1000,
            max_run: 1 << 20,
        }
    }

    #[test]
    fn a_line_that_prints_comes_back_whole() {
        let outcome = run_line("printf 'a\\nb\\n'", limits(5000));
        assert!(
            matches!(&outcome, Outcome::Ok(text) if text == "a\nb\n"),
            "{outcome:?}"
        );
    }

    #[test]
    fn a_failing_line_reports_its_status_and_what_it_said() {
        let outcome = run_line("echo oops >&2; exit 3", limits(5000));
        let Outcome::Failed(exit) = outcome else {
            panic!("{outcome:?}");
        };
        assert_eq!(exit.status.code(), Some(3));
        assert_eq!(exit.stderr, "oops");
        assert_eq!(exit.to_string(), "exit status: 3: oops");
    }

    #[test]
    fn a_line_that_runs_past_the_deadline_is_killed_with_its_pipeline() {
        let started = Instant::now();
        let outcome = run_line("sleep 10 | sleep 10", limits(150));
        assert!(matches!(outcome, Outcome::TimedOut), "{outcome:?}");
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    /// A child that fills the pipe has to be read as it goes: waiting for it to exit first would block it, and the run would be reported as a timeout instead of as too much output.
    #[test]
    fn more_output_than_the_run_cap_is_cut_off_rather_than_buffered() {
        let started = Instant::now();
        let outcome = run_line(
            "yes | head -c 2000000",
            Limits {
                timeout: Duration::from_secs(10),
                max_line: 1000,
                max_run: 1 << 20,
            },
        );
        assert!(matches!(outcome, Outcome::TooLong), "{outcome:?}");
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn a_run_that_fits_the_cap_but_fills_the_pipe_still_finishes() {
        let outcome = run_line(
            "yes | head -c 500000",
            Limits {
                timeout: Duration::from_secs(10),
                max_line: 1000,
                max_run: 1 << 20,
            },
        );
        assert!(
            matches!(&outcome, Outcome::Ok(text) if text.len() == 500_000),
            "{outcome:?}"
        );
    }

    #[test]
    fn one_line_longer_than_the_line_cap_is_too_long_however_short_the_run() {
        let outcome = run_line("head -c 5000 /dev/zero | tr '\\0' a", limits(5000));
        assert!(matches!(outcome, Outcome::TooLong), "{outcome:?}");
    }

    #[test]
    fn a_line_that_cannot_start_is_a_spawn_error_or_a_failure_never_a_hang() {
        let outcome = run_line("hogar-shell-no-such-program-9e3f", limits(5000));
        assert!(matches!(outcome, Outcome::Failed { .. }), "{outcome:?}");
    }

    fn process_exists(pid: i32) -> bool {
        // SAFETY: signal 0 only checks that the process exists.
        unsafe { libc::kill(pid, 0) == 0 }
    }

    fn eventually(what: &str, check: impl Fn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if check() {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("timed out waiting for {what}");
    }

    /// A pid file of this test's own, which a command writes the pid of what it leaves behind to.
    fn pid_file(name: &str) -> std::path::PathBuf {
        let path =
            std::env::temp_dir().join(format!("hogar-shell-process-{name}-{}", std::process::id()));
        let _ = std::fs::remove_file(&path);
        path
    }

    fn read_pid(path: &std::path::Path) -> i32 {
        let read = || {
            std::fs::read_to_string(path)
                .ok()?
                .trim()
                .parse::<i32>()
                .ok()
        };
        eventually("the pid file", || read().is_some());
        let pid = read().expect("a pid");
        let _ = std::fs::remove_file(path);
        pid
    }

    fn kill(pid: i32) {
        // SAFETY: only signals the process this test started.
        unsafe {
            libc::kill(pid, libc::SIGKILL);
        }
    }

    /// `setsid foo &` leaves a process outside the group, out of reach of the group kill, holding the run's pipes: the run is over when the command is, not when that process lets go of them.
    #[test]
    fn a_process_left_holding_the_pipes_outside_the_group_does_not_hold_the_run() {
        let left = pid_file("setsid-run");
        let started = Instant::now();
        let outcome = run_line(
            &format!(
                "setsid sh -c 'echo $$ > {}; exec sleep 30' & echo done",
                left.display()
            ),
            limits(5000),
        );
        let pid = read_pid(&left);
        kill(pid);
        assert!(
            matches!(&outcome, Outcome::Ok(text) if text == "done\n"),
            "{outcome:?}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "the run waited {:?} on a process it does not own",
            started.elapsed()
        );
    }

    #[test]
    fn a_stream_left_holding_its_pipes_still_ends_when_its_command_does() {
        let left = pid_file("setsid-stream");
        let events = collect_stream(
            &format!(
                "setsid sh -c 'echo $$ > {}; exec sleep 30' & echo done",
                left.display()
            ),
            1000,
        );
        kill(read_pid(&left));
        assert_eq!(events, ["done", "ended: exited"]);
    }

    /// What a run starts in its own group is part of the run, and goes when it ends.
    #[test]
    fn what_a_run_leaves_running_in_its_group_goes_with_it() {
        let outcome = run_line("sleep 30 >/dev/null 2>&1 & echo $!", limits(5000));
        let Outcome::Ok(text) = outcome else {
            panic!("{outcome:?}");
        };
        let pid: i32 = text.trim().parse().expect("a pid");
        eventually("the background job to be gone", || !process_exists(pid));
    }

    fn collect_stream(line: &str, max_line: usize) -> Vec<String> {
        let (tx, rx) = mpsc::channel();
        let _stream = stream_line(line, max_line, move |event| {
            let _ = tx.send(match event {
                StreamEvent::Line(line) => line,
                StreamEvent::Ended(end) => format!("ended: {}", stream_end_name(&end)),
            });
        })
        .unwrap();
        let mut events = Vec::new();
        while let Ok(event) = rx.recv_timeout(Duration::from_secs(5)) {
            let ended = event.starts_with("ended");
            events.push(event);
            if ended {
                break;
            }
        }
        events
    }

    fn stream_end_name(end: &StreamEnd) -> &'static str {
        match end {
            StreamEnd::Exited(_) => "exited",
            StreamEnd::LineTooLong => "too long",
            StreamEnd::Wait(_) => "wait",
        }
    }

    #[test]
    fn a_stream_yields_one_event_per_line_and_then_its_end() {
        assert_eq!(
            collect_stream("printf 'a\\nb\\nc'", 1000),
            ["a", "b", "c", "ended: exited"]
        );
    }

    #[test]
    fn a_stream_line_over_the_cap_ends_the_stream() {
        assert_eq!(
            collect_stream("echo ok; head -c 5000 /dev/zero | tr '\\0' a", 1000),
            ["ok", "ended: too long"]
        );
    }

    /// A command that never stops printing is the hardest case for the promise: a chunk is always mid-delivery when the stop lands. The callback is dropped only when the reading thread is done, so waiting for its channel to close waits for every event there will ever be.
    #[test]
    fn stopping_a_stream_kills_the_command_and_nothing_is_delivered_after() {
        let stopped = Arc::new(AtomicBool::new(false));
        let late = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel();
        let stream = {
            let (stopped, late) = (Arc::clone(&stopped), Arc::clone(&late));
            stream_line("echo $$; while :; do echo tick; done", 1000, move |event| {
                if stopped.load(Ordering::SeqCst) {
                    late.store(true, Ordering::SeqCst);
                }
                if let StreamEvent::Line(line) = event {
                    let _ = tx.send(line);
                }
            })
            .unwrap()
        };
        let pid: i32 = rx
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(5)).as_deref(),
            Ok("tick")
        );

        stream.stop();
        stopped.store(true, Ordering::SeqCst);
        loop {
            match rx.recv_timeout(Duration::from_secs(5)) {
                Ok(_) => {}
                Err(RecvTimeoutError::Disconnected) => break,
                Err(RecvTimeoutError::Timeout) => panic!("the reading thread never finished"),
            }
        }
        assert!(
            !late.load(Ordering::SeqCst),
            "an event was delivered after stop returned"
        );
        assert!(!process_exists(pid), "the command was killed and collected");
    }

    #[test]
    fn dropping_a_stream_kills_the_command() {
        let (tx, rx) = mpsc::channel();
        let stream = stream_line("echo $$; exec sleep 30", 1000, move |event| {
            if let StreamEvent::Line(line) = event {
                let _ = tx.send(line);
            }
        })
        .unwrap();
        let pid: i32 = rx
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .parse()
            .unwrap();
        drop(stream);
        eventually("the command to die", || !process_exists(pid));
    }

    /// The reason this module exists: a child that never exits must not hold the thread.
    #[test]
    fn a_child_that_will_not_finish_is_killed_at_the_deadline() {
        let started = Instant::now();
        assert_eq!(output("sleep", &["30"], Duration::from_millis(120)), None);
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the wait returned after {:?}, so the deadline did nothing",
            started.elapsed()
        );
    }
}
