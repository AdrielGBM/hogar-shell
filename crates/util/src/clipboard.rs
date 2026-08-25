//! Putting things on the Wayland clipboard.
//!
//! This used to hand off to `wl-copy`. It no longer does, and the reason is worth stating because the old comment here gave the opposite one: Wayland does not let a client *deposit* a selection, it registers a source and serves the bytes to whoever pastes, later. A one-shot process therefore cannot copy — which is precisely what `wl-copy` exists to work around, by forking a daemon that stays alive holding the offer.
//!
//! A shell is already that daemon. So the offer is held here, in the process that produced the data, over `ext-data-control-v1` (or the wlr spelling on an older compositor) — one dependency fewer, and a screenshot no longer round-trips several megabytes through a pipe to a helper.
//!
//! Every copy takes a thread, which then blocks until something else copies. That is not a cost being paid reluctantly: it *is* the selection. The compositor cancels the previous source when a new one arrives, so the thread ends by itself and there is never more than one alive.

/// Copies `text` to the clipboard, off the UI thread — owning a selection means blocking for as long as it is held, and a launcher's Enter handler runs on the frame.
pub fn copy(text: &str) {
    copy_bytes("text/plain;charset=utf-8", text.as_bytes().to_vec());
}

/// Copies `data` under `mime`.
pub fn copy_bytes(mime: &'static str, data: Vec<u8>) {
    let _ = std::thread::Builder::new()
        .name("hogar-shell-clipboard".to_string())
        .spawn(move || {
            if let Err(reason) = platform_wayland::set_selection(mime, data) {
                tracing::warn!("clipboard: {reason}");
            }
        });
}

/// Whether a copy would reach the clipboard, for a caller deciding whether to offer the gesture. `None` when this process cannot reach a compositor to ask.
pub fn supported() -> Option<bool> {
    platform_wayland::clipboard_supported()
}
