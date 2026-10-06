//! What each screen's wallpaper answers to besides its picture ([`services::wallpaper::Desk`]), kept current from the compositor's workspace and window feeds once a region asks for it.

use std::cell::Cell;
use std::collections::HashMap;

use services::wallpaper::{self, Desk};
use telar::{RwSignal, detached, signal};

thread_local! {
    static DESKS: RwSignal<HashMap<Option<String>, Desk>> = detached(|| signal(HashMap::new()));
    static FOLLOWING: Cell<bool> = const { Cell::new(false) };
}

/// The desk of `output`, read as a signal: what reads it follows windows opening and workspaces changing.
pub fn of(output: Option<&str>) -> Desk {
    let key = output.map(str::to_string);
    DESKS.with(|desks| desks.with(|held| held.get(&key).copied().unwrap_or_default()))
}

/// [`of`] without following it.
pub fn now(output: Option<&str>) -> Desk {
    let key = output.map(str::to_string);
    DESKS.with(|desks| desks.peek_with(|held| held.get(&key).copied().unwrap_or_default()))
}

/// Says that `output`'s desk is now `desk`.
pub fn show(output: Option<&str>, desk: Desk) {
    let key = output.map(str::to_string);
    DESKS.with(|desks| {
        if desks.peek_with(|held| held.get(&key) != Some(&desk)) {
            desks.update(|held| {
                held.insert(key, desk);
            });
        }
    });
}

/// Reads `output`'s desk now, and keeps every followed screen's current on each workspace and window change for as long as the shell runs.
pub fn follow(output: Option<&str>) {
    let key = output.map(str::to_string);
    if !DESKS.with(|desks| desks.peek_with(|held| held.contains_key(&key))) {
        show(output, wallpaper::desk_now(output));
    }
    if FOLLOWING.with(|following| following.replace(true)) {
        return;
    }
    platform_wayland::app_watch(services::hyprland::subscribe, |_| refresh());
    platform_wayland::app_watch(services::windows::subscribe, |_| refresh());
}

fn refresh() {
    let followed: Vec<Option<String>> =
        DESKS.with(|desks| desks.peek_with(|held| held.keys().cloned().collect()));
    for output in followed {
        show(output.as_deref(), wallpaper::desk_now(output.as_deref()));
    }
}
