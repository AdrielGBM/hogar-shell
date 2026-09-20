//! What no fixture can prove: that this crate speaks to the compositor it is running under. Opt-in via `HOGAR_SHELL_WAYLAND_LIVE=1 cargo test -p platform-wayland --test live_compositor -- --nocapture --test-threads=1`, since each test opens the process-wide probe gate the unit tests rely on staying shut.

use std::sync::mpsc;
use std::time::Duration;

use platform_wayland::{
    CaptureArea, CaptureBackend, Interest, ManagedToplevel, Toplevel, Workspace,
    activate_workspace, advertises, advertises_all, allow_compositor_probes, capture,
    capture_toplevel, current_temperature, focus_toplevel, focused_toplevel, gamma_supported,
    neutral_gamma, output_power_on, output_power_supported, set_output_power,
    toplevel_capture_supported, warm, watch_managed_toplevels, watch_toplevels, watch_workspaces,
};

/// Whether this run asked for the live compositor, opening the probe gate when it did.
fn live(what: &str) -> bool {
    if std::env::var("HOGAR_SHELL_WAYLAND_LIVE").is_err() {
        eprintln!("set HOGAR_SHELL_WAYLAND_LIVE to {what}; skipping");
        return false;
    }
    allow_compositor_probes();
    true
}

/// Everything a watcher publishes until it goes quiet: one publish per window as the compositor announces them, so the last is the whole list.
fn settled<T>(changes: &mpsc::Receiver<Vec<T>>) -> Vec<T> {
    let mut listed = Vec::new();
    while let Ok(next) = changes.recv_timeout(Duration::from_millis(500)) {
        listed = next;
    }
    listed
}

/// Both capture routes, against the compositor that is actually running: one protocol crops on the compositor's side and the other crops here, off a whole-output read scaled by hand, and a HiDPI screen is where those two stop agreeing.
#[test]
fn both_routes_read_the_same_screen_back_at_the_same_size() {
    if !live("capture from the real compositor") {
        return;
    }
    const SELECTION: CaptureArea = CaptureArea::Region {
        x: 10,
        y: 20,
        width: 100,
        height: 50,
    };
    let mut sizes = Vec::new();
    for backend in [CaptureBackend::ImageCopyCapture, CaptureBackend::Screencopy] {
        let whole = capture(None, CaptureArea::Output, false, backend)
            .unwrap_or_else(|e| panic!("{backend:?} could not capture the screen: {e}"));
        assert!(
            whole.width > 0 && whole.height > 0,
            "{backend:?} read nothing"
        );
        assert_eq!(
            whole.pixels.len(),
            whole.width as usize * whole.height as usize * 4,
            "{backend:?} returned a buffer that is not its own size"
        );
        let part = capture(None, SELECTION, false, backend)
            .unwrap_or_else(|e| panic!("{backend:?} could not capture a region: {e}"));
        assert!(part.width < whole.width, "{backend:?} ignored the region");
        sizes.push((whole.width, whole.height, part.width, part.height));
    }
    assert_eq!(
        sizes[0], sizes[1],
        "the two routes disagree about the screen"
    );
}

/// Capturing one window by the identifier the *other* connection reported: a protocol object cannot be shared between connections, and it does not have to be.
#[test]
fn toplevel_capture_names_a_window_across_two_connections() {
    if !live("capture a real window") {
        return;
    }
    assert!(toplevel_capture_supported());
    let (published, changes) = mpsc::channel();
    let interest = Interest::new();
    assert!(watch_toplevels(&interest, move |windows: &[Toplevel]| {
        let _ = published.send(windows.to_vec());
    }));
    let window = settled(&changes).first().expect("a window is open").clone();
    eprintln!("capturing {:?} {:?}", window.app_id, window.identifier);

    let shot = capture_toplevel(&window.identifier, false).expect("the window captures");
    assert!(shot.width > 0 && shot.height > 0);
    assert_eq!(
        shot.pixels.len(),
        shot.width as usize * shot.height as usize * 4,
        "tightly packed RGBA8, like every other capture"
    );
    assert!(
        shot.pixels
            .as_chunks::<4>()
            .0
            .iter()
            .any(|px| px[..3] != [0, 0, 0]),
        "an all-black window means the capture went through but read nothing"
    );
    assert!(
        capture_toplevel("not-a-window", false).is_err(),
        "an identifier nothing answers to is an error, not someone else's pixels"
    );
}

/// That the compositor accepts a gamma table and holds the tint. **It warms the screen for a second and puts it back**: the protocol has no "what is the gamma" request, so the only evidence is that the compositor did not answer `failed`.
#[test]
fn the_compositor_takes_a_ramp_and_gives_the_screen_back() {
    if !live("tint the real screen") {
        return;
    }
    assert_eq!(
        gamma_supported(),
        Some(true),
        "this compositor does not implement wlr-gamma-control"
    );
    assert!(warm(2500), "the request could not be sent");
    assert_eq!(current_temperature(), Some(2500));
    std::thread::sleep(Duration::from_millis(800));
    assert!(neutral_gamma(), "the screen could not be given back");
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(current_temperature(), None);

    assert!(warm(2500), "a fresh producer could not be started");
    std::thread::sleep(Duration::from_millis(800));
    assert_eq!(current_temperature(), Some(2500));
    assert!(neutral_gamma());
}

/// Reading the power mode, which disturbs nothing, so a missing protocol is told apart from a broken one.
#[test]
fn power_reads_back_from_the_compositor() {
    if !live("ask the real compositor") {
        return;
    }
    assert_eq!(output_power_supported(), Some(true));
    assert_eq!(
        output_power_on(),
        Some(true),
        "the screen this is running on is on, so the reading has to say so"
    );
}

/// **It turns the screen off and back on**: the mode is exactly what this reads back, and reading it without setting it proves only that the protocol answers.
#[test]
fn a_screen_can_be_blanked_and_woken() {
    if !live("blank the real screen") {
        return;
    }
    assert_eq!(output_power_supported(), Some(true));
    assert_eq!(
        output_power_on(),
        Some(true),
        "the screen this is running on should be on"
    );
    set_output_power(false).expect("the screen blanks");
    assert_eq!(output_power_on(), Some(false));
    std::thread::sleep(Duration::from_millis(600));
    set_output_power(true).expect("and wakes again");
    assert_eq!(output_power_on(), Some(true));
}

/// That the watcher reads a real compositor, and that activating over the protocol moves it. **It switches workspace and switches back**, before asserting, so a failed expectation does not leave the desktop somewhere else.
#[test]
fn the_watcher_reads_the_compositor_and_activation_moves_it() {
    if !live("watch the real compositor") {
        return;
    }
    let (published, changes) = mpsc::channel();
    let interest = Interest::new();
    assert!(
        watch_workspaces(&interest, move |workspaces: &[Workspace]| {
            let _ = published.send(workspaces.to_vec());
        }),
        "the compositor advertises ext-workspace-v1 but the watcher would not start"
    );
    let first = changes
        .recv_timeout(Duration::from_secs(3))
        .expect("the watcher publishes the current list without waiting for a change");
    assert!(!first.is_empty(), "a session has at least one workspace");
    assert_eq!(
        first.iter().filter(|w| w.active).count(),
        1,
        "exactly one workspace is active on a single-output session"
    );
    assert!(
        first.iter().all(|w| !w.name.is_empty()),
        "a name is what a pill draws"
    );

    let was_active = first.iter().find(|w| w.active).expect("one is active").id;
    let Some(target) = first.iter().find(|w| w.can_activate && !w.active) else {
        eprintln!("only one workspace exists; nothing to activate");
        return;
    };
    assert!(
        activate_workspace(target.id),
        "the request could not be sent"
    );
    let mut moved = None;
    while let Ok(workspaces) = changes.recv_timeout(Duration::from_secs(3)) {
        if let Some(active) = workspaces.iter().find(|w| w.active) {
            moved = Some(active.id);
            break;
        }
    }
    activate_workspace(was_active);
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(
        moved,
        Some(target.id),
        "activating a workspace over the protocol has to move the compositor"
    );
}

/// That the window list reads a real compositor, and agrees with what it says about itself.
#[test]
fn the_watcher_lists_the_windows_that_are_open() {
    if !live("list the real compositor's windows") {
        return;
    }
    let (published, changes) = mpsc::channel();
    let interest = Interest::new();
    assert!(
        watch_toplevels(&interest, move |windows: &[Toplevel]| {
            let _ = published.send(windows.to_vec());
        }),
        "the compositor advertises ext-foreign-toplevel-list-v1 but the watcher would not start"
    );
    let listed = settled(&changes);
    assert!(
        !listed.is_empty(),
        "this test is running in a terminal, which is itself a window"
    );
    assert!(
        listed.iter().all(|w| !w.identifier.is_empty()),
        "an empty identifier would make every match against IPC succeed"
    );
    let identifiers: std::collections::HashSet<&str> =
        listed.iter().map(|w| w.identifier.as_str()).collect();
    assert_eq!(
        identifiers.len(),
        listed.len(),
        "the protocol promises the identifier is unique, and the join depends on it"
    );
}

/// That what the watcher calls the focused window is the one that has focus.
#[test]
fn the_watcher_agrees_with_the_compositor_about_which_window_has_focus() {
    if !live("read the real compositor") {
        return;
    }
    let (published, changes) = mpsc::channel();
    let interest = Interest::new();
    assert!(
        watch_managed_toplevels(&interest, move |windows: &[ManagedToplevel]| {
            let _ = published.send(windows.to_vec());
        }),
        "the compositor advertises the manager but the watcher would not start"
    );
    let listed = settled(&changes);
    assert!(
        !listed.is_empty(),
        "this test is running in a terminal, which is itself a window"
    );
    assert!(
        listed.iter().filter(|w| w.activated).count() <= 1,
        "two focused windows at once means the state array is being merged instead of replaced"
    );
    assert_eq!(
        focused_toplevel().map(|w| w.id),
        listed.iter().find(|w| w.activated).map(|w| w.id),
        "the convenience reading and the list have to agree"
    );
}

/// Activation on its own, which tells a request that never lands apart from a caller that hands focus straight back. **It focuses another window and puts the focus back.**
#[test]
fn activate_moves_the_compositor_on_its_own() {
    if !live("focus a real window") {
        return;
    }
    let (published, changes) = mpsc::channel();
    let interest = Interest::new();
    assert!(watch_managed_toplevels(
        &interest,
        move |windows: &[ManagedToplevel]| {
            let _ = published.send(windows.to_vec());
        }
    ));
    let listed = settled(&changes);
    let was = listed.iter().find(|w| w.activated).map(|w| w.id);
    let Some(target) = listed.iter().find(|w| !w.activated) else {
        eprintln!("only one window is open; nothing to switch to");
        return;
    };
    assert!(focus_toplevel(target.id), "the request could not be sent");
    let mut moved = None;
    for _ in 0..12 {
        if let Ok(windows) = changes.recv_timeout(Duration::from_millis(250))
            && let Some(active) = windows.iter().find(|w| w.activated)
        {
            moved = Some(active.id);
            if moved == Some(target.id) {
                break;
            }
        }
    }
    if let Some(was) = was {
        focus_toplevel(was);
        std::thread::sleep(Duration::from_millis(400));
    }
    assert_eq!(
        moved,
        Some(target.id),
        "activate did not move the compositor, so the switcher's problem is here and not in its caller"
    );
}

/// Whether this compositor can be asked for a fractional scale at all; the fallback is silent by design and looks like success from inside.
#[test]
fn advertises_fractional_scaling() {
    if !live("ask the real compositor") {
        return;
    }
    let interfaces = ["wp_fractional_scale_manager_v1", "wp_viewporter"];
    for interface in interfaces {
        println!("{interface}: {:?}", advertises(interface));
    }
    assert_eq!(
        advertises_all(&interfaces),
        Some(true),
        "this compositor cannot be asked for a fractional scale; surfaces fall back to whole numbers"
    );
}

/// The optional surface protocols a translucent, cheap shell leans on. The `blur` capability itself arrives as an event on a bound manager, which a registry read cannot see.
#[test]
fn advertises_the_optional_surface_protocols() {
    if !live("ask the real compositor") {
        return;
    }
    assert_eq!(
        advertises("ext_background_effect_manager_v1"),
        Some(true),
        "this compositor cannot blur behind a surface; a translucent one needs a rule in its own config"
    );
    assert_eq!(
        advertises("wp_single_pixel_buffer_manager_v1"),
        Some(true),
        "this compositor cannot be handed a single pixel; reservation strips allocate shm instead"
    );
    assert_eq!(
        advertises("wp_cursor_shape_manager_v1"),
        Some(true),
        "this compositor cannot be asked for a cursor shape; a handle's pointer never changes"
    );
}

mod clipboard {
    use std::os::fd::OwnedFd;

    use platform_wayland::set_selection;
    use std::io::Read;
    use std::os::fd::AsFd;
    use std::sync::mpsc;
    use std::time::Duration;
    use wayland_client::globals::{GlobalListContents, registry_queue_init};
    use wayland_client::protocol::{wl_registry, wl_seat};
    use wayland_client::{Connection, Dispatch, QueueHandle};
    use wayland_protocols::ext::data_control::v1::client::{
        ext_data_control_device_v1::ExtDataControlDeviceV1,
        ext_data_control_manager_v1::ExtDataControlManagerV1,
        ext_data_control_offer_v1::ExtDataControlOfferV1,
    };

    /// The reader half, which exists only so the writer can be proved; reading a selection is not something this shell does, so this lives in the test rather than in the module.
    struct Paster {
        want: String,
        got: Option<String>,
    }

    /// A copy is only real if something else can paste it.
    #[test]
    fn a_selection_can_be_pasted_by_another_client() {
        if !super::live("copy against the real compositor") {
            return;
        }
        const MIME: &str = "text/plain;charset=utf-8";
        let payload = "platform-wayland clipboard round trip";

        // Every copy needs a thread of its own: `set_selection` *is* the ownership, so it does not return until something else takes the selection. Calling it inline would hang the test, which is the same mistake a caller could make — hence `copy_bytes` spawning rather than leaving it to them.
        let (ended, when_ended) = mpsc::channel();
        std::thread::spawn(move || {
            let outcome = set_selection(MIME, payload.as_bytes().to_vec());
            let _ = ended.send(outcome);
        });
        // The set is asynchronous; give the compositor a moment to publish the offer to other clients.
        std::thread::sleep(Duration::from_millis(300));

        let pasted = paste(MIME).expect("a paster could connect");
        assert_eq!(pasted.as_deref(), Some(payload));

        // Taking the selection away is what ends the owning thread, which is the lifetime rule under test: without it, every copy in a session would leave a thread behind for as long as the shell runs.
        std::thread::spawn(|| set_selection(MIME, b"something else".to_vec()));
        let outcome = when_ended
            .recv_timeout(Duration::from_secs(5))
            .expect("the first owner ends when its source is cancelled");
        assert!(outcome.is_ok(), "{outcome:?}");
    }

    /// Reads the current selection through data-control, or `None` if nothing offered `mime`.
    fn paste(mime: &str) -> Result<Option<String>, String> {
        let connection = Connection::connect_to_env().map_err(|e| e.to_string())?;
        let (globals, mut queue) =
            registry_queue_init::<Paster>(&connection).map_err(|e| e.to_string())?;
        let handle = queue.handle();
        let seat: wl_seat::WlSeat = globals
            .bind(&handle, 1..=9, ())
            .map_err(|e| e.to_string())?;
        let manager: ExtDataControlManagerV1 = globals
            .bind(&handle, 1..=1, ())
            .map_err(|e| e.to_string())?;
        let _device = manager.get_data_device(&seat, &handle, ());
        let mut state = Paster {
            want: mime.to_string(),
            got: None,
        };
        for _ in 0..10 {
            queue.roundtrip(&mut state).map_err(|e| e.to_string())?;
            if state.got.is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        Ok(state.got)
    }

    impl Dispatch<ExtDataControlOfferV1, ()> for Paster {
        fn event(
            state: &mut Self,
            offer: &ExtDataControlOfferV1,
            event: wayland_protocols::ext::data_control::v1::client::ext_data_control_offer_v1::Event,
            _: &(),
            connection: &Connection,
            _: &QueueHandle<Self>,
        ) {
            use wayland_protocols::ext::data_control::v1::client::ext_data_control_offer_v1::Event;
            let Event::Offer { mime_type } = event else {
                return;
            };
            if mime_type != state.want || state.got.is_some() {
                return;
            }
            let Ok((read, write)) = std::io::pipe() else {
                return;
            };
            let write = OwnedFd::from(write);
            offer.receive(mime_type, write.as_fd());
            // **Flush before reading, or this deadlocks.** `receive` is a request, and a request made inside a dispatch callback sits in the outgoing buffer until something flushes it — so the sender never hears about the pipe, never writes, and the read below waits for ever.
            let _ = connection.flush();
            // The write end must be dropped here or the read never sees EOF: this process holds a copy of the descriptor the compositor duplicated for the sender.
            drop(write);
            let mut text = String::new();
            let mut read = std::fs::File::from(OwnedFd::from(read));
            if read.read_to_string(&mut text).is_ok() {
                state.got = Some(text);
            }
        }
    }

    impl Dispatch<ExtDataControlDeviceV1, ()> for Paster {
        wayland_client::event_created_child!(Paster, ExtDataControlDeviceV1, [
            0 => (ExtDataControlOfferV1, ())
        ]);

        fn event(
            _: &mut Self,
            _: &ExtDataControlDeviceV1,
            _: wayland_protocols::ext::data_control::v1::client::ext_data_control_device_v1::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
        }
    }

    impl Dispatch<wl_seat::WlSeat, ()> for Paster {
        fn event(
            _: &mut Self,
            _: &wl_seat::WlSeat,
            _: wl_seat::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
        }
    }

    impl Dispatch<ExtDataControlManagerV1, ()> for Paster {
        fn event(
            _: &mut Self,
            _: &ExtDataControlManagerV1,
            _: <ExtDataControlManagerV1 as wayland_client::Proxy>::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
        }
    }

    impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for Paster {
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
}
