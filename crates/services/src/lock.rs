//! Whether the session is locked, and everything that decides it.
//!
//! One state, three writers and one performer. The **writers** are the session menu, `hogar-shell lock`, logind's `Lock`/`Unlock` signals and the idle timers — all of which do nothing but change [`LockState`]. The **performer** is [`on_state`], which runs on the driver thread and is the only place that takes or releases the compositor's session lock. Splitting them that way is what makes a lock requested from a keybind, from `loginctl`, and from a click the same lock rather than three racing attempts at one.
//!
//! Two things are checked *before* the screen is covered, never after: that the compositor implements `ext-session-lock-v1`, and that PAM can be loaded. A lock this process cannot undo is the one failure with no way out for the user, so it is refused with a message instead.
//!
//! **A lock outlives the process that took it.** The compositor keeps the session locked when its locker dies, and shows a fallback screen of its own until another client takes the lock over. So once the compositor grants a lock, the shell records that it holds one, keyed to the compositor session it holds it in, and the next start takes that lock back behind the minimal lock screen ([`restore`]) — but only when the compositor also says the session is still locked. Two pieces of evidence, because neither is enough alone. `ext-session-lock-v1` has no read side, and asking it for a lock is no question, since on an unlocked session it *takes* one; so whether the session is locked *now* comes from `hyprland-lock-notify-v1` ([`platform_wayland::compositor_lock`]), and where nothing can tell, nothing is taken back. The record is what keeps the restore to this shell's own crash: without it, a starting shell would reach for any locked session, including one a live locker holds.

use std::cell::{Cell, RefCell};
use std::ffi::OsStr;
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use platform_wayland::{CompositorLock, EventSender, LockHandle};

use crate::pam::{self, AuthError};
use util::broadcast::Store;

/// How often the shell asks the compositor whether the lock it requested has actually been granted. A one-shot chain rather than a standing interval: it exists only while a lock does.
const CONFIRM_POLL: Duration = Duration::from_millis(250);

/// The record of a granted lock, under the shell's runtime directory — `$XDG_RUNTIME_DIR`, which logind empties when the user's last session ends — because it describes one compositor session and means nothing past it.
const MARKER: &str = "held-lock";

/// Which lock screen the opener mounts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Screen {
    /// The screen `[lock]` describes: the clock, the avatar, and whatever readings it switches on.
    Configured,
    /// The password prompt alone, mounted directly rather than as a fallback. What a lock taken back after a crash asks for: whatever killed the last process may be in the configured screen, and dying again would leave the user at the compositor's fallback screen once more.
    Minimal,
}

/// Why the shell is taking a lock, which decides what it draws and what a refusal means.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Origin {
    /// Something asked for it: the session menu, `hogar-shell lock`, logind, the idle timers.
    Requested,
    /// Taking back a lock the previous process died holding.
    Restored,
}

impl Origin {
    fn screen(self) -> Screen {
        match self {
            Origin::Requested => Screen::Configured,
            Origin::Restored => Screen::Minimal,
        }
    }
}

/// Which unlock method is running, so the screen can say what it is waiting for rather than just spinning.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    Password,
    Fingerprint,
    Face,
}

impl Method {
    pub fn message_key(self) -> &'static str {
        match self {
            Method::Password => "lock.checking",
            Method::Fingerprint => "lock.touch_sensor",
            Method::Face => "lock.looking",
        }
    }
}

/// What the lock screen shows and what the rest of the shell reads.
///
/// `wanted` and `locked` are deliberately separate: between asking the compositor and being granted, the desktop may still be on screen. Anything security-sensitive — suspending, reporting the session as locked over IPC — must wait for `locked`, which is the compositor's own word that nothing is visible.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LockState {
    /// The shell wants the session locked.
    pub wanted: bool,
    /// The compositor confirmed it.
    pub locked: bool,
    /// An authentication attempt is in flight; the field is inert until it lands.
    pub busy: Option<Method>,
    /// Consecutive failed attempts, reset by a success.
    pub failures: u32,
    /// The i18n key of the line under the password field, if any.
    pub message: Option<String>,
    /// While this is in the future, attempts are refused without troubling PAM.
    pub locked_out_until: Option<Instant>,
    /// Set when a lock was asked for and could not be taken, so the caller learns rather than waits.
    pub refused: Option<String>,
}

impl LockState {
    /// Whether the field should take a password right now.
    pub fn accepts_input(&self) -> bool {
        self.busy.is_none() && !self.is_locked_out()
    }

    pub fn is_locked_out(&self) -> bool {
        self.locked_out_until
            .is_some_and(|until| Instant::now() < until)
    }

    /// Seconds left on a lockout, for the countdown the screen shows.
    pub fn lockout_remaining(&self) -> u64 {
        self.locked_out_until
            .map(|until| until.saturating_duration_since(Instant::now()).as_secs())
            .unwrap_or(0)
    }
}

static STATE: Store<LockState> = Store::new(LockState::default);

pub fn current() -> LockState {
    STATE.get()
}

/// Registers `tx` for lock-state changes. Every lock surface subscribes, and so does the driver-side performer.
pub fn subscribe(tx: EventSender<LockState>) {
    STATE.subscribe(tx);
}

/// Whether the session is locked *and the compositor has confirmed it* — what `hogar-shell lock status` reports and what a `lockstatus`-style indicator reads.
pub fn is_locked() -> bool {
    STATE.get().locked
}

/// Asks for the session to be locked. Idempotent; the performer does the work on the driver thread.
pub fn lock() {
    if STATE.get().wanted {
        return;
    }
    STATE.update(|state| {
        state.wanted = true;
        state.refused = None;
        state.failures = 0;
        state.message = None;
        state.locked_out_until = None;
    });
}

/// Asks for the session to be unlocked. Only reached after a successful authentication, or from `hogar-shell lock off` — which is a deliberate escape hatch for a shell that has locked a machine its user cannot authenticate to, and is exactly as privileged as the process already is.
pub fn unlock() {
    if !STATE.get().wanted {
        return;
    }
    STATE.update(|state| {
        state.wanted = false;
        state.busy = None;
        state.message = None;
    });
}

/// The reason a lock could not be taken, if the last attempt was refused.
pub fn refusal() -> Option<String> {
    STATE.get().refused
}

/// The lock this process holds.
struct Holding {
    handle: LockHandle,
    origin: Origin,
    /// Whether the compositor has granted it. Kept here rather than read back from the handle, because the driver clears the handle's `locked` as it tears an ended lock down — which is exactly when "it was granted, then ended" and "it was refused" have to be told apart.
    granted: bool,
}

/// Puts the lock screen up, drawing the given [`Screen`] on every output.
type Opener = Box<dyn Fn(Screen) -> LockHandle>;

/// What the lock's lifecycle tells the rest of the machine: logind's `LockedHint`, which `loginctl` and power managers read, and the biometric attempts that run beside a requested lock.
#[derive(Clone, Copy)]
struct Outside {
    locked_hint: fn(bool),
    biometrics: fn(bool),
}

fn run_biometrics(on: bool) {
    if on {
        crate::biometrics::start();
    } else {
        crate::biometrics::stop();
    }
}

thread_local! {
    // The live lock, owned by the driver thread — the only thread that may take or release one.
    static HOLDING: RefCell<Option<Holding>> = const { RefCell::new(None) };
    // Whether a confirmation poll is already in flight, so a burst of state updates arms one chain, not ten.
    static POLLING: RefCell<bool> = const { RefCell::new(false) };
    // How to put the lock screen up. Installed at startup: what the locked screen *draws* is a surface, and a service has no business knowing one — it owns when the session is locked, not what that looks like.
    static SESSION: RefCell<Option<Opener>> = const { RefCell::new(None) };
    // Behind one seam so the lifecycle tests can swap in a recorder: the real calls reach the logind session of whoever runs `cargo test`, and the biometric state every other test in the process shares.
    static OUTSIDE: Cell<Outside> = const {
        Cell::new(Outside {
            locked_hint: crate::session::set_locked_hint,
            biometrics: run_biometrics,
        })
    };
}

fn outside() -> Outside {
    OUTSIDE.with(Cell::get)
}

/// Tells the rest of the machine that this process holds no lock any more, however the lock ended: logind's hint goes back to false, and the biometric attempts stop. One function for every ending — an unlock, a compositor that refused the lock or took it back — so no ending can leave logind reporting a session locked that nothing is covering.
fn stand_down() {
    let outside = outside();
    (outside.biometrics)(false);
    (outside.locked_hint)(false);
}

/// Registers how the lock screen is put up, given which [`Screen`] to mount. Set once at startup by whoever owns the surface.
pub fn set_session_opener(open: impl Fn(Screen) -> LockHandle + 'static) {
    SESSION.with(|hook| *hook.borrow_mut() = Some(Box::new(open)));
}

fn open_session(screen: Screen) -> Option<LockHandle> {
    SESSION.with(|hook| hook.borrow().as_ref().map(|open| open(screen)))
}

/// Whether this machine can lock at all: the compositor implements the protocol and PAM will load. Read on the driver thread — `lock_supported` is answered by the driver's own view of the compositor's globals.
pub fn can_lock() -> Result<(), String> {
    if !platform_wayland::lock_supported() {
        return Err("this compositor does not implement ext-session-lock-v1".to_string());
    }
    let library = config::config()
        .map(|c| c.lock.pam_library.clone())
        .unwrap_or_default();
    if !pam::is_available(&library) {
        return Err(
            "libpam could not be loaded, so nothing could unlock the screen; set [lock] pam_library".to_string(),
        );
    }
    Ok(())
}

/// The performer: reconciles the compositor's lock with what [`LockState`] asks for. Registered once at startup with `platform_wayland::watch(lock::subscribe, lock::on_state)`, so it runs on the driver thread — the only one that may open a surface.
pub fn on_state(state: LockState) {
    let held = HOLDING.with(|holding| holding.borrow().is_some());
    match (state.wanted, held) {
        (true, false) => {
            take(Origin::Requested);
        }
        (false, true) => release(),
        _ => {}
    }
}

/// Asks the compositor for the lock, or refuses and says why. Returns whether it was asked — not whether it was granted, which only the confirmation poll learns.
fn take(origin: Origin) -> bool {
    if let Err(reason) = can_lock() {
        tracing::error!("refusing to lock: {reason}");
        STATE.update(|state| {
            state.wanted = false;
            state.refused = Some(reason);
        });
        return false;
    }
    let Some(handle) = open_session(origin.screen()) else {
        tracing::error!("refusing to lock: no lock surface is installed");
        STATE.update(|state| state.wanted = false);
        return false;
    };
    HOLDING.with(|slot| {
        *slot.borrow_mut() = Some(Holding {
            handle,
            origin,
            granted: false,
        });
    });
    arm_confirmation_poll();
    let outside = outside();
    // A lock taken back after a crash offers the password alone: the fewer parts it runs, the fewer can take down the one way back in.
    if origin == Origin::Requested {
        (outside.biometrics)(true);
    }
    (outside.locked_hint)(true);
    true
}

fn release() {
    stand_down();
    // Before the unlock is even requested, so no moment exists in which the session is unlocked and the record says otherwise; dying in between costs the next start a restore, never a lock.
    Marker::in_runtime_dir().forget();
    HOLDING.with(|slot| {
        if let Some(holding) = slot.borrow_mut().take() {
            holding.handle.unlock();
        }
    });
    STATE.update(|state| {
        state.locked = false;
        state.failures = 0;
        state.locked_out_until = None;
    });
}

/// Follows the lock from "asked for" to "granted", and notices a compositor that refuses or takes it back.
///
/// A one-shot timer that re-arms itself while a lock is held rather than a standing interval: the driver's loop has no way to remove an app-level source once registered, so a permanent ticker would outlive every lock the session ever takes.
fn arm_confirmation_poll() {
    if POLLING.with(|polling| std::mem::replace(&mut *polling.borrow_mut(), true)) {
        return;
    }
    schedule_confirmation_poll();
}

fn schedule_confirmation_poll() {
    platform_wayland::timeout(CONFIRM_POLL, || {
        let reading = HOLDING.with(|slot| {
            slot.borrow_mut().as_mut().map(|holding| {
                let answer = answer(
                    holding.granted,
                    holding.handle.is_locked(),
                    holding.handle.is_finished(),
                );
                holding.granted |= answer == Answer::Granted;
                (answer, holding.origin)
            })
        });
        let Some((answer, origin)) = reading else {
            POLLING.with(|polling| *polling.borrow_mut() = false);
            return;
        };
        match answer {
            Answer::Pending | Answer::Standing => schedule_confirmation_poll(),
            Answer::Granted => {
                Marker::in_runtime_dir().follow(answer, CompositorSession::current);
                STATE.update(|state| state.locked = true);
                schedule_confirmation_poll();
            }
            Answer::Ended { granted } => {
                Marker::in_runtime_dir().follow(answer, CompositorSession::current);
                ended(granted, origin);
            }
        }
    });
}

/// One reading of the compositor's answer to the lock this process holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Answer {
    /// Not granted yet: the desktop may still be on screen.
    Pending,
    /// Granted since the last reading — the moment the lock becomes one the compositor keeps if this process dies.
    Granted,
    /// Granted before, and still held.
    Standing,
    /// The compositor ended the lock: took it back if it had granted it, refused it if it never had.
    Ended { granted: bool },
}

/// Reads the handle's two flags against what the shell already knew. `locked` seen alongside `finished` still counts as granted: both can land between two polls, and a lock granted and then ended has left the session unlocked just the same.
fn answer(granted: bool, locked: bool, finished: bool) -> Answer {
    match (finished, granted, locked) {
        (true, ..) => Answer::Ended {
            granted: granted || locked,
        },
        (false, true, _) => Answer::Standing,
        (false, false, true) => Answer::Granted,
        (false, false, false) => Answer::Pending,
    }
}

/// The compositor ended the lock itself. This process holds no lock any more, and saying otherwise would let a suspend go ahead behind a screen nothing of the shell's is covering.
fn ended(granted: bool, origin: Origin) {
    HOLDING.with(|slot| *slot.borrow_mut() = None);
    POLLING.with(|polling| *polling.borrow_mut() = false);
    stand_down();
    let reason = if !granted && origin == Origin::Restored {
        tracing::error!(
            "the compositor would not hand back the lock the last hogar-shell died holding, so its own fallback screen stays up. \
             On Hyprland, set misc:allow_session_lock_restore = true and restart hogar-shell, or run \
             `hyprctl --instance 0 eval 'hl.clear_crashed_lockscreen()'` from another TTY to unlock without it \
             (docs/features/system/lock.md). {} is kept so a restart takes the lock back; a start that finds the session \
             unlocked forgets it",
            Marker::in_runtime_dir().path.display()
        );
        "the compositor would not hand back the lock the last hogar-shell held"
    } else {
        tracing::warn!("the compositor ended the session lock");
        "the compositor ended the session lock"
    };
    STATE.update(|state| {
        state.wanted = false;
        state.locked = false;
        state.refused = Some(reason.to_string());
    });
}

/// Takes back a lock the previous hogar-shell died holding, so the user meets a password prompt rather than the compositor's fallback screen.
///
/// Called once, at startup, on the driver thread, after the session opener is installed and the config is published: [`can_lock`] reads the driver's view of the compositor and the configured PAM library, [`platform_wayland::compositor_lock`] is only answered there, and the opener reads the config it draws with. The rules are [`restoration`]'s; this only carries them out.
///
/// The lock is taken with [`Screen::Minimal`], and a compositor that will not hand it over — Hyprland without `misc:allow_session_lock_restore` — leaves the record in place, so a restart after the user enables the setting takes it back.
///
/// **One race is left, and it is named rather than engineered around.** The compositor's answer is read here and the lock is asked for on the driver's next turn, so a session unlocked in that instant — by `hl.clear_crashed_lockscreen()`, say, typed at exactly that moment — is locked again behind a password prompt. It exists only at startup, it is as wide as one loop turn, and closing it would take a lock request that is conditional on the session's state, which no protocol offers.
pub fn restore() {
    let marker = Marker::in_runtime_dir();
    let held = match marker.read() {
        Ok(held) => held,
        Err(e) => {
            tracing::warn!(
                "cannot read {}: {e}; not taking back any lock",
                marker.path.display()
            );
            return;
        }
    };
    match restoration(
        held.as_deref(),
        CompositorSession::current(),
        platform_wayland::compositor_lock(),
    ) {
        Restore::Nothing => {}
        Restore::Discard => {
            tracing::info!("forgetting a lock held in an earlier compositor session");
            marker.forget();
        }
        Restore::Unlocked => {
            tracing::info!(
                "the last hogar-shell died holding the session lock, and something has unlocked the session since; forgetting the lock"
            );
            marker.forget();
        }
        Restore::Unverified => {
            tracing::warn!(
                "the last hogar-shell died holding the session lock, but nothing can say whether the session is still locked \
                 (hyprland-lock-notify-v1 is missing, or its first read failed), so the lock is not taken back"
            );
        }
        Restore::Unknown(reason) => {
            report_unidentified(&UNIDENTIFIED_REPORTED, &reason);
        }
        Restore::Take => {
            tracing::warn!(
                "the last hogar-shell died with the session locked; taking the lock back behind the minimal lock screen"
            );
            // Only once the lock is held: the performer would otherwise see `wanted` with nothing held and take a second, configured lock.
            if take(Origin::Restored) {
                lock();
            }
        }
    }
}

/// What a starting shell does about the record it finds.
#[derive(Debug, PartialEq, Eq)]
enum Restore {
    /// The record is from this compositor session and the compositor says the session is locked: the lock is still held, by a process that no longer exists.
    Take,
    /// The record is from another compositor session, or unreadable: whatever it described ended with that session.
    Discard,
    /// The record is from this compositor session, and the compositor says the session is unlocked: something else ended the lock — Hyprland's `hl.clear_crashed_lockscreen()`, another locker, anything. The record is forgotten and nothing is locked.
    Unlocked,
    /// The record is from this compositor session, and nothing can say whether the session is still locked. Left alone: if no query can tell, the shell does not guess.
    Unverified,
    /// No record: the last process held no granted lock when it went.
    Nothing,
    /// A record, and no way to tell which compositor session this is. Left alone both ways: taking the lock could lock a session nobody locked, and deleting the record would lose a lock a later start may still be able to match.
    Unknown(String),
}

/// The startup decision, from the record's contents (`None` when there is none), the current compositor session, and the compositor's own word on whether the session is locked. Pure, so every branch is testable without a compositor.
///
/// Only a record matching this very session is weighed against the compositor's answer: a foreign or unreadable one says nothing about this session, whatever the compositor reports, and is thrown away.
fn restoration(
    held: Option<&str>,
    current: Result<CompositorSession, String>,
    compositor: CompositorLock,
) -> Restore {
    let Some(held) = held else {
        return Restore::Nothing;
    };
    let Some(held) = CompositorSession::parse(held) else {
        return Restore::Discard;
    };
    match current {
        Ok(current) if current == held => match compositor {
            CompositorLock::Locked => Restore::Take,
            CompositorLock::Unlocked => Restore::Unlocked,
            CompositorLock::CannotTell => Restore::Unverified,
        },
        Ok(_) => Restore::Discard,
        Err(reason) => Restore::Unknown(reason),
    }
}

/// Whether [`report_unidentified`] has spoken for this process.
static UNIDENTIFIED_REPORTED: AtomicBool = AtomicBool::new(false);

/// Says, once per process, that the compositor session cannot be identified. Whatever causes it — no `$WAYLAND_DISPLAY`, a socket that is gone — lasts as long as the process does, so repeating it at every lock would only bury the line that explains it. Returns whether it spoke.
fn report_unidentified(reported: &AtomicBool, reason: &str) -> bool {
    if reported.swap(true, Ordering::Relaxed) {
        return false;
    }
    tracing::warn!(
        "cannot tell which compositor session this is ({reason}): no lock is taken back at startup, and a lock this process holds \
         when it dies will not be taken back by the next one"
    );
    true
}

/// Which compositor session this process is connected to, as the identity of the Wayland socket file it connected through.
///
/// `$WAYLAND_DISPLAY` alone is not enough — a restarted compositor binds `wayland-1` again — but the file behind the name is new: the old one is unlinked when its compositor exits, and the next one's `bind` creates another. Its device and inode tell the two apart with no compositor-specific code. The modification time goes with them because inode numbers can be recycled: `$XDG_RUNTIME_DIR` is normally a tmpfs, which numbers inodes from a counter, but nothing guarantees it is one, and a filesystem that reuses a freed inode would hand a new compositor's socket its predecessor's number. A socket's mtime is the moment it was bound — traffic through a socket never touches its inode — so it separates the two to the nanosecond. Every field fails the same way: a mismatch reads as another session, which discards the record and locks nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CompositorSession {
    device: u64,
    inode: u64,
    bound_secs: i64,
    bound_nanos: i64,
}

impl CompositorSession {
    fn current() -> Result<Self, String> {
        let path = socket_path(
            std::env::var_os("WAYLAND_DISPLAY").as_deref(),
            std::env::var_os("XDG_RUNTIME_DIR").as_deref(),
            inherited_socket()?,
        )?;
        let metadata = std::fs::metadata(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        if !metadata.file_type().is_socket() {
            return Err(format!("{} is not a socket", path.display()));
        }
        Ok(Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            bound_secs: metadata.mtime(),
            bound_nanos: metadata.mtime_nsec(),
        })
    }

    fn encode(&self) -> String {
        format!(
            "{} {} {} {}\n",
            self.device, self.inode, self.bound_secs, self.bound_nanos
        )
    }

    /// The inverse of [`encode`](Self::encode), strict about it: anything else is not a record this code wrote, and is discarded rather than guessed at.
    fn parse(text: &str) -> Option<Self> {
        let mut fields = text.split_whitespace();
        let session = Self {
            device: fields.next()?.parse().ok()?,
            inode: fields.next()?.parse().ok()?,
            bound_secs: fields.next()?.parse().ok()?,
            bound_nanos: fields.next()?.parse().ok()?,
        };
        fields.next().is_none().then_some(session)
    }
}

/// Where the socket this process connected through lives, resolved exactly as `wayland-client`'s `connect_to_env` resolved it: `$WAYLAND_DISPLAY` as it is when absolute, else under an absolute `$XDG_RUNTIME_DIR`.
fn socket_path(
    display: Option<&OsStr>,
    runtime_dir: Option<&OsStr>,
    inherited: bool,
) -> Result<PathBuf, String> {
    if inherited {
        return Err(
            "the compositor handed this process a connected socket (WAYLAND_SOCKET), which names no socket file"
                .to_string(),
        );
    }
    let display = display
        .filter(|display| !display.is_empty())
        .map(Path::new)
        .ok_or_else(|| "WAYLAND_DISPLAY is not set".to_string())?;
    if display.is_absolute() {
        return Ok(display.to_path_buf());
    }
    let runtime_dir = runtime_dir
        .map(Path::new)
        .filter(|dir| dir.is_absolute())
        .ok_or_else(|| "XDG_RUNTIME_DIR is not an absolute path".to_string())?;
    Ok(runtime_dir.join(display))
}

/// Whether this process started with `WAYLAND_SOCKET`: a connection the compositor made and handed over, which names no socket file, so `$WAYLAND_DISPLAY` may name some other compositor entirely. Read from `/proc/self/environ` because that is the environment the process *started* with — the connection removes the variable from the live one as it uses it.
fn inherited_socket() -> Result<bool, String> {
    let environ = std::fs::read("/proc/self/environ")
        .map_err(|e| format!("cannot read /proc/self/environ: {e}"))?;
    Ok(environ
        .split(|byte| *byte == 0)
        .any(|variable| variable.starts_with(b"WAYLAND_SOCKET=")))
}

/// The record that this process holds a lock the compositor has granted, and in which compositor session.
///
/// **Written and removed synchronously, on the driver thread, and never through `util::writer`.** The order of the two is the whole of its correctness: a write that lands after a later removal leaves a record of a lock that is gone, which is one of the two conditions a start needs before it locks, and leaves the compositor's answer as the only thing between it and a lock nobody asked for. The writer's queue orders writes to one path among themselves but knows nothing of a `remove_file` beside them, and [`util::writer::queue`] returns before its write lands — so a queued record could land after an unlock had already removed it. Here both happen in the order the compositor's answers arrive, on the one thread that reads them, so no write can overtake a removal.
///
/// Written with [`util::fs::write_atomic`], so a crash part-way leaves no record or a whole one; a torn record would not parse, and one that does not parse is discarded rather than trusted.
struct Marker {
    path: PathBuf,
}

impl Marker {
    fn in_runtime_dir() -> Self {
        Self {
            path: util::paths::runtime_dir().join(MARKER),
        }
    }

    /// The record's contents, or `None` when there is none.
    fn read(&self) -> std::io::Result<Option<String>> {
        match std::fs::read_to_string(&self.path) {
            Ok(text) => Ok(Some(text)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Keeps the record in step with one reading of the compositor's answer: written when the lock is granted, and not a moment before, because the compositor's promise to keep the session locked covers a client that dies *while locked* — a record written at the request would outlive a lock that was refused. Removed when a granted lock ends. **Kept** when a lock ends without ever being granted: that is a restore the compositor would not accept, and the lock it was restoring is still held by the process that died.
    fn follow(&self, answer: Answer, session: impl FnOnce() -> Result<CompositorSession, String>) {
        match answer {
            Answer::Granted => self.record(session()),
            Answer::Ended { granted: true } => self.forget(),
            Answer::Pending | Answer::Standing | Answer::Ended { granted: false } => {}
        }
    }

    fn record(&self, session: Result<CompositorSession, String>) {
        let session = match session {
            Ok(session) => session,
            Err(reason) => {
                report_unidentified(&UNIDENTIFIED_REPORTED, &reason);
                return;
            }
        };
        if let Err(e) = util::fs::write_atomic(&self.path, session.encode().as_bytes()) {
            tracing::error!(
                "cannot record the lock at {}: {e}; if hogar-shell dies while locked, the next start will not take the lock back",
                self.path.display()
            );
        }
    }

    fn forget(&self) {
        match std::fs::remove_file(&self.path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => tracing::error!(
                "cannot remove {}: {e}; a hogar-shell started in this compositor session will act on it if the compositor reports the session locked",
                self.path.display()
            ),
        }
    }
}

/// Takes a password attempt. Returns immediately: PAM is run on a worker thread, because `pam_unix` sleeps for seconds after a wrong password and the lock screen must keep drawing while it does.
pub fn submit(password: String) {
    let state = STATE.get();
    if !state.wanted || !state.accepts_input() {
        return;
    }
    if password.is_empty() {
        STATE.update(|state| state.message = Some("lock.empty_password".to_string()));
        return;
    }
    let config = config::shared_config()
        .map(|c| c.lock.clone())
        .unwrap_or_default();
    let service = pam::service_name(&config.pam_service);
    let user = pam::current_user();
    STATE.update(|state| {
        state.busy = Some(Method::Password);
        state.message = None;
    });
    let _ = std::thread::Builder::new()
        .name("hogar-shell-pam".to_string())
        .spawn(move || {
            let verdict = pam::authenticate(&service, &user, &password, &config.pam_library);
            drop(password);
            match verdict {
                Ok(()) => succeed(Method::Password),
                Err(error) => fail(error, config.max_tries, config.lockout_seconds),
            }
        });
}

/// A successful unlock, whatever proved it. The one path out of the lock, so a fingerprint and a password leave exactly the same state behind.
pub fn succeed(method: Method) {
    tracing::info!("unlocked by {method:?}");
    STATE.update(|state| {
        state.wanted = false;
        state.busy = None;
        state.failures = 0;
        state.message = None;
        state.locked_out_until = None;
    });
}

/// A failed attempt: counts it, says why, and starts a lockout once the configured limit is reached.
pub fn fail(error: AuthError, max_tries: u32, lockout_seconds: u64) {
    let key = error.message_key().to_string();
    if let AuthError::Unavailable(detail) = &error {
        tracing::error!("authentication is unavailable: {detail}");
    }
    let next = STATE.update(|state| {
        state.busy = None;
        state.failures += 1;
        state.message = Some(key);
        // `max_tries = 0` never locks out — a machine whose owner would rather keep retrying than be shut out for thirty seconds every time they fumble a long passphrase.
        if max_tries > 0 && state.failures >= max_tries && lockout_seconds > 0 {
            state.locked_out_until = Some(Instant::now() + Duration::from_secs(lockout_seconds));
        }
    });
    if let Some(deadline) = next.locked_out_until {
        clear_lockout_when_elapsed(deadline);
    }
}

/// Publishes one more state change when the lockout expires, so the screen re-enables its field on its own rather than only when the user next presses a key it is ignoring.
fn clear_lockout_when_elapsed(deadline: Instant) {
    let remaining = deadline.saturating_duration_since(Instant::now());
    let _ = std::thread::Builder::new()
        .name("hogar-shell-lockout".to_string())
        .spawn(move || {
            std::thread::sleep(remaining + Duration::from_millis(50));
            STATE.update(|state| {
                if !state.is_locked_out() {
                    state.locked_out_until = None;
                    state.failures = 0;
                    state.message = None;
                }
            });
        });
}

/// Marks a biometric attempt as running, so the screen says what it is waiting for and a password typed meanwhile is not thrown away by a competing attempt.
pub fn set_busy(method: Option<Method>) {
    if STATE.get().busy == method {
        return;
    }
    STATE.update(|state| state.busy = method);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wanting_a_lock_and_holding_one_are_different_questions() {
        // The window between the two is the whole reason they are separate fields: a suspend triggered on `wanted` would race the compositor's first covered frame.
        let asked = LockState {
            wanted: true,
            locked: false,
            ..LockState::default()
        };
        assert!(!asked.locked, "asking is not being locked");
        let granted = LockState {
            locked: true,
            ..asked.clone()
        };
        assert!(granted.locked);
    }

    #[test]
    fn a_lockout_refuses_input_until_it_elapses() {
        let mut state = LockState {
            wanted: true,
            ..LockState::default()
        };
        assert!(state.accepts_input(), "an idle field takes a password");

        state.busy = Some(Method::Password);
        assert!(
            !state.accepts_input(),
            "a check in flight is not a second prompt"
        );

        state.busy = None;
        state.locked_out_until = Some(Instant::now() + Duration::from_secs(30));
        assert!(state.is_locked_out());
        assert!(!state.accepts_input());
        assert!(state.lockout_remaining() > 25);

        state.locked_out_until = Some(Instant::now() - Duration::from_secs(1));
        assert!(!state.is_locked_out(), "an elapsed lockout is over");
        assert_eq!(state.lockout_remaining(), 0);
        assert!(state.accepts_input());
    }

    fn session(inode: u64, bound_nanos: i64) -> CompositorSession {
        CompositorSession {
            device: 56,
            inode,
            bound_secs: 1_789_733_138,
            bound_nanos,
        }
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Told {
        LockedHint(bool),
        Biometrics(bool),
    }

    thread_local! {
        static TOLD: RefCell<Vec<Told>> = const { RefCell::new(Vec::new()) };
    }

    /// Swaps the machine for a recorder on this test's thread, so nothing here reaches logind or the biometric state other tests share.
    fn record_outside() {
        OUTSIDE.with(|outside| {
            outside.set(Outside {
                locked_hint: |locked| {
                    TOLD.with(|told| told.borrow_mut().push(Told::LockedHint(locked)));
                },
                biometrics: |on| TOLD.with(|told| told.borrow_mut().push(Told::Biometrics(on))),
            });
        });
    }

    fn last_locked_hint() -> Option<bool> {
        TOLD.with(|told| {
            told.borrow().iter().rev().find_map(|told| match told {
                Told::LockedHint(locked) => Some(*locked),
                Told::Biometrics(_) => None,
            })
        })
    }

    #[test]
    fn a_lock_the_compositor_refuses_leaves_logind_told_the_session_is_unlocked() {
        record_outside();
        for origin in [Origin::Requested, Origin::Restored] {
            TOLD.with(|told| told.borrow_mut().clear());
            ended(false, origin);
            assert_eq!(
                last_locked_hint(),
                Some(false),
                "take() told logind the session was locked; a refusal ({origin:?}) that leaves it so tells loginctl and every power manager that a session nothing is covering is locked"
            );
        }
    }

    #[test]
    fn a_lock_the_compositor_takes_back_leaves_logind_told_the_session_is_unlocked() {
        record_outside();
        ended(true, Origin::Requested);
        assert_eq!(
            last_locked_hint(),
            Some(false),
            "the compositor ended a lock it had granted, so the session is open again and the hint has to say so"
        );
        assert!(
            TOLD.with(|told| told.borrow().contains(&Told::Biometrics(false))),
            "and the biometric attempts that ran beside it stop, as they do on an unlock"
        );
    }

    #[test]
    fn a_take_refused_before_the_compositor_is_asked_never_tells_logind_the_session_is_locked() {
        record_outside();
        assert!(
            !take(Origin::Requested),
            "with no driver on this thread the protocol reads as missing, so the take is refused"
        );
        assert_eq!(
            last_locked_hint(),
            None,
            "a lock that was never asked for must not reach logind as a locked session"
        );
    }

    /// A marker in a directory of the test's own. Never the runtime directory: a record matching the live compositor session would make the next real start lock the screen.
    fn scratch_marker(name: &str) -> Marker {
        let dir =
            std::env::temp_dir().join(format!("hogar-shell-lock-{name}-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        Marker {
            path: dir.join(MARKER),
        }
    }

    fn discard(marker: Marker) {
        if let Some(dir) = marker.path.parent() {
            std::fs::remove_dir_all(dir).ok();
        }
    }

    const EVERY_ANSWER: [CompositorLock; 3] = [
        CompositorLock::Locked,
        CompositorLock::Unlocked,
        CompositorLock::CannotTell,
    ];

    #[test]
    fn a_lock_held_in_this_compositor_session_and_still_locked_is_taken_back() {
        let here = session(84, 123_972_133);
        assert_eq!(
            restoration(Some(&here.encode()), Ok(here), CompositorLock::Locked),
            Restore::Take,
            "the record names this very socket and the compositor says the session is locked: the lock is still held, for a process that is gone"
        );
    }

    #[test]
    fn a_lock_something_else_has_since_ended_is_forgotten_and_nothing_is_locked() {
        let here = session(84, 123_972_133);
        assert_eq!(
            restoration(Some(&here.encode()), Ok(here), CompositorLock::Unlocked),
            Restore::Unlocked,
            "hl.clear_crashed_lockscreen() or another locker unlocked the session; locking it now would lock a session nobody locked"
        );
    }

    #[test]
    fn a_session_the_compositor_cannot_vouch_for_is_left_alone() {
        let here = session(84, 123_972_133);
        assert_eq!(
            restoration(Some(&here.encode()), Ok(here), CompositorLock::CannotTell),
            Restore::Unverified,
            "with no notifier nothing can tell a session still held from one unlocked since, and the shell does not guess"
        );
    }

    #[test]
    fn no_record_takes_nothing_whatever_the_compositor_says() {
        for compositor in EVERY_ANSWER {
            assert_eq!(
                restoration(None, Ok(session(84, 0)), compositor),
                Restore::Nothing,
                "a shell that held no granted lock when it went leaves nothing to take back, even from a locked session ({compositor:?})"
            );
            assert_eq!(
                restoration(
                    None,
                    Err("WAYLAND_DISPLAY is not set".to_string()),
                    compositor
                ),
                Restore::Nothing,
                "with no record there is nothing to match, so an unknown session is not worth a word ({compositor:?})"
            );
        }
    }

    #[test]
    fn a_record_from_another_compositor_session_is_discarded_whatever_the_compositor_says() {
        let earlier = session(84, 123_972_133);
        for compositor in EVERY_ANSWER {
            assert_eq!(
                restoration(
                    Some(&earlier.encode()),
                    Ok(session(91, 123_972_133)),
                    compositor
                ),
                Restore::Discard,
                "a restarted compositor binds a new socket file; the lock the record describes ended with the old one ({compositor:?})"
            );
            assert_eq!(
                restoration(
                    Some(&earlier.encode()),
                    Ok(session(84, 555_000_001)),
                    compositor
                ),
                Restore::Discard,
                "the same inode bound at another moment is a recycled number, not the same session ({compositor:?})"
            );
            assert_eq!(
                restoration(Some("84 not-a-record\n"), Ok(earlier), compositor),
                Restore::Discard,
                "a record this code did not write cannot vouch for anything, so it is thrown away rather than trusted ({compositor:?})"
            );
        }
    }

    #[test]
    fn an_unidentifiable_session_takes_nothing_and_is_reported_once() {
        let held = session(84, 123_972_133).encode();
        for compositor in EVERY_ANSWER {
            assert_eq!(
                restoration(
                    Some(&held),
                    Err("WAYLAND_DISPLAY is not set".to_string()),
                    compositor
                ),
                Restore::Unknown("WAYLAND_DISPLAY is not set".to_string()),
                "a record that cannot be matched must neither lock the session nor be deleted, and the reason has to reach the log ({compositor:?})"
            );
        }

        let reported = AtomicBool::new(false);
        assert!(report_unidentified(&reported, "WAYLAND_DISPLAY is not set"));
        assert!(
            !report_unidentified(&reported, "WAYLAND_DISPLAY is not set"),
            "the cause lasts as long as the process, so saying it at every lock would bury the one line that explains it"
        );
    }

    #[test]
    fn the_socket_is_found_the_way_the_connection_found_it() {
        let runtime = Some(OsStr::new("/run/user/1000"));
        assert_eq!(
            socket_path(Some(OsStr::new("wayland-1")), runtime, false),
            Ok(PathBuf::from("/run/user/1000/wayland-1"))
        );
        assert_eq!(
            socket_path(Some(OsStr::new("/tmp/nested-0")), runtime, false),
            Ok(PathBuf::from("/tmp/nested-0")),
            "an absolute WAYLAND_DISPLAY is used as it is, whatever the runtime directory"
        );
        assert!(
            socket_path(None, runtime, false).is_err(),
            "no display means no connection was made through a socket file"
        );
        assert!(
            socket_path(
                Some(OsStr::new("wayland-1")),
                Some(OsStr::new("run/user")),
                false
            )
            .is_err(),
            "the connection refuses a relative runtime directory, so it cannot name the socket it used"
        );
        assert!(
            socket_path(Some(OsStr::new("wayland-1")), runtime, true).is_err(),
            "a handed-over connection may belong to a compositor WAYLAND_DISPLAY does not name"
        );
    }

    #[test]
    fn the_record_is_written_only_once_the_compositor_grants_the_lock() {
        let marker = scratch_marker("granted");
        let here = session(84, 123_972_133);

        marker.follow(answer(false, false, false), || Ok(here));
        assert!(
            !marker.path.exists(),
            "asked for is not held: the compositor keeps a lock only for a locker that dies while locked"
        );

        marker.follow(answer(false, true, false), || Ok(here));
        assert_eq!(
            std::fs::read_to_string(&marker.path).ok(),
            Some(here.encode()),
            "granted is the moment the lock would outlive this process, so that is when it is recorded"
        );
        discard(marker);
    }

    #[test]
    fn releasing_the_lock_removes_the_record() {
        let marker = scratch_marker("released");
        let here = session(84, 123_972_133);
        marker.follow(answer(false, true, false), || Ok(here));

        marker.forget();
        assert!(
            !marker.path.exists(),
            "a record left behind by an unlock would lock the next start's session, which nobody locked"
        );
        marker.forget();
        discard(marker);
    }

    #[test]
    fn a_lock_the_compositor_ends_after_granting_it_leaves_no_record() {
        let marker = scratch_marker("ended");
        let here = session(84, 123_972_133);
        marker.follow(answer(false, true, false), || Ok(here));

        marker.follow(answer(true, false, true), || Ok(here));
        assert!(
            !marker.path.exists(),
            "the compositor took the lock back, so the session is unlocked and the record is a lie"
        );

        std::fs::write(&marker.path, here.encode()).unwrap();
        marker.follow(answer(false, true, true), || Ok(here));
        assert!(
            !marker.path.exists(),
            "granted and ended between two polls is still a lock that ended, not one that was refused"
        );
        discard(marker);
    }

    #[test]
    fn a_restore_the_compositor_refuses_keeps_the_record() {
        let marker = scratch_marker("refused");
        let held = session(84, 123_972_133);
        std::fs::write(&marker.path, held.encode()).unwrap();

        marker.follow(answer(false, false, true), || Ok(held));
        assert_eq!(
            std::fs::read_to_string(&marker.path).ok(),
            Some(held.encode()),
            "refused before it was ever granted: the dead process's lock still holds the session, and the record is what lets a restart take it back once the compositor allows it"
        );
        discard(marker);
    }

    #[test]
    fn every_method_says_what_it_is_waiting_for() {
        for method in [Method::Password, Method::Fingerprint, Method::Face] {
            assert!(method.message_key().starts_with("lock."));
        }
        assert_ne!(
            Method::Fingerprint.message_key(),
            Method::Face.message_key(),
            "a sensor to touch and a camera to face are different instructions"
        );
    }
}
