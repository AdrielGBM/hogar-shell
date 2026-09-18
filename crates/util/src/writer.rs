//! The one thread that rewrites the shell's files.
//!
//! Atomicity alone is not enough: it makes each write whole, and says nothing about *which* write ends up on disk. `state.json` used to be saved from a thread spawned per call, so two toggles a few milliseconds apart raced — the older thread could rename last, the newer state was gone, and the user's last flick of the switch came back undone after a restart. That is not a bug atomicity can fix, because both writes were perfectly atomic.
//!
//! So every file the shell *rewrites* has one owner, this queue, save the one record named below. A caller hands over a path and the bytes it wants there; this queue decides when they land, and they land in the order they were asked for. Every path also carries a generation, handed out at submission, and a write whose generation has been overtaken is **dropped rather than renamed over a newer one** — the invariant that keeps the later of two rapid writes on disk, and what lets a burst of toggles cost one write instead of thirty.
//!
//! Two kinds of write stay out of it on purpose. A path written once — a cache entry named for what it holds, a screenshot under a name nothing else is using — has no order between its writes to protect, so its caller uses [`crate::fs::write_atomic`] directly. And the session-lock record (`services::lock`) is written synchronously on the driver thread, because its correctness is the order between its write and a later removal, and this queue orders the writes to a path without knowing anything of a `remove_file` beside them.
//!
//! Callers pick how they hear about failure. [`queue`] returns immediately and the writer logs what goes wrong, which is what a state save wants: it happens in a click handler, and a synchronous write there would stall the frame. [`write`] waits for the outcome and hands it back, which is what a config save wants: the settings panel reports a failed save to the user, and `Config::save_section`'s callers have a `Result` to do it with.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex, OnceLock};

/// Queues `bytes` for `path` and returns without waiting. The write still happens in the order it was asked for, relative to every other write to that path.
///
/// For a caller with nowhere to put a `Result` — a toggle flipped from a bar, which must not hold the frame to find out. A failure is logged rather than swallowed; there is no other place left for it to go.
pub fn queue(path: impl Into<PathBuf>, bytes: Vec<u8>) {
    let path = path.into();
    if let Some(done) = enqueue(path.clone(), bytes, None) {
        report(&path, done);
    }
}

/// Queues `bytes` for `path` and waits for the writer to carry it out, so the caller gets the write's own outcome and can read the file back the moment this returns.
///
/// The wait is what keeps `Config::save_section` able to answer with a `Result`: a save the user asked for is a save they are told about when it fails, and the alternative — returning `Ok` and logging the failure somewhere else — is the settings panel reporting a save it did not make. A waited write also sits behind everything already queued, for any path, so its caller waits out those writes and their fsyncs as well as its own; every caller that waits was already writing synchronously on its own thread.
pub fn write(path: impl Into<PathBuf>, bytes: Vec<u8>) -> std::io::Result<()> {
    let (answer, outcome) = mpsc::channel();
    if let Some(done) = enqueue(path.into(), bytes, Some(answer)) {
        return done;
    }
    // The writer answers every job it takes, so a closed channel means the thread died under it — which is not this write's own failure, but is certainly not a success.
    outcome
        .recv()
        .unwrap_or_else(|_| Err(std::io::Error::other("the file writer stopped")))
}

/// Waits until every write asked for so far has been carried out.
///
/// For a test that has to read back what it queued, and for a caller on its way out that would rather not lose the last toggle. It waits on the queue draining rather than on any one write, because the point of the queue is that the *last* write wins and a caller cannot know which one that was.
pub fn flush() {
    // Nothing has ever been written, so there is no thread and nothing to wait for — starting one here to watch it do nothing would be the opposite of what the shell's lazy-start rule is for.
    let Some(writer) = WRITER.get() else {
        return;
    };
    let mut queue = writer.queue.lock().unwrap();
    while !queue.jobs.is_empty() || queue.working {
        queue = writer.turn.wait(queue).unwrap();
    }
}

/// A write somebody has asked for, in the order they asked.
struct Job {
    path: PathBuf,
    bytes: Vec<u8>,
    generation: u64,
    /// Where the outcome goes for a caller waiting on it; `None` for a queued write, whose failure the writer logs itself.
    answer: Option<mpsc::Sender<std::io::Result<()>>>,
}

/// What the writer knows about one path: how many writes have been asked for, and how far it has got. It knows *that* a write landed and never *what* it held — the config reload tells its own echo apart by comparing what the files hold against what it last applied (`config::fingerprint`), which also covers the writes that never came through here.
#[derive(Default)]
struct Track {
    /// The newest generation anybody has asked for. Handed out in submission order, so it is also the count of writes this path has ever been given.
    submitted: u64,
    /// The newest generation whose bytes are on disk.
    written: u64,
}

impl Track {
    /// Whether a write at `generation` must be dropped instead of renamed into place.
    ///
    /// Two reasons, and only the first is a correctness rule: bytes older than what is already on disk must never replace them, which is what makes two rapid writes leave the *later* one behind. The second is thrift — a queued write that nobody is waiting on, whose contents a newer submission has already replaced, is work with no reader, so a burst of toggles collapses to one write instead of thirty.
    ///
    /// A caller blocked on its own outcome is never dropped for the second reason. It asked for *these* bytes to be on disk and is holding a frame to hear that they are, so it gets its write even when a newer one is already queued behind it — which costs one extra write and keeps the answer honest. The newer write still lands last, because the queue is in order.
    fn stale(&self, generation: u64, waited: bool) -> bool {
        generation <= self.written || (!waited && generation < self.submitted)
    }
}

#[derive(Default)]
struct Queue {
    jobs: VecDeque<Job>,
    paths: HashMap<PathBuf, Track>,
    /// Whether a job has been taken off the queue but not finished, so [`flush`] cannot read an empty queue as a finished one.
    working: bool,
}

struct Writer {
    queue: Mutex<Queue>,
    /// Signalled when a job arrives and again when one is finished: the first wakes the writer, the second wakes [`flush`].
    turn: Condvar,
    /// Whether there is a thread behind the queue at all. False only if the process could not spawn one, which would otherwise leave a waiting caller blocked on a queue nobody serves.
    threaded: bool,
}

static WRITER: OnceLock<Arc<Writer>> = OnceLock::new();

/// The writer, starting its thread on the first write the shell asks for — nothing is spawned in a process that never saves anything, the same rule every service here is started by.
fn writer() -> &'static Arc<Writer> {
    WRITER.get_or_init(|| {
        let writer = Arc::new(Writer::new(true));
        let thread = Arc::clone(&writer);
        match std::thread::Builder::new()
            .name("hogar-shell-file-write".to_string())
            .spawn(move || thread.run())
        {
            Ok(_) => writer,
            Err(e) => {
                tracing::warn!("could not start the file writer: {e}");
                Arc::new(Writer::new(false))
            }
        }
    })
}

/// Hands a job to the writer, or — when there is no thread to hand it to — carries it out on the calling thread and returns its outcome, which is why the answer is a `Some` in that case and never otherwise.
fn enqueue(
    path: PathBuf,
    bytes: Vec<u8>,
    answer: Option<mpsc::Sender<std::io::Result<()>>>,
) -> Option<std::io::Result<()>> {
    let writer = writer();
    if !writer.threaded {
        return Some(crate::fs::write_atomic(&path, &bytes));
    }
    let mut queue = writer.queue.lock().unwrap();
    let track = queue.paths.entry(path.clone()).or_default();
    track.submitted += 1;
    let generation = track.submitted;
    queue.jobs.push_back(Job {
        path,
        bytes,
        generation,
        answer,
    });
    writer.turn.notify_all();
    None
}

/// Where a queued write's failure goes, since there is no caller left holding an outcome for it.
fn report(path: &Path, outcome: std::io::Result<()>) {
    if let Err(e) = outcome {
        tracing::warn!("could not write {}: {e}", path.display());
    }
}

impl Writer {
    fn new(threaded: bool) -> Self {
        Self {
            queue: Mutex::new(Queue::default()),
            turn: Condvar::new(),
            threaded,
        }
    }

    fn run(&self) {
        loop {
            let (job, stale) = self.next();
            let outcome = match stale {
                true => Ok(()),
                false => crate::fs::write_atomic(&job.path, &job.bytes),
            };
            let landed = (!stale && outcome.is_ok()).then_some(job.generation);
            self.finish(&job.path, landed);
            match job.answer {
                Some(answer) => {
                    let _ = answer.send(outcome);
                }
                None => report(&job.path, outcome),
            }
        }
    }

    /// Blocks until there is a job to do, and answers with it and with whether a newer write has already made it pointless. Both under one lock, because the verdict is about the queue as it stands the moment the job leaves it.
    fn next(&self) -> (Job, bool) {
        let mut queue = self.queue.lock().unwrap();
        loop {
            if let Some(job) = queue.jobs.pop_front() {
                queue.working = true;
                let waited = job.answer.is_some();
                let stale = queue
                    .paths
                    .get(&job.path)
                    .is_some_and(|track| track.stale(job.generation, waited));
                return (job, stale);
            }
            queue = self.turn.wait(queue).unwrap();
        }
    }

    /// Records the generation now on disk, if one landed, and wakes anything waiting for the queue to drain.
    fn finish(&self, path: &Path, landed: Option<u64>) {
        let mut queue = self.queue.lock().unwrap();
        if let Some(generation) = landed
            && let Some(track) = queue.paths.get_mut(path)
        {
            track.written = generation;
        }
        queue.working = false;
        self.turn.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("hogar-shell-writer-{name}-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// **The race that cost the user their last toggle.** A thread per write meant two saves a few milliseconds apart could rename in either order, so the *older* state won and a switch flicked twice came back on the wrong setting. One queue in submission order is the whole fix, and this is the smallest statement of it.
    #[test]
    fn two_rapid_writes_leave_the_later_one_on_disk() {
        let dir = scratch("rapid");
        let path = dir.join("state.json");
        queue(&path, br#"{"dnd":true}"#.to_vec());
        queue(&path, br#"{"dnd":false}"#.to_vec());
        flush();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            r#"{"dnd":false}"#,
            "the second write is the one the user made last"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The same rule under load, over two paths at once and with both kinds of caller mixed in: however many writes are in flight and whichever way they were asked for, each file ends up holding the last thing asked of *it*.
    #[test]
    fn a_hundred_interleaved_writes_leave_the_last_one_asked_for_on_disk() {
        let dir = scratch("interleaved");
        let state = dir.join("state.json");
        let config = dir.join("config.toml");
        for turn in 0..50 {
            queue(
                &state,
                format!(r#"{{"launch_counts":{turn}}}"#).into_bytes(),
            );
            match turn % 5 {
                0 => write(
                    &config,
                    format!("[clock]\nformat = \"{turn}\"\n").into_bytes(),
                )
                .expect("a waited write reports its own outcome"),
                _ => queue(
                    &config,
                    format!("[clock]\nformat = \"{turn}\"\n").into_bytes(),
                ),
            }
        }
        flush();
        assert_eq!(
            std::fs::read_to_string(&state).unwrap(),
            r#"{"launch_counts":49}"#,
            "the last state write wins, whatever order the queue was filled in"
        );
        assert_eq!(
            std::fs::read_to_string(&config).unwrap(),
            "[clock]\nformat = \"49\"\n",
            "and the other path is tracked separately, by its own generations"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The generation rule on its own, since the two cases it exists for cannot be told apart by looking at the file afterwards: both leave the newest contents there, and only one of them does it by doing less work.
    #[test]
    fn a_write_a_newer_one_has_overtaken_is_dropped_rather_than_renamed_into_place() {
        let track = Track {
            submitted: 4,
            written: 3,
        };
        assert!(
            track.stale(2, true),
            "older bytes must never replace newer ones on disk, however the caller asked"
        );
        assert!(track.stale(3, true), "nor the same bytes twice");
        assert!(
            !track.stale(4, false),
            "the newest submission is the write the queue exists to carry out"
        );

        let queued = Track {
            submitted: 4,
            written: 0,
        };
        assert!(
            queued.stale(2, false),
            "a queued write nobody waits on, already replaced by a newer submission, is work with no reader"
        );
        assert!(
            !queued.stale(2, true),
            "but a caller holding its own outcome gets the write it asked for"
        );
    }

    /// A save the user asked for and did not get has to reach them. The panel reports it, `save_section` returns it — so the writer cannot be the place the error stops, which is what a queued write with no way back would make it.
    #[test]
    fn a_waited_write_hands_its_failure_back_instead_of_only_logging_it() {
        let dir = scratch("failure");
        let blocker = dir.join("config.toml");
        std::fs::write(&blocker, "[theme]\n").unwrap();

        let outcome = write(blocker.join("nested.toml"), b"[theme]\n".to_vec());
        assert!(
            outcome.is_err(),
            "a directory that cannot exist is a failed save, and the caller is the one who says so"
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
