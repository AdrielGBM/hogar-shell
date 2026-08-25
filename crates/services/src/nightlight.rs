//! The night light: how warm the screens are, held by the compositor rather than by a helper process.
//!
//! Thin on purpose. The tint lives in the gamma controls `platform_wayland` holds, so there is no state to duplicate here and nothing to broadcast — a caller asking how warm the screen is asks the thing holding it. What this owns is the one decision the protocol has no opinion about: which temperature "on" means.

/// What `on` warms to when the caller names no temperature.
///
/// Warm enough to take the blue edge off an evening screen and cool enough to read a photograph by. The deep-amber end of the range is a preference, not a default — it is a keystroke away for anyone who wants it.
pub const DEFAULT_TEMPERATURE: u32 = 4000;

/// Whether this compositor lets the shell set gamma at all. `None` when no compositor could be reached, which is a different answer from "it cannot".
pub fn supported() -> Option<bool> {
    platform_wayland::gamma_supported()
}

/// The temperature currently held, or `None` when the screens are at their own.
pub fn current() -> Option<u32> {
    platform_wayland::current_temperature()
}

/// Warms every screen to `kelvin`, clamped to the range the protocol is useful over.
pub fn on(kelvin: u32) -> bool {
    platform_wayland::warm(kelvin)
}

/// Restores every screen's own ramp.
pub fn off() -> bool {
    platform_wayland::neutral_gamma()
}

/// Turns the tint off if any is held, and on at `kelvin` otherwise — what a keybind binds to.
pub fn toggle(kelvin: u32) -> bool {
    match current() {
        Some(_) => off(),
        None => on(kelvin),
    }
}
