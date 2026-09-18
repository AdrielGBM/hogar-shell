//! What a run reads about itself and the compositor it is talking to, and the tracing layer that files telar's own reports into the timeline.
//!
//! Every reading here is of the running process or its compositor, taken from `/proc` and `getrusage` — nothing is estimated. The one indirection is the compositor's pid, which is not guessed from a process name (a nested session runs two compositors with the same one) but asked of the kernel: a fresh connection to `$WAYLAND_DISPLAY` and `SO_PEERCRED` name exactly the process on the other end of the socket the spike's surfaces live on.

use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

use tracing::field::{Field, Visit};
use tracing::{Event, Level, Metadata, Subscriber};
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};

use crate::timeline::{Record, Recorder};

/// The memory figures the report breaks RSS into, from `/proc/self/status`, in kB.
pub const MEMORY_FIELDS: [&str; 5] = ["VmRSS", "RssAnon", "RssFile", "RssShmem", "VmHWM"];

pub fn memory() -> Vec<(String, u64)> {
    let Ok(status) = std::fs::read_to_string("/proc/self/status") else {
        return Vec::new();
    };
    status
        .lines()
        .filter_map(|line| {
            let (key, value) = line.split_once(':')?;
            if !MEMORY_FIELDS.contains(&key) {
                return None;
            }
            let kb = value.trim().strip_suffix("kB")?.trim().parse().ok()?;
            Some((key.to_owned(), kb))
        })
        .collect()
}

/// CPU time this process has used so far, user and system, every thread included — the UI thread that builds frames and the render threads that rasterise them.
pub fn process_cpu_us() -> u64 {
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    // SAFETY: `usage` is a valid, writable `rusage` for the kernel to fill.
    if unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) } != 0 {
        return 0;
    }
    let micros = |t: libc::timeval| t.tv_sec as u64 * 1_000_000 + t.tv_usec as u64;
    micros(usage.ru_utime) + micros(usage.ru_stime)
}

fn wayland_socket() -> Option<PathBuf> {
    let display = std::env::var_os("WAYLAND_DISPLAY").unwrap_or_else(|| "wayland-0".into());
    let display = PathBuf::from(display);
    if display.is_absolute() {
        return Some(display);
    }
    Some(PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR")?).join(display))
}

/// The pid of the compositor at the other end of `$WAYLAND_DISPLAY`.
pub fn compositor_pid() -> Option<u32> {
    let stream = UnixStream::connect(wayland_socket()?).ok()?;
    let mut credentials: libc::ucred = unsafe { std::mem::zeroed() };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: `credentials` is a writable `ucred` and `len` holds its size, which is what `SO_PEERCRED` fills.
    let status = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut credentials as *mut libc::ucred).cast(),
            &mut len,
        )
    };
    (status == 0 && credentials.pid > 0).then_some(credentials.pid as u32)
}

pub fn process_name(pid: u32) -> Option<String> {
    Some(
        std::fs::read_to_string(format!("/proc/{pid}/comm"))
            .ok()?
            .trim()
            .to_owned(),
    )
}

/// CPU time `pid` has used so far, summed over its threads' scheduler statistics (nanosecond resolution); falls back to `/proc/<pid>/stat`'s tick counts where schedstat is unavailable.
pub fn task_cpu_us(pid: u32) -> Option<u64> {
    let from_schedstat = || -> Option<u64> {
        let mut total = 0u64;
        for task in std::fs::read_dir(format!("/proc/{pid}/task"))
            .ok()?
            .flatten()
        {
            let text = std::fs::read_to_string(task.path().join("schedstat")).ok()?;
            total += text.split_whitespace().next()?.parse::<u64>().ok()?;
        }
        Some(total / 1000)
    };
    from_schedstat().or_else(|| {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        // The command name is parenthesised and may itself hold spaces, so fields are counted from its closing parenthesis.
        let fields: Vec<&str> = stat.rsplit_once(')')?.1.split_whitespace().collect();
        let ticks: u64 =
            fields.get(11)?.parse::<u64>().ok()? + fields.get(12)?.parse::<u64>().ok()?;
        // SAFETY: `sysconf` has no preconditions.
        let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) }.max(1) as u64;
        Some(ticks * 1_000_000 / hz)
    })
}

pub fn cpu_model() -> Option<String> {
    let info = std::fs::read_to_string("/proc/cpuinfo").ok()?;
    let line = info.lines().find(|l| l.starts_with("model name"))?;
    Some(line.split_once(':')?.1.trim().to_owned())
}

pub fn logical_cpus() -> usize {
    std::thread::available_parallelism().map_or(1, |n| n.get())
}

/// Routes telar's `TELAR_PERF` summaries, and anything logged at info level or above, into the timeline.
///
/// Nothing is printed: the run's stderr is the protocol log, and a line of anything else in it is a line the parser has to skip.
pub fn install_tracing(recorder: Recorder) {
    let subscriber = tracing_subscriber::registry().with(Capture { recorder });
    let _ = tracing::subscriber::set_global_default(subscriber);
}

struct Capture {
    recorder: Recorder,
}

fn wanted(metadata: &Metadata<'_>) -> bool {
    *metadata.level() <= Level::INFO
}

impl<S: Subscriber> Layer<S> for Capture {
    fn register_callsite(
        &self,
        metadata: &'static Metadata<'static>,
    ) -> tracing::subscriber::Interest {
        if wanted(metadata) {
            tracing::subscriber::Interest::always()
        } else {
            tracing::subscriber::Interest::never()
        }
    }

    fn enabled(&self, metadata: &Metadata<'_>, _: Context<'_, S>) -> bool {
        wanted(metadata)
    }

    fn on_event(&self, event: &Event<'_>, _: Context<'_, S>) {
        let mut text = Text::default();
        event.record(&mut text);
        let metadata = event.metadata();
        if metadata.target() == "telar_perf" {
            self.recorder.record(Record::Perf {
                summary: text.message,
            });
            return;
        }
        let mut message = text.message;
        for field in text.fields {
            message.push(' ');
            message.push_str(&field);
        }
        self.recorder.record(Record::Log {
            level: metadata.level().to_string(),
            target: metadata.target().to_owned(),
            message,
        });
    }
}

#[derive(Default)]
struct Text {
    message: String,
    fields: Vec<String>,
}

impl Visit for Text {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.message = value.to_owned();
        } else {
            self.fields.push(format!("{}={value}", field.name()));
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.message = format!("{value:?}");
        } else {
            self.fields.push(format!("{}={value:?}", field.name()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn this_process_reports_its_memory_and_cpu() {
        let memory = memory();
        let rss = memory.iter().find(|(k, _)| k == "VmRSS").map(|(_, v)| *v);
        assert!(
            rss.is_some_and(|kb| kb > 0),
            "a running process has resident pages: {memory:?}"
        );
        let _burn: u64 = (0..200_000u64).map(|i| i.wrapping_mul(i)).sum();
        assert!(process_cpu_us() > 0);
    }

    #[test]
    fn a_process_can_read_its_own_task_cpu() {
        assert!(task_cpu_us(std::process::id()).is_some());
    }
}
