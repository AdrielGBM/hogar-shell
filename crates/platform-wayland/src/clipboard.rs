//! Owning the clipboard selection, by speaking the protocol rather than shelling out to `wl-copy`.
//!
//! Wayland has no way for a client to hand the compositor a *copy* of the selection: it registers a **source** and then serves the bytes, on demand, to whoever pastes — possibly minutes later. That is why copying from a one-shot process needs `wl-copy`, which exists to fork into the background and stay alive holding the offer. A shell is already a long-lived process, so it is better placed to do that than a helper it has to spawn: one fewer dependency, and the data never leaves the address space it was produced in.
//!
//! `ext-data-control-v1` is the standardised spelling and `zwlr-data-control-unstable-v1` the older one every wlroots compositor still carries. Both are bound, because "which of the two" is the difference between working on the current Hyprland and working on a two-year-old Sway.
//!
//! **A selection lives on its own thread**, one per copy. Setting a new selection makes the compositor send `cancelled` to the previous source, so the thread that owned it ends by itself — no shared state, no bookkeeping, and exactly the lifetime the protocol already defines.

use std::io::Write;
use std::os::fd::OwnedFd;

use wayland_client::globals::{GlobalList, GlobalListContents, registry_queue_init};
use wayland_client::protocol::{wl_registry, wl_seat};
use wayland_client::{Connection, Dispatch, QueueHandle};

use wayland_protocols::ext::data_control::v1::client::{
    ext_data_control_device_v1::ExtDataControlDeviceV1,
    ext_data_control_manager_v1::ExtDataControlManagerV1,
    ext_data_control_offer_v1::ExtDataControlOfferV1, ext_data_control_source_v1,
    ext_data_control_source_v1::ExtDataControlSourceV1,
};
use wayland_protocols_wlr::data_control::v1::client::{
    zwlr_data_control_device_v1::ZwlrDataControlDeviceV1,
    zwlr_data_control_manager_v1::ZwlrDataControlManagerV1,
    zwlr_data_control_offer_v1::ZwlrDataControlOfferV1, zwlr_data_control_source_v1,
    zwlr_data_control_source_v1::ZwlrDataControlSourceV1,
};

/// What a copy is: the bytes, and what to call them.
struct Selection {
    mime: String,
    data: Vec<u8>,
    /// Turns true when the compositor hands the selection to someone else, which is this thread's cue to end.
    done: bool,
}

/// Puts `data` on the clipboard under `mime`, and keeps serving it until something else copies.
///
/// Returns once the selection is *registered*, not once it is pasted — the thread it leaves behind is what answers the paste. `Err` means the compositor implements neither data-control protocol, which is the one case a caller can do anything about (it can say so).
pub fn set_selection(mime: &str, data: Vec<u8>) -> Result<(), String> {
    let connection = Connection::connect_to_env().map_err(|e| e.to_string())?;
    let (globals, queue) =
        registry_queue_init::<Selection>(&connection).map_err(|e| e.to_string())?;
    let handle = queue.handle();

    let seat: wl_seat::WlSeat = globals
        .bind(&handle, 1..=9, ())
        .map_err(|e| format!("no seat: {e}"))?;

    let mut state = Selection {
        mime: mime.to_string(),
        data,
        done: false,
    };

    // `ext` first: it is the standardised one, and a compositor carrying both is telling us which it prefers.
    if let Ok(manager) = globals.bind::<ExtDataControlManagerV1, _, _>(&handle, 1..=1, ()) {
        let device = manager.get_data_device(&seat, &handle, ());
        let source = manager.create_data_source(&handle, ());
        source.offer(state.mime.clone());
        device.set_selection(Some(&source));
        return serve(connection, queue, &mut state);
    }
    if let Ok(manager) = globals.bind::<ZwlrDataControlManagerV1, _, _>(&handle, 1..=2, ()) {
        let device = manager.get_data_device(&seat, &handle, ());
        let source = manager.create_data_source(&handle, ());
        source.offer(state.mime.clone());
        device.set_selection(Some(&source));
        return serve(connection, queue, &mut state);
    }
    Err("the compositor implements neither ext-data-control nor wlr-data-control".to_string())
}

/// Dispatches until the selection is taken away.
///
/// A round trip first, so the caller learns the selection was accepted rather than being told "sent" about a request the compositor has not seen yet. After that this thread exists only to answer pastes.
fn serve(
    connection: Connection,
    mut queue: wayland_client::EventQueue<Selection>,
    state: &mut Selection,
) -> Result<(), String> {
    queue.roundtrip(state).map_err(|e| e.to_string())?;
    while !state.done {
        if queue.blocking_dispatch(state).is_err() {
            break;
        }
    }
    drop(connection);
    Ok(())
}

/// Writes the selection into the pipe the paster opened.
///
/// Errors are dropped rather than reported: the far end closing early is an ordinary way for a paste to end — a reader that wanted the first line and stopped — and it is not this side's business. What must not happen is a panic on `EPIPE`, which is what an unhandled write error on a closed pipe would be.
fn answer(data: &[u8], fd: OwnedFd) {
    let mut pipe = std::fs::File::from(fd);
    let _ = pipe.write_all(data);
    let _ = pipe.flush();
}

impl Dispatch<ext_data_control_source_v1::ExtDataControlSourceV1, ()> for Selection {
    fn event(
        state: &mut Self,
        _: &ExtDataControlSourceV1,
        event: ext_data_control_source_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            ext_data_control_source_v1::Event::Send { mime_type, fd }
                if mime_type == state.mime =>
            {
                answer(&state.data, fd)
            }
            // Someone else copied. The protocol says this source is spent, so the thread holding it is too.
            ext_data_control_source_v1::Event::Cancelled => state.done = true,
            _ => {}
        }
    }
}

impl Dispatch<zwlr_data_control_source_v1::ZwlrDataControlSourceV1, ()> for Selection {
    fn event(
        state: &mut Self,
        _: &ZwlrDataControlSourceV1,
        event: zwlr_data_control_source_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_data_control_source_v1::Event::Send { mime_type, fd }
                if mime_type == state.mime =>
            {
                answer(&state.data, fd)
            }
            zwlr_data_control_source_v1::Event::Cancelled => state.done = true,
            _ => {}
        }
    }
}

// The device and offer objects carry events about what *others* have copied, which this writer has no use for — but a protocol object still has to have a home for them, and an offer arrives with its own object that would leak without one.
macro_rules! ignores_events {
    ($($proxy:ty),* $(,)?) => {$(
        impl Dispatch<$proxy, ()> for Selection {
            fn event(
                _: &mut Self,
                _: &$proxy,
                _: <$proxy as wayland_client::Proxy>::Event,
                _: &(),
                _: &Connection,
                _: &QueueHandle<Self>,
            ) {
            }
        }
    )*};
}

ignores_events!(
    wl_seat::WlSeat,
    ExtDataControlManagerV1,
    ExtDataControlOfferV1,
    ZwlrDataControlManagerV1,
    ZwlrDataControlOfferV1,
);

/// A device announces what *others* have copied, and its `data_offer` event arrives carrying a **new object**.
///
/// That is why these two cannot go through `ignores_events!`: a protocol event that creates a child needs the binding told what user data to give it, and without the declaration `wayland-client` aborts the process — not an error, an abort, on the first foreign copy. Which is to say: a writer that never reads the clipboard still has to say what it would do with an offer it is handed.
macro_rules! makes_offers {
    ($device:ty, $offer:ty) => {
        impl Dispatch<$device, ()> for Selection {
            wayland_client::event_created_child!(Selection, $device, [
                0 => ($offer, ())
            ]);

            fn event(
                _: &mut Self,
                _: &$device,
                _: <$device as wayland_client::Proxy>::Event,
                _: &(),
                _: &Connection,
                _: &QueueHandle<Self>,
            ) {
            }
        }
    };
}

makes_offers!(ExtDataControlDeviceV1, ExtDataControlOfferV1);
makes_offers!(ZwlrDataControlDeviceV1, ZwlrDataControlOfferV1);

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for Selection {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

/// Whether this compositor can be handed a selection at all — either spelling will do.
pub fn clipboard_supported() -> Option<bool> {
    let ext = crate::globals::advertises("ext_data_control_manager_v1")?;
    let wlr = crate::globals::advertises("zwlr_data_control_manager_v1")?;
    Some(ext || wlr)
}

/// Unused, but the type has to be nameable for `globals.bind` to infer it.
#[allow(dead_code)]
fn _globals_type_is_used(_: &GlobalList) {}
