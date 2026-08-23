//! The open windows, as the compositor itself lists them.
//!
//! Deliberately not part of [`crate::hyprland`]: `wlr-foreign-toplevel-management` is spoken by every wlroots
//! compositor, so nothing here needs a Hyprland to answer. What it costs is a narrower window than
//! `hyprland::clients` — a title, an application id, the outputs it sits on and its state, with no geometry, no
//! workspace and no process id, because the protocol carries none of those. A caller wanting those wants the
//! client list and therefore wants Hyprland.

use std::sync::Arc;

use platform_wayland::{EventSender, Interest, ManagedToplevel, ManagedToplevelId};

use util::broadcast::{Broadcast, Service};

static WINDOWS: Service<Vec<ManagedToplevel>> = Service::new("hogar-shell-windows", run);

fn run(service: &Arc<Broadcast<Vec<ManagedToplevel>>>) {
    let published = Arc::clone(service);
    // The broadcast outlives this call, so the producer thread returns once it has registered rather than
    // parking on a watcher that already has a thread of its own. The claim is what it leaves behind to end the
    // registration with, since by then there is no producer thread here to notice anything.
    let interest = Interest::new();
    let owned = interest.clone();
    platform_wayland::watch_managed_toplevels(&interest, move |windows: &[ManagedToplevel]| {
        published.publish(windows.to_vec());
        if !published.wanted() {
            owned.retire();
        }
    });
}

/// Registers `tx` for the window list and starts the watcher on first use.
pub fn subscribe(tx: EventSender<Vec<ManagedToplevel>>) {
    WINDOWS.subscribe(tx);
}

/// The last published window list, with no round trip.
pub fn current() -> Option<Vec<ManagedToplevel>> {
    WINDOWS.current()
}

/// Stands `windows` in for the compositor's own, without starting the watcher — so a `[preview]` draws the
/// windows it describes whether or not anything is running.
pub fn seed(windows: Vec<ManagedToplevel>) {
    WINDOWS.seed(windows);
}

/// Raises and focuses a window. Reports whether the request could be sent, not whether the compositor obeyed:
/// the protocol answers an action by publishing a new state rather than by replying.
pub fn focus(id: ManagedToplevelId) -> bool {
    platform_wayland::focus_toplevel(id)
}

/// Asks a window to close — the same request its own close button makes, so an application with unsaved work
/// gets to put up its dialog rather than losing it.
pub fn close(id: ManagedToplevelId) -> bool {
    platform_wayland::close_toplevel(id)
}
