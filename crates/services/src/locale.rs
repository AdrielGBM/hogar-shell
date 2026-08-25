//! The UI language as a shared source: one current locale broadcast to every UI thread, so a language switch in one panel updates the bars and other panels live. The locale signal is a thread-local (like the theme), so propagation uses the same watch/subscribe channel pattern as the notification and battery services — `set` fans the new tag out to each subscribed loop, which applies it on its own thread and redraws.

use std::cell::Cell;
use std::sync::Mutex;

use platform_wayland::EventSender;

struct State {
    current: String,
    subscribers: Vec<EventSender<String>>,
}

static STATE: Mutex<State> = Mutex::new(State {
    current: String::new(),
    subscribers: Vec::new(),
});

thread_local! {
    // Ensures the UI thread subscribes exactly once, so `set` doesn't fan out to a growing list of duplicates.
    static SUBSCRIBED: Cell<bool> = const { Cell::new(false) };
}

/// Seeds the current language (from config) without broadcasting. Called at startup and on each config reload.
pub fn init(lang: String) {
    STATE.lock().unwrap().current = lang;
}

/// The current language, or `fallback` if none has been set yet.
pub fn current_or(fallback: String) -> String {
    let current = STATE.lock().unwrap().current.clone();
    if current.is_empty() {
        fallback
    } else {
        current
    }
}

/// Registers `tx` (bound to a surface's event loop) and immediately sends the current language so the surface starts in sync. The `watch` producer for [`attach`].
pub fn subscribe(tx: EventSender<String>) {
    let mut state = STATE.lock().unwrap();
    let current = state.current.clone();
    if !current.is_empty() {
        let _ = tx.send(current);
    }
    state.subscribers.push(tx);
}

/// Switches the language and broadcasts it to every subscribed surface (dropping any whose loop has closed).
pub fn set(lang: impl Into<String>) {
    let lang = lang.into();
    let mut state = STATE.lock().unwrap();
    state.current = lang.clone();
    state.subscribers.retain(|tx| tx.send(lang.clone()));
}

/// Applies the current language on this thread. Call at the top of a surface builder (after loading config): it is what stops a surface appearing in the configured language one frame after it appeared.
pub fn attach(fallback: String) {
    telar::set_locale(current_or(fallback));
}

/// Subscribes the UI thread to language switches, once, for as long as the process runs.
///
/// **At app level, not from a surface build**, for the same reason as the icon store: `watch` binds its channel to whichever surface is being built, and that channel dies when the surface's content is rebuilt — which a config reload does to every bar. Registered from a surface, the subscription would be taken down by the first reload and the guard below would stop anything registering it again, so a language switch would go nowhere for the rest of the session. The locale signal is one per *thread* rather than per surface, so one subscription is all there is to make.
pub fn follow_switches() {
    SUBSCRIBED.with(|done| {
        if !done.replace(true) {
            platform_wayland::watch(subscribe, telar::set_locale);
        }
    });
}
