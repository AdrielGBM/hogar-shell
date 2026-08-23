//! Reading pixels back off the compositor.
//!
//! A capture is not a surface: nothing is mapped, nothing is drawn, and the answer is wanted synchronously by
//! whoever asked. So this takes its own short-lived connection — the same shape [`enumerate_outputs`] uses —
//! rather than borrowing the driver's loop, which would mean pumping a screenshot's round trips through the
//! thread every bar is painted on.
//!
//! Two protocols answer the same question, and both are spoken. `ext-image-copy-capture-v1` is the standardised
//! successor and the route taken first; `zwlr-screencopy-v1` is what every wlroots compositor has carried for
//! years and is the fallback. The same two-spelling shape [`crate::clipboard`] takes, for the same reason: which
//! of the two is the difference between working on the current Hyprland and working on a two-year-old Sway.
//!
//! The newer protocol will not crop. It captures a *source* whole and has no region request, so a selection is
//! read back at output size and cut here — which is what [`region`] is, and why the older route is not simply
//! the worse one.
//!
//! [`enumerate_outputs`]: crate::enumerate_outputs

use std::collections::HashMap;
use std::time::{Duration, Instant};

use smithay_client_toolkit::output::{OutputHandler, OutputState};
use smithay_client_toolkit::registry::{ProvidesRegistryState, RegistryState};
use smithay_client_toolkit::reexports::protocols_wlr::screencopy::v1::client::zwlr_screencopy_frame_v1::{
    self, ZwlrScreencopyFrameV1,
};
use smithay_client_toolkit::reexports::protocols_wlr::screencopy::v1::client::zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1;
use smithay_client_toolkit::shm::slot::SlotPool;
use smithay_client_toolkit::shm::{Shm, ShmHandler};
use smithay_client_toolkit::{delegate_output, delegate_registry, delegate_shm, registry_handlers};
use wayland_client::globals::{GlobalList, registry_queue_init};
use wayland_client::protocol::{wl_output, wl_shm};
use wayland_client::{Connection, Dispatch, EventQueue, QueueHandle, WEnum};
use wayland_protocols::ext::foreign_toplevel_list::v1::client::{
    ext_foreign_toplevel_handle_v1::{self, ExtForeignToplevelHandleV1},
    ext_foreign_toplevel_list_v1::{self, ExtForeignToplevelListV1},
};
use wayland_protocols::ext::image_capture_source::v1::client::{
    ext_foreign_toplevel_image_capture_source_manager_v1::ExtForeignToplevelImageCaptureSourceManagerV1,
    ext_image_capture_source_v1::ExtImageCaptureSourceV1,
    ext_output_image_capture_source_manager_v1::ExtOutputImageCaptureSourceManagerV1,
};
use wayland_protocols::ext::image_copy_capture::v1::client::{
    ext_image_copy_capture_frame_v1::{self, ExtImageCopyCaptureFrameV1},
    ext_image_copy_capture_manager_v1::{ExtImageCopyCaptureManagerV1, Options},
    ext_image_copy_capture_session_v1::{self, ExtImageCopyCaptureSessionV1},
};

/// How long a capture waits for the compositor to hand a frame back. A copy is a blit the compositor does on its
/// own schedule, so it needs a bound rather than a parked thread: a shell that hangs on a screenshot is worse
/// than one that says the screenshot failed.
const DEADLINE: Duration = Duration::from_secs(3);

/// One captured image: tightly-packed RGBA8, top row first, in the output's own physical pixels and the right
/// way up whatever transform the screen is running under.
pub struct Capture {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

/// Which part of the screen to read. Region coordinates are relative to the output, in its logical pixels —
/// which is what the older protocol takes, so the translation from a screen-wide selection belongs to the caller
/// that knows where each output sits.
#[derive(Clone, Copy, Debug)]
pub enum CaptureArea {
    Output,
    Region {
        x: i32,
        y: i32,
        width: i32,
        height: i32,
    },
}

/// Which protocol to read through.
///
/// `Auto` is what a user who has not thought about it gets, and it falls back. Naming one means "this route or
/// none": a user who names a backend is usually debugging one, and a silent fallback is what hides the answer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Backend {
    #[default]
    Auto,
    ImageCopyCapture,
    Screencopy,
}

/// The interfaces the `ext-image-copy-capture` route needs. Two globals, not one: the capture manager takes a
/// *source*, and a source for an output comes from its own factory.
pub const IMAGE_COPY_CAPTURE_INTERFACES: &[&str] = &[
    "ext_image_copy_capture_manager_v1",
    "ext_output_image_capture_source_manager_v1",
];

/// The interface the `wlr-screencopy` route needs.
pub const SCREENCOPY_INTERFACES: &[&str] = &["zwlr_screencopy_manager_v1"];

/// Whether this compositor can be asked for pixels at all, by either route. Asked before offering a capture
/// rather than after: finding out from a failed keypress is worse than greying the button out.
pub fn capture_supported() -> bool {
    let has = |interfaces| crate::globals::advertises_all(interfaces) == Some(true);
    has(IMAGE_COPY_CAPTURE_INTERFACES) || has(SCREENCOPY_INTERFACES)
}

/// Captures `area` of `output` (the first output when unnamed), including the cursor when asked.
pub fn capture(
    output: Option<&str>,
    area: CaptureArea,
    cursor: bool,
    backend: Backend,
) -> Result<Capture, CaptureError> {
    let mut reader = Reader::open()?;
    let target = reader.target(output)?;
    match backend {
        Backend::ImageCopyCapture => reader.image_copy_capture(&target, area, cursor),
        Backend::Screencopy => reader.screencopy(&target, area, cursor),
        Backend::Auto => match reader.image_copy_capture(&target, area, cursor) {
            Ok(capture) => Ok(capture),
            Err(newer) => {
                tracing::info!("capture: {newer}; falling back to wlr-screencopy");
                reader.screencopy(&target, area, cursor)
            }
        },
    }
}

#[cfg(test)]
mod toplevel_tests {
    use super::*;

    /// Capturing one window by the identifier the *other* connection reported, which is the whole claim: a
    /// protocol object cannot be shared between connections, and it does not have to be.
    ///
    /// `HOGAR_SHELL_WAYLAND_LIVE=1 cargo test -p platform-wayland toplevel_capture -- --nocapture`
    #[test]
    fn toplevel_capture_names_a_window_across_two_connections() {
        use std::sync::mpsc;
        use std::time::Duration;

        if std::env::var("HOGAR_SHELL_WAYLAND_LIVE").is_err() {
            eprintln!("set HOGAR_SHELL_WAYLAND_LIVE to capture a real window; skipping");
            return;
        }
        assert!(toplevel_capture_supported());

        // The watcher's connection, which is where a caller's identifier would come from.
        let (published, changes) = mpsc::channel();
        let interest = crate::Interest::new();
        assert!(crate::watch_toplevels(
            &interest,
            move |windows: &[crate::Toplevel]| {
                let _ = published.send(windows.to_vec());
            }
        ));
        let mut listed = Vec::new();
        while let Ok(windows) = changes.recv_timeout(Duration::from_millis(500)) {
            listed = windows;
        }
        let window = listed.first().expect("a window is open").clone();
        eprintln!("capturing {:?} {:?}", window.app_id, window.identifier);

        // And the capture's own, which has never seen that handle.
        let shot = capture_toplevel(&window.identifier, false).expect("the window captures");
        eprintln!("{}x{}", shot.width, shot.height);
        assert!(shot.width > 0 && shot.height > 0);
        assert_eq!(
            shot.pixels.len(),
            shot.width as usize * shot.height as usize * 4,
            "tightly packed RGBA8, like every other capture"
        );
        assert!(
            shot.pixels.chunks_exact(4).any(|px| px[..3] != [0, 0, 0]),
            "an all-black window means the capture went through but read nothing"
        );

        assert!(
            capture_toplevel("not-a-window", false).is_err(),
            "an identifier nothing answers to is an error, not someone else's pixels"
        );
    }
}

/// Captures one window, named by the identifier `ext-foreign-toplevel-list-v1` gave it.
///
/// Only the newer protocol can do this: `wlr-screencopy` captures outputs, so there is no fallback and a
/// compositor without `ext-image-copy-capture` says so rather than quietly handing back a screen.
pub fn capture_toplevel(identifier: &str, cursor: bool) -> Result<Capture, CaptureError> {
    Reader::open()?.toplevel(identifier, cursor)
}

/// The interfaces capturing a *window* needs, which is a different pair from capturing an output: the source
/// comes from the toplevel factory, and the list is what hands out the handles that factory takes.
pub const TOPLEVEL_CAPTURE_INTERFACES: &[&str] = &[
    "ext_image_copy_capture_manager_v1",
    "ext_foreign_toplevel_image_capture_source_manager_v1",
    "ext_foreign_toplevel_list_v1",
];

/// Whether this compositor can be asked for one window's pixels.
pub fn toplevel_capture_supported() -> bool {
    crate::globals::advertises_all(TOPLEVEL_CAPTURE_INTERFACES) == Some(true)
}

/// The output to read from, and the two facts about it a capture needs afterwards.
struct Target {
    output: wl_output::WlOutput,
    /// The size the compositor lays this screen out at, which is the space a region is expressed in. `None` when
    /// the compositor announced no `xdg_output` for it.
    logical_size: Option<(i32, i32)>,
    scale: i32,
}

/// One short-lived connection, and the two routes over it.
struct Reader {
    connection: Connection,
    queue: EventQueue<CaptureState>,
    globals: GlobalList,
    state: CaptureState,
}

impl Reader {
    fn open() -> Result<Self, CaptureError> {
        let connection = Connection::connect_to_env().map_err(CaptureError::from)?;
        let (globals, mut queue) =
            registry_queue_init::<CaptureState>(&connection).map_err(CaptureError::from)?;
        let qh = queue.handle();
        let mut state = CaptureState {
            registry_state: RegistryState::new(&globals),
            output_state: OutputState::new(&globals, &qh),
            shm: Shm::bind(&globals, &qh).map_err(CaptureError::from)?,
            session: Session::default(),
            frame: Frame::default(),
            toplevels: HashMap::new(),
        };
        // Two round trips: the first announces the outputs, the second delivers each one's name and mode.
        for _ in 0..2 {
            queue.roundtrip(&mut state).map_err(CaptureError::from)?;
        }
        Ok(Self {
            connection,
            queue,
            globals,
            state,
        })
    }

    /// The output named, else the first the compositor announced.
    fn target(&self, name: Option<&str>) -> Result<Target, CaptureError> {
        let outputs = self.state.output_state.outputs();
        let found = match name {
            None => outputs.into_iter().next(),
            Some(name) => outputs.into_iter().find(|output| {
                self.state
                    .output_state
                    .info(output)
                    .and_then(|info| info.name)
                    .is_some_and(|announced| announced == name)
            }),
        };
        let output = found.ok_or_else(|| match name {
            Some(name) => CaptureError(format!("no output named '{name}'")),
            None => CaptureError("no outputs to capture".to_string()),
        })?;
        let info = self.state.output_state.info(&output);
        Ok(Target {
            logical_size: info.as_ref().and_then(|info| info.logical_size),
            scale: info.map(|info| info.scale_factor).unwrap_or(1).max(1),
            output,
        })
    }

    /// The standardised route: a source for the output, a session over it, then one frame into a buffer sized
    /// the way the session said it must be.
    fn image_copy_capture(
        &mut self,
        target: &Target,
        area: CaptureArea,
        cursor: bool,
    ) -> Result<Capture, CaptureError> {
        let qh = self.queue.handle();
        let sources: ExtOutputImageCaptureSourceManagerV1 = self
            .globals
            .bind(&qh, 1..=1, ())
            .map_err(|e| CaptureError(format!("no ext-image-capture-source: {e}")))?;
        let source = sources.create_source(&target.output, &qh, ());
        let full = self.capture_from(source, cursor)?;
        crop_to(full, area, target)
    }

    /// One window's pixels, named by the identifier `ext-foreign-toplevel-list-v1` gave it.
    ///
    /// The list is bound here rather than borrowed from the watcher: what the two share is the identifier, not
    /// the object. Two round trips, because the first announces the handles and the second delivers what each
    /// one is called.
    fn toplevel(&mut self, identifier: &str, cursor: bool) -> Result<Capture, CaptureError> {
        let qh = self.queue.handle();
        let list: ExtForeignToplevelListV1 = self
            .globals
            .bind(&qh, 1..=1, ())
            .map_err(|e| CaptureError(format!("no ext-foreign-toplevel-list: {e}")))?;
        for _ in 0..2 {
            self.queue
                .roundtrip(&mut self.state)
                .map_err(CaptureError::from)?;
        }
        let handle = self
            .state
            .toplevels
            .get(identifier)
            .cloned()
            .ok_or_else(|| CaptureError(format!("no window with identifier '{identifier}'")))?;

        let sources: ExtForeignToplevelImageCaptureSourceManagerV1 = self
            .globals
            .bind(&qh, 1..=1, ())
            .map_err(|e| CaptureError(format!("no toplevel capture source: {e}")))?;
        let source = sources.create_source(&handle, &qh, ());
        // Nothing here wants to go on hearing about windows; the handles stay valid until this connection ends.
        list.stop();
        self.capture_from(source, cursor)
    }

    /// Everything a capture does once it has a source, which is all of it: the two source factories differ and
    /// both hand back the same `ext_image_capture_source_v1`.
    fn capture_from(
        &mut self,
        source: ExtImageCaptureSourceV1,
        cursor: bool,
    ) -> Result<Capture, CaptureError> {
        let qh = self.queue.handle();
        let manager: ExtImageCopyCaptureManagerV1 = self
            .globals
            .bind(&qh, 1..=1, ())
            .map_err(|e| CaptureError(format!("no ext-image-copy-capture: {e}")))?;

        self.state.session = Session::default();
        self.state.frame = Frame::default();
        let options = if cursor {
            Options::PaintCursors
        } else {
            Options::empty()
        };
        let session = manager.create_session(&source, options, &qh, ());

        // The session answers with the buffer it will fill — a size, and the formats it will accept — and only
        // then is there anything to allocate. Which is why this cannot be one round trip.
        self.pump(|state| state.session.settled())?;
        let constraints = self.state.session.constraints()?;

        let stride = constraints.width * 4;
        let length = (constraints.height as usize * stride as usize).max(1);
        let mut pool = SlotPool::new(length, &self.state.shm).map_err(CaptureError::from)?;
        let (buffer, _) = pool
            .create_buffer(
                constraints.width as i32,
                constraints.height as i32,
                stride as i32,
                constraints.format,
            )
            .map_err(CaptureError::from)?;

        let frame = session.create_frame(&qh, ());
        frame.attach_buffer(buffer.wl_buffer());
        // The whole buffer, always: this session has captured nothing before, so there is no previous content
        // for the compositor to leave standing.
        frame.damage_buffer(0, 0, constraints.width as i32, constraints.height as i32);
        frame.capture();
        self.pump(|state| state.frame.settled())?;

        let outcome = self.state.frame.outcome();
        let pixels = outcome.and_then(|()| {
            let canvas = buffer
                .canvas(&mut pool)
                .ok_or_else(|| CaptureError("the capture buffer was released".to_string()))?;
            Ok(to_rgba(
                canvas,
                constraints.width,
                constraints.height,
                stride,
                constraints.format,
            ))
        });

        frame.destroy();
        session.destroy();
        source.destroy();

        Ok(upright(
            pixels?,
            constraints.width,
            constraints.height,
            self.state.frame.transform,
        ))
    }

    /// The wlroots route, which crops on the compositor's side and so never sends more than the selection.
    fn screencopy(
        &mut self,
        target: &Target,
        area: CaptureArea,
        cursor: bool,
    ) -> Result<Capture, CaptureError> {
        let qh = self.queue.handle();
        let manager: ZwlrScreencopyManagerV1 = self
            .globals
            .bind(&qh, 1..=3, ())
            .map_err(|e| CaptureError(format!("no wlr-screencopy: {e}")))?;

        self.state.frame = Frame::default();
        let overlay = i32::from(cursor);
        let frame = match area {
            CaptureArea::Output => manager.capture_output(overlay, &target.output, &qh, ()),
            CaptureArea::Region {
                x,
                y,
                width,
                height,
            } => {
                manager.capture_output_region(overlay, &target.output, x, y, width, height, &qh, ())
            }
        };

        self.pump(|state| state.frame.buffer.is_some() || state.frame.failed.is_some())?;
        let spec = self
            .state
            .frame
            .buffer
            .ok_or_else(|| CaptureError("the compositor refused the capture".to_string()))?;
        let format = supported_format(spec.format)?;

        let length = (spec.height as usize * spec.stride as usize).max(1);
        let mut pool = SlotPool::new(length, &self.state.shm).map_err(CaptureError::from)?;
        let (buffer, _) = pool
            .create_buffer(
                spec.width as i32,
                spec.height as i32,
                spec.stride as i32,
                format,
            )
            .map_err(CaptureError::from)?;
        frame.copy(buffer.wl_buffer());
        self.pump(|state| state.frame.settled())?;

        let outcome = self.state.frame.outcome();
        let pixels = outcome.and_then(|()| {
            let canvas = buffer
                .canvas(&mut pool)
                .ok_or_else(|| CaptureError("the capture buffer was released".to_string()))?;
            Ok(to_rgba(
                canvas,
                spec.width,
                spec.height,
                spec.stride,
                format,
            ))
        });
        frame.destroy();
        Ok(upright(
            pixels?,
            spec.width,
            spec.height,
            self.state.frame.transform,
        ))
    }

    /// Dispatches until `done` or the deadline. Blocking on the queue rather than spinning: a copy takes as long
    /// as the compositor's next composition, and a busy loop would spend that whole frame burning a core.
    fn pump(&mut self, done: impl Fn(&CaptureState) -> bool) -> Result<(), CaptureError> {
        let start = Instant::now();
        while !done(&self.state) {
            if start.elapsed() > DEADLINE {
                return Err(CaptureError("the compositor did not answer".to_string()));
            }
            self.connection.flush().map_err(CaptureError::from)?;
            self.queue
                .blocking_dispatch(&mut self.state)
                .map_err(CaptureError::from)?;
        }
        Ok(())
    }
}

/// `area` out of a whole-output capture, for the route that cannot ask the compositor to crop.
///
/// The selection arrives in the output's logical pixels and the capture is in its physical ones, so the two are
/// related by whatever ratio the screen is scaled at. Taken from the sizes themselves rather than from the
/// announced integer scale: a screen at 1.5× reports a scale of 2 and is neither.
fn crop_to(full: Capture, area: CaptureArea, target: &Target) -> Result<Capture, CaptureError> {
    let CaptureArea::Region {
        x,
        y,
        width,
        height,
    } = area
    else {
        return Ok(full);
    };
    let ratio = target
        .logical_size
        .filter(|(logical_width, _)| *logical_width > 0)
        .map(|(logical_width, _)| f64::from(full.width as i32) / f64::from(logical_width))
        .unwrap_or(f64::from(target.scale));
    region(&full, x, y, width, height, ratio)
}

/// A rectangle of `full`, given in logical pixels scaled by `ratio`, clamped to what was actually captured.
fn region(
    full: &Capture,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    ratio: f64,
) -> Result<Capture, CaptureError> {
    let to_pixels = |logical: i32| (f64::from(logical) * ratio).round() as i32;
    let left = to_pixels(x).max(0) as u32;
    let top = to_pixels(y).max(0) as u32;
    let width = (to_pixels(width).max(0) as u32).min(full.width.saturating_sub(left));
    let height = (to_pixels(height).max(0) as u32).min(full.height.saturating_sub(top));
    if width == 0 || height == 0 {
        return Err(CaptureError(
            "the selection is outside the screen".to_string(),
        ));
    }
    let mut pixels = Vec::with_capacity(width as usize * height as usize * 4);
    for row in 0..height {
        let start = (((top + row) * full.width + left) * 4) as usize;
        pixels.extend_from_slice(&full.pixels[start..start + width as usize * 4]);
    }
    Ok(Capture {
        width,
        height,
        pixels,
    })
}

fn supported_format(format: WEnum<wl_shm::Format>) -> Result<wl_shm::Format, CaptureError> {
    let format = match format {
        WEnum::Value(format) => format,
        WEnum::Unknown(raw) => return Err(CaptureError(format!("unknown pixel format {raw}"))),
    };
    if is_supported(format) {
        Ok(format)
    } else {
        Err(CaptureError(format!("unsupported pixel format {format:?}")))
    }
}

fn is_supported(format: wl_shm::Format) -> bool {
    matches!(
        format,
        wl_shm::Format::Argb8888
            | wl_shm::Format::Xrgb8888
            | wl_shm::Format::Abgr8888
            | wl_shm::Format::Xbgr8888
    )
}

/// The compositor's buffer as tightly-packed RGBA8, row for row.
///
/// Two corrections, and both are invisible in a still image until they are wrong: the 32-bit formats are
/// little-endian, so an `argb8888` buffer carries blue first; and an `x` format's fourth byte is undefined
/// rather than opaque, so taking it as alpha yields a picture that is transparent in a viewer.
fn to_rgba(canvas: &[u8], width: u32, height: u32, stride: u32, format: wl_shm::Format) -> Vec<u8> {
    let swap_red_blue = matches!(format, wl_shm::Format::Argb8888 | wl_shm::Format::Xrgb8888);
    let opaque = matches!(format, wl_shm::Format::Xrgb8888 | wl_shm::Format::Xbgr8888);
    let width = width as usize;
    let height = height as usize;
    let stride = stride as usize;
    let mut out = Vec::with_capacity(width * height * 4);
    for row in 0..height {
        let start = row * stride;
        for column in 0..width {
            let pixel = &canvas[start + column * 4..start + column * 4 + 4];
            let (r, b) = if swap_red_blue {
                (pixel[2], pixel[0])
            } else {
                (pixel[0], pixel[2])
            };
            out.extend_from_slice(&[r, pixel[1], b, if opaque { 255 } else { pixel[3] }]);
        }
    }
    out
}

/// The picture the right way up, given the transform the compositor says it applied to the buffer.
///
/// The eight `wl_output` transforms are the symmetries of a rectangle, so undoing one is applying another from
/// the same set — [`inverse`] is that table, and this is the only place a rotated or mirrored screen stops
/// coming back sideways.
fn upright(pixels: Vec<u8>, width: u32, height: u32, transform: wl_output::Transform) -> Capture {
    let transform = inverse(transform);
    if transform == wl_output::Transform::Normal {
        return Capture {
            width,
            height,
            pixels,
        };
    }
    let turned = matches!(
        transform,
        wl_output::Transform::_90
            | wl_output::Transform::_270
            | wl_output::Transform::Flipped90
            | wl_output::Transform::Flipped270
    );
    let (out_width, out_height) = if turned {
        (height, width)
    } else {
        (width, height)
    };
    let last_x = width.saturating_sub(1);
    let last_y = height.saturating_sub(1);
    let mut out = Vec::with_capacity(out_width as usize * out_height as usize * 4);
    for y in 0..out_height {
        for x in 0..out_width {
            let (source_x, source_y) = match transform {
                wl_output::Transform::_90 => (last_x - y, x),
                wl_output::Transform::_180 => (last_x - x, last_y - y),
                wl_output::Transform::_270 => (y, last_y - x),
                wl_output::Transform::Flipped => (last_x - x, y),
                wl_output::Transform::Flipped90 => (y, x),
                wl_output::Transform::Flipped180 => (x, last_y - y),
                wl_output::Transform::Flipped270 => (last_x - y, last_y - x),
                _ => (x, y),
            };
            let start = ((source_y * width + source_x) * 4) as usize;
            out.extend_from_slice(&pixels[start..start + 4]);
        }
    }
    Capture {
        width: out_width,
        height: out_height,
        pixels: out,
    }
}

/// The transform that undoes `transform`.
///
/// A rotation is undone by the opposite rotation; every flipped variant is its own inverse, because a flip
/// composed with a rotation is a reflection and a reflection applied twice is nothing.
fn inverse(transform: wl_output::Transform) -> wl_output::Transform {
    match transform {
        wl_output::Transform::_90 => wl_output::Transform::_270,
        wl_output::Transform::_270 => wl_output::Transform::_90,
        other => other,
    }
}

#[derive(Debug)]
pub struct CaptureError(pub String);

impl std::fmt::Display for CaptureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for CaptureError {}

impl CaptureError {
    fn from(error: impl std::fmt::Display) -> Self {
        Self(error.to_string())
    }
}

#[derive(Clone, Copy)]
struct BufferSpec {
    format: WEnum<wl_shm::Format>,
    width: u32,
    height: u32,
    stride: u32,
}

/// What a buffer has to look like for the session to fill it.
struct Constraints {
    width: u32,
    height: u32,
    format: wl_shm::Format,
}

/// The `ext-image-copy-capture` session, up to the point where it has said what it wants.
#[derive(Default)]
struct Session {
    size: Option<(u32, u32)>,
    /// Every shared-memory format offered, in the order offered — the client picks, so the first one this build
    /// can read is the one taken.
    formats: Vec<WEnum<wl_shm::Format>>,
    done: bool,
    stopped: bool,
}

impl Session {
    fn settled(&self) -> bool {
        self.done || self.stopped
    }

    fn constraints(&self) -> Result<Constraints, CaptureError> {
        if self.stopped {
            return Err(CaptureError("the capture session stopped".to_string()));
        }
        let (width, height) = self
            .size
            .ok_or_else(|| CaptureError("the session named no buffer size".to_string()))?;
        let format = self
            .formats
            .iter()
            .find_map(|offered| supported_format(*offered).ok())
            .ok_or_else(|| {
                CaptureError(
                    "the session offered no shared-memory format this build reads".to_string(),
                )
            })?;
        Ok(Constraints {
            width,
            height,
            format,
        })
    }
}

/// One frame in flight, whichever protocol asked for it.
struct Frame {
    /// Only the wlroots route fills this: its frame announces the buffer, where a session announces it once for
    /// every frame that follows.
    buffer: Option<BufferSpec>,
    ready: bool,
    failed: Option<String>,
    transform: wl_output::Transform,
}

impl Default for Frame {
    fn default() -> Self {
        Self {
            buffer: None,
            ready: false,
            failed: None,
            transform: wl_output::Transform::Normal,
        }
    }
}

impl Frame {
    fn settled(&self) -> bool {
        self.ready || self.failed.is_some()
    }

    fn outcome(&self) -> Result<(), CaptureError> {
        match &self.failed {
            Some(reason) => Err(CaptureError(reason.clone())),
            None if self.ready => Ok(()),
            None => Err(CaptureError("the capture failed".to_string())),
        }
    }
}

struct CaptureState {
    registry_state: RegistryState,
    output_state: OutputState,
    shm: Shm,
    session: Session,
    frame: Frame,
    /// The open windows this connection was told about, by the identifier they announced.
    ///
    /// A capture cannot borrow the toplevel watcher's handle — a protocol object belongs to the connection
    /// that made it — but it does not have to: the identifier is what crosses. The protocol promises it is
    /// unique and stable for the window's life, so listing the toplevels again here and matching on it names
    /// the same window without anything being shared.
    toplevels: HashMap<String, ExtForeignToplevelHandleV1>,
}

impl OutputHandler for CaptureState {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }
    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
}

impl ShmHandler for CaptureState {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl ProvidesRegistryState for CaptureState {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers![OutputState];
}

impl Dispatch<ZwlrScreencopyFrameV1, ()> for CaptureState {
    fn event(
        state: &mut Self,
        _: &ZwlrScreencopyFrameV1,
        event: <ZwlrScreencopyFrameV1 as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_screencopy_frame_v1::Event::Buffer {
                format,
                width,
                height,
                stride,
            } => {
                state.frame.buffer = Some(BufferSpec {
                    format,
                    width,
                    height,
                    stride,
                });
            }
            // This protocol has no transform event; the one orientation it reports is a buffer stored bottom-up,
            // which is the same symmetry the newer one spells `flipped_180`.
            zwlr_screencopy_frame_v1::Event::Flags {
                flags: WEnum::Value(flags),
            } => {
                if flags.contains(zwlr_screencopy_frame_v1::Flags::YInvert) {
                    state.frame.transform = wl_output::Transform::Flipped180;
                }
            }
            zwlr_screencopy_frame_v1::Event::Ready { .. } => state.frame.ready = true,
            zwlr_screencopy_frame_v1::Event::Failed => {
                state.frame.failed = Some("the capture failed".to_string())
            }
            _ => {}
        }
    }
}

impl Dispatch<ExtImageCopyCaptureSessionV1, ()> for CaptureState {
    fn event(
        state: &mut Self,
        _: &ExtImageCopyCaptureSessionV1,
        event: <ExtImageCopyCaptureSessionV1 as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            ext_image_copy_capture_session_v1::Event::BufferSize { width, height } => {
                state.session.size = Some((width, height))
            }
            ext_image_copy_capture_session_v1::Event::ShmFormat { format } => {
                state.session.formats.push(format)
            }
            ext_image_copy_capture_session_v1::Event::Done => state.session.done = true,
            ext_image_copy_capture_session_v1::Event::Stopped => state.session.stopped = true,
            _ => {}
        }
    }
}

impl Dispatch<ExtImageCopyCaptureFrameV1, ()> for CaptureState {
    fn event(
        state: &mut Self,
        _: &ExtImageCopyCaptureFrameV1,
        event: <ExtImageCopyCaptureFrameV1 as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            ext_image_copy_capture_frame_v1::Event::Transform {
                transform: WEnum::Value(transform),
            } => state.frame.transform = transform,
            ext_image_copy_capture_frame_v1::Event::Ready => state.frame.ready = true,
            ext_image_copy_capture_frame_v1::Event::Failed { reason } => {
                state.frame.failed = Some(failure(reason))
            }
            _ => {}
        }
    }
}

/// Why a frame failed, in the terms the user's toast will carry.
fn failure(reason: WEnum<ext_image_copy_capture_frame_v1::FailureReason>) -> String {
    use ext_image_copy_capture_frame_v1::FailureReason;
    match reason {
        WEnum::Value(FailureReason::BufferConstraints) => {
            "the compositor changed the buffer it wants mid-capture".to_string()
        }
        WEnum::Value(FailureReason::Stopped) => "the capture session stopped".to_string(),
        _ => "the capture failed".to_string(),
    }
}

wayland_client::delegate_noop!(CaptureState: ignore ZwlrScreencopyManagerV1);
wayland_client::delegate_noop!(CaptureState: ignore ExtImageCopyCaptureManagerV1);
wayland_client::delegate_noop!(CaptureState: ignore ExtOutputImageCaptureSourceManagerV1);
wayland_client::delegate_noop!(CaptureState: ignore ExtForeignToplevelImageCaptureSourceManagerV1);

impl Dispatch<ExtForeignToplevelListV1, ()> for CaptureState {
    wayland_client::event_created_child!(CaptureState, ExtForeignToplevelListV1, [
        ext_foreign_toplevel_list_v1::EVT_TOPLEVEL_OPCODE => (ExtForeignToplevelHandleV1, ()),
    ]);

    fn event(
        _: &mut Self,
        _: &ExtForeignToplevelListV1,
        _: ext_foreign_toplevel_list_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

/// Only the identifier is kept. A capture names a window by the one field the protocol promises is unique and
/// stable, and has no use for a title it would only have to guess with.
impl Dispatch<ExtForeignToplevelHandleV1, ()> for CaptureState {
    fn event(
        state: &mut Self,
        proxy: &ExtForeignToplevelHandleV1,
        event: ext_foreign_toplevel_handle_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let ext_foreign_toplevel_handle_v1::Event::Identifier { identifier } = event {
            state.toplevels.insert(identifier, proxy.clone());
        }
    }
}
wayland_client::delegate_noop!(CaptureState: ignore ExtImageCaptureSourceV1);

delegate_output!(CaptureState);
delegate_shm!(CaptureState);
delegate_registry!(CaptureState);

#[cfg(test)]
mod tests {
    use super::*;

    /// Four bytes per pixel, one pixel per row, two rows — enough to catch every correction at once.
    fn buffer() -> Vec<u8> {
        vec![10, 20, 30, 40, 50, 60, 70, 80]
    }

    /// A 2×2 image whose pixels are numbered, so a transform can be read off the result by eye:
    /// `1 2` over `3 4`.
    fn numbered() -> Vec<u8> {
        (1u8..=4).flat_map(|n| [n, n, n, 255]).collect()
    }

    fn corners(capture: &Capture) -> Vec<u8> {
        capture.pixels.chunks(4).map(|pixel| pixel[0]).collect()
    }

    #[test]
    fn a_little_endian_argb_buffer_comes_back_as_rgba() {
        // `argb8888` is a 32-bit word, so in memory it reads B, G, R, A. Taken as-is, every screenshot on this
        // machine would come out with its reds and blues swapped.
        let rgba = to_rgba(&buffer(), 1, 2, 4, wl_shm::Format::Argb8888);
        assert_eq!(rgba[0..4], [30, 20, 10, 40]);
        assert_eq!(rgba[4..8], [70, 60, 50, 80]);

        // An `xbgr` buffer is already in byte order and its fourth byte means nothing, so it must be forced
        // opaque — kept as alpha, a full-screen capture opens as a transparent image.
        let rgba = to_rgba(&buffer(), 1, 2, 4, wl_shm::Format::Xbgr8888);
        assert_eq!(rgba[0..4], [10, 20, 30, 255]);
    }

    #[test]
    fn padding_between_rows_is_dropped_rather_than_read_as_pixels() {
        // A compositor is free to pad each row; a reader that ignores the stride shears the image.
        let padded = vec![1, 2, 3, 4, 0, 0, 0, 0, 5, 6, 7, 8, 0, 0, 0, 0];
        let rgba = to_rgba(&padded, 1, 2, 8, wl_shm::Format::Xbgr8888);
        assert_eq!(rgba.len(), 8, "two pixels, not four");
        assert_eq!(rgba[4..8], [5, 6, 7, 255]);
    }

    #[test]
    fn a_y_inverted_frame_is_read_bottom_up() {
        // Which is the whole of what wlr-screencopy reports, spelled as the transform that undoes it.
        let flipped = upright(numbered(), 2, 2, wl_output::Transform::Flipped180);
        assert_eq!(
            corners(&flipped),
            vec![3, 4, 1, 2],
            "the last row is drawn first"
        );
        assert_eq!((flipped.width, flipped.height), (2, 2));
    }

    #[test]
    fn a_rotated_screen_comes_back_the_right_way_up() {
        // A screen the compositor turned a quarter turn anticlockwise is undone by turning it back, not by
        // turning it the same way again — which is the mistake that leaves a portrait monitor upside down.
        let turned = upright(numbered(), 2, 2, wl_output::Transform::_90);
        assert_eq!(corners(&turned), vec![3, 1, 4, 2]);
        assert_eq!(
            (turned.width, turned.height),
            (2, 2),
            "a square keeps its size; the axes still swapped"
        );

        // Undoing a rotation twice is doing nothing, which is what makes the table above provable rather than
        // asserted: every transform composed with its own undo is the identity.
        for transform in [
            wl_output::Transform::Normal,
            wl_output::Transform::_90,
            wl_output::Transform::_180,
            wl_output::Transform::_270,
            wl_output::Transform::Flipped,
            wl_output::Transform::Flipped90,
            wl_output::Transform::Flipped180,
            wl_output::Transform::Flipped270,
        ] {
            let once = upright(numbered(), 2, 2, transform);
            let twice = upright(once.pixels, once.width, once.height, inverse(transform));
            assert_eq!(
                corners(&twice),
                vec![1, 2, 3, 4],
                "{transform:?} did not undo"
            );
        }
    }

    #[test]
    fn a_portrait_capture_swaps_its_sides_when_it_is_turned_back() {
        // Three columns of one row, turned a quarter turn: one column of three rows.
        let strip: Vec<u8> = (1u8..=3).flat_map(|n| [n, n, n, 255]).collect();
        let turned = upright(strip, 3, 1, wl_output::Transform::_90);
        assert_eq!((turned.width, turned.height), (1, 3));
    }

    #[test]
    fn a_selection_is_cut_out_of_the_output_in_its_own_pixels() {
        // The route without a region request reads the whole screen back, so the scale between the logical
        // rectangle the user dragged and the pixels that arrived is the only thing standing between a correct
        // crop and one that is off by a factor of two on every HiDPI laptop.
        let full = Capture {
            width: 4,
            height: 4,
            pixels: (0u8..16).flat_map(|n| [n, n, n, 255]).collect(),
        };
        let doubled = region(&full, 1, 1, 1, 1, 2.0).expect("crops");
        assert_eq!((doubled.width, doubled.height), (2, 2));
        assert_eq!(corners(&doubled), vec![10, 11, 14, 15]);

        let unscaled = region(&full, 1, 1, 2, 2, 1.0).expect("crops");
        assert_eq!(corners(&unscaled), vec![5, 6, 9, 10]);
    }

    #[test]
    fn a_selection_past_the_edge_clamps_and_one_outside_it_does_not() {
        let full = Capture {
            width: 4,
            height: 4,
            pixels: vec![7; 4 * 4 * 4],
        };
        let over = region(&full, 3, 3, 10, 10, 1.0).expect("clamps");
        assert_eq!(
            (over.width, over.height),
            (1, 1),
            "a drag past the screen edge is a selection to the edge"
        );
        assert!(region(&full, 9, 9, 2, 2, 1.0).is_err());
    }

    #[test]
    fn a_session_takes_the_first_format_it_can_read_and_refuses_when_there_is_none() {
        let session = Session {
            size: Some((100, 50)),
            formats: vec![
                WEnum::Unknown(9999),
                WEnum::Value(wl_shm::Format::Rgb565),
                WEnum::Value(wl_shm::Format::Xrgb8888),
                WEnum::Value(wl_shm::Format::Argb8888),
            ],
            done: true,
            stopped: false,
        };
        let constraints = session.constraints().expect("one format is readable");
        assert_eq!(constraints.format, wl_shm::Format::Xrgb8888);
        assert_eq!((constraints.width, constraints.height), (100, 50));

        let unreadable = Session {
            formats: vec![WEnum::Value(wl_shm::Format::Rgb565)],
            ..session
        };
        assert!(unreadable.constraints().is_err());
    }

    /// A session that stops before it says anything must read as a failure rather than as a zero-sized picture.
    #[test]
    fn a_stopped_session_has_no_constraints() {
        let stopped = Session {
            stopped: true,
            ..Session::default()
        };
        assert!(stopped.constraints().is_err());
        assert!(stopped.settled(), "and nothing waits on it");
    }

    /// Both routes, against the compositor that is actually running.
    ///
    /// Everything above this line is arithmetic on buffers a test made up; none of it can say whether the
    /// session hand-shake is right, and a protocol implementation that has never spoken to a compositor is a
    /// guess. Needs a live one, so it is opt-in the same way the clipboard round trip is:
    /// `HOGAR_SHELL_WAYLAND_LIVE=1 cargo test -p platform-wayland capture -- --nocapture`
    #[test]
    fn both_routes_read_the_same_screen_back_at_the_same_size() {
        if std::env::var("HOGAR_SHELL_WAYLAND_LIVE").is_err() {
            eprintln!("set HOGAR_SHELL_WAYLAND_LIVE to capture from the real compositor; skipping");
            return;
        }
        const SELECTION: CaptureArea = CaptureArea::Region {
            x: 10,
            y: 20,
            width: 100,
            height: 50,
        };
        let mut sizes = Vec::new();
        for backend in [Backend::ImageCopyCapture, Backend::Screencopy] {
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
            eprintln!(
                "{backend:?}: {}×{} whole, {}×{} region",
                whole.width, whole.height, part.width, part.height
            );
        }
        // The point of the assertion: one protocol crops on the compositor's side and the other crops here, off
        // a whole-output read scaled by hand. A HiDPI screen is where those two stop agreeing.
        assert_eq!(
            sizes[0], sizes[1],
            "the two routes disagree about the screen"
        );
    }
}
