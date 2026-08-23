//! One PipeWire capture stream, in this process rather than behind a pipe.
//!
//! The visualiser needs what the speakers are playing, and reading it used to cost a `pw-cat`: a process, a
//! pipe and a copy per hop, running for as long as anything was subscribed. That was the one genuinely hot
//! subprocess this shell had left. Here it is a `pw_stream` on a main loop of its own, handing the samples
//! straight to a closure.
//!
//! **Loaded at runtime, not linked**, for the same reason as [`nvml`](super::nvml) and [`pam`](super::pam):
//! linking `libpipewire-0.3` would make PipeWire a *build* dependency of a shell that degrades to silent bars
//! without it at runtime, and would put a second library between a clone and a working bar. The other two
//! PipeWire helpers stay subprocesses on purpose — `pw-dump --monitor` is one long-lived process that parses
//! only when the graph changes, and `wpctl` is one fork per volume change. Neither is hot; this one was.
//!
//! **The callback runs on the main loop, not the data thread.** Without `PW_STREAM_FLAG_RT_PROCESS` PipeWire
//! hands `process` to the loop this thread is running, which is what makes it safe to do a transform and wake
//! every subscribed surface from inside it. With that flag it would be the realtime graph thread, where both
//! are a missed deadline for every other application on the machine.
//!
//! **Exactly one format is offered**, so negotiation either produces f32 mono at the asked rate or fails
//! outright — which is what makes reading a buffer as `f32` safe without parsing the negotiated format back.
//! The conversion happens in the stream's own adapter, and `node.rate` is deliberately not set: asking the
//! *graph* to run at 44.1 kHz would make every other application resample for a row of bars, which is a thing
//! `pw-cat --rate` does and this does not.

use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::ops::ControlFlow;
use std::panic::AssertUnwindSafe;
use std::sync::OnceLock;

use util::deps::{self, Dep};

/// PipeWire's objects, only ever held as pointers.
type MainLoop = *mut c_void;
type Loop = *mut c_void;
type Stream = *mut c_void;
type Properties = *mut c_void;

/// SPA's basic pod types, from `spa/utils/type.h`. Only the two a format object is built from.
const SPA_TYPE_ID: u32 = 3;
const SPA_TYPE_INT: u32 = 4;
const SPA_TYPE_OBJECT: u32 = 15;
const SPA_TYPE_OBJECT_FORMAT: u32 = 0x0004_0003;

/// The parameter this pod *is*: the formats the stream will accept.
const SPA_PARAM_ENUM_FORMAT: u32 = 3;

/// The keys inside a format object, from `spa/param/format.h`.
const SPA_FORMAT_MEDIA_TYPE: u32 = 1;
const SPA_FORMAT_MEDIA_SUBTYPE: u32 = 2;
const SPA_FORMAT_AUDIO_FORMAT: u32 = 0x0001_0001;
const SPA_FORMAT_AUDIO_RATE: u32 = 0x0001_0003;
const SPA_FORMAT_AUDIO_CHANNELS: u32 = 0x0001_0004;

/// The values those keys carry here: raw audio, little-endian 32-bit float.
const SPA_MEDIA_TYPE_AUDIO: u32 = 1;
const SPA_MEDIA_SUBTYPE_RAW: u32 = 1;
const SPA_AUDIO_FORMAT_F32_LE: u32 = 0x11b;

/// A capture is an *input* to this process, whatever it is reading.
const PW_DIRECTION_INPUT: u32 = 0;

/// Let the session manager choose the target. The sink is named by `stream.capture.sink` instead, which is
/// what turns a recording stream around onto what is being played rather than onto a microphone.
const PW_ID_ANY: u32 = u32::MAX;

const PW_STREAM_FLAG_AUTOCONNECT: u32 = 1 << 0;
/// Ask for the buffers to be mapped, without which `data` comes back null and there is nothing to read.
const PW_STREAM_FLAG_MAP_BUFFERS: u32 = 1 << 2;

const PW_STREAM_STATE_ERROR: c_int = -1;
const PW_STREAM_STATE_UNCONNECTED: c_int = 0;

/// The events struct this fills in. Claiming the version it was written against is what tells an older
/// library not to reach for a callback it has no field for.
const PW_VERSION_STREAM_EVENTS: u32 = 2;

#[repr(C)]
struct SpaDictItem {
    key: *const c_char,
    value: *const c_char,
}

#[repr(C)]
struct SpaDict {
    flags: u32,
    n_items: u32,
    items: *const SpaDictItem,
}

#[repr(C)]
struct SpaChunk {
    offset: u32,
    size: u32,
    stride: i32,
    flags: i32,
}

#[repr(C)]
struct SpaData {
    kind: u32,
    flags: u32,
    fd: i64,
    mapoffset: u32,
    maxsize: u32,
    data: *mut c_void,
    chunk: *mut SpaChunk,
}

#[repr(C)]
struct SpaBuffer {
    n_metas: u32,
    n_datas: u32,
    metas: *mut c_void,
    datas: *mut SpaData,
}

/// Only the first field is read, and the pointer is handed straight back to PipeWire. The rest of the struct
/// has grown over 0.3's life and none of it is wanted here.
#[repr(C)]
struct PwBuffer {
    buffer: *mut SpaBuffer,
}

/// The callback table, laid out as `pw_stream_events` — every slot present so the offsets match, and only the
/// two that are wanted filled in.
#[repr(C)]
struct StreamEvents {
    version: u32,
    destroy: Option<unsafe extern "C" fn(*mut c_void)>,
    state_changed: Option<unsafe extern "C" fn(*mut c_void, c_int, c_int, *const c_char)>,
    control_info: Option<unsafe extern "C" fn(*mut c_void, u32, *const c_void)>,
    io_changed: Option<unsafe extern "C" fn(*mut c_void, u32, *mut c_void, u32)>,
    param_changed: Option<unsafe extern "C" fn(*mut c_void, u32, *const c_void)>,
    add_buffer: Option<unsafe extern "C" fn(*mut c_void, *mut PwBuffer)>,
    remove_buffer: Option<unsafe extern "C" fn(*mut c_void, *mut PwBuffer)>,
    process: Option<unsafe extern "C" fn(*mut c_void)>,
    drained: Option<unsafe extern "C" fn(*mut c_void)>,
    command: Option<unsafe extern "C" fn(*mut c_void, *const c_void)>,
    trigger_done: Option<unsafe extern "C" fn(*mut c_void)>,
}

static EVENTS: StreamEvents = StreamEvents {
    version: PW_VERSION_STREAM_EVENTS,
    destroy: None,
    state_changed: Some(on_state_changed),
    control_info: None,
    io_changed: None,
    param_changed: None,
    add_buffer: None,
    remove_buffer: None,
    process: Some(on_process),
    drained: None,
    command: None,
    trigger_done: None,
};

/// The handful of `libpipewire` symbols a capture needs. A stream carries its own context and core, which is
/// what `pw_stream_new_simple` is for — the rest of the connection API never has to be named.
struct Pw {
    main_loop_new: unsafe extern "C" fn(*const SpaDict) -> MainLoop,
    main_loop_get_loop: unsafe extern "C" fn(MainLoop) -> Loop,
    main_loop_run: unsafe extern "C" fn(MainLoop) -> c_int,
    main_loop_quit: unsafe extern "C" fn(MainLoop) -> c_int,
    main_loop_destroy: unsafe extern "C" fn(MainLoop),
    properties_new_dict: unsafe extern "C" fn(*const SpaDict) -> Properties,
    stream_new_simple: unsafe extern "C" fn(
        Loop,
        *const c_char,
        Properties,
        *const StreamEvents,
        *mut c_void,
    ) -> Stream,
    stream_connect: unsafe extern "C" fn(Stream, u32, u32, u32, *mut *const c_void, u32) -> c_int,
    stream_dequeue_buffer: unsafe extern "C" fn(Stream) -> *mut PwBuffer,
    stream_queue_buffer: unsafe extern "C" fn(Stream, *mut PwBuffer) -> c_int,
    stream_destroy: unsafe extern "C" fn(Stream),
    /// Held so the symbols above stay mapped. Never called.
    _library: libloading::Library,
}

// The function pointers are into a library that is never unloaded, and every object made from them lives on
// one thread — the loop is created, run and destroyed inside a single call.
unsafe impl Send for Pw {}
unsafe impl Sync for Pw {}

static PW: OnceLock<Option<Pw>> = OnceLock::new();

fn library() -> Option<&'static Pw> {
    PW.get_or_init(load).as_ref()
}

fn load() -> Option<Pw> {
    // SAFETY: loading a shared object runs its initialisers, and every symbol is looked up by the signature
    // PipeWire documents for it.
    unsafe {
        deps::open_library(Dep::LibPipeWire, None, |library| {
            let init = *library
                .get::<unsafe extern "C" fn(*mut c_int, *mut *mut *mut c_char)>(b"pw_init\0")?;
            let pw = Pw {
                main_loop_new: *library.get(b"pw_main_loop_new\0")?,
                main_loop_get_loop: *library.get(b"pw_main_loop_get_loop\0")?,
                main_loop_run: *library.get(b"pw_main_loop_run\0")?,
                main_loop_quit: *library.get(b"pw_main_loop_quit\0")?,
                main_loop_destroy: *library.get(b"pw_main_loop_destroy\0")?,
                properties_new_dict: *library.get(b"pw_properties_new_dict\0")?,
                stream_new_simple: *library.get(b"pw_stream_new_simple\0")?,
                stream_connect: *library.get(b"pw_stream_connect\0")?,
                stream_dequeue_buffer: *library.get(b"pw_stream_dequeue_buffer\0")?,
                stream_queue_buffer: *library.get(b"pw_stream_queue_buffer\0")?,
                stream_destroy: *library.get(b"pw_stream_destroy\0")?,
                _library: library,
            };
            // Once, here, and never undone: `pw_deinit` on a process that is exiting anyway buys nothing, and
            // a second capture must not re-initialise the library underneath the first.
            init(std::ptr::null_mut(), std::ptr::null_mut());
            Ok(pw)
        })
    }
}

/// Captures the default sink's monitor until it stops, handing `on_hop` exactly `hop` samples at a time.
///
/// Blocks the calling thread for the life of the capture: it *is* the main loop. `Ok(())` means the stream ran
/// and then ended — PipeWire went away, or `on_hop` asked to stop — which is a caller's cue to re-attach.
/// `Err` means it never started, which is the one case worth retiring on.
pub fn monitor(
    rate: u32,
    hop: usize,
    on_hop: &mut dyn FnMut(&[f32]) -> ControlFlow<()>,
) -> std::io::Result<()> {
    let pw =
        library().ok_or_else(|| std::io::Error::other("libpipewire is not on this machine"))?;
    let main_loop = unsafe { (pw.main_loop_new)(std::ptr::null()) };
    if main_loop.is_null() {
        return Err(std::io::Error::other("PipeWire gave no main loop"));
    }
    let result = run(pw, main_loop, rate, hop, on_hop);
    unsafe { (pw.main_loop_destroy)(main_loop) };
    result
}

/// What one capture holds. Reached only through the raw pointer PipeWire was handed, which is what keeps the
/// callbacks and this function from aliasing the same `&mut`.
struct Capture<'a> {
    pw: &'static Pw,
    main_loop: MainLoop,
    stream: Stream,
    hop: usize,
    /// Samples arrived but not yet handed on: a quantum is whatever the graph chose, and a hop is what the
    /// consumer asked for.
    pending: Vec<f32>,
    on_hop: &'a mut dyn FnMut(&[f32]) -> ControlFlow<()>,
    /// Set when the stream ends for any reason, so the loop is never entered after it has been asked to stop.
    stopped: bool,
    /// What PipeWire said when the stream went to `error`.
    failure: Option<String>,
    /// A panic out of `on_hop`, carried back to the caller's thread rather than unwinding through C.
    panic: Option<Box<dyn std::any::Any + Send>>,
}

impl Capture<'_> {
    fn stop(&mut self) {
        self.stopped = true;
        unsafe { (self.pw.main_loop_quit)(self.main_loop) };
    }

    fn drain(&mut self) {
        while !self.stopped {
            let buffer = unsafe { (self.pw.stream_dequeue_buffer)(self.stream) };
            if buffer.is_null() {
                return;
            }
            self.take(buffer);
            unsafe { (self.pw.stream_queue_buffer)(self.stream, buffer) };
        }
    }

    fn take(&mut self, buffer: *mut PwBuffer) {
        let Some(data) = (unsafe { first_data(buffer) }) else {
            return;
        };
        for sample in data.chunks_exact(size_of::<f32>()) {
            self.pending
                .push(f32::from_le_bytes(sample.try_into().unwrap_or_default()));
            if self.pending.len() < self.hop {
                continue;
            }
            let asked = (self.on_hop)(&self.pending);
            self.pending.clear();
            if asked.is_break() {
                self.stop();
                return;
            }
        }
    }
}

/// The valid bytes of a buffer's first block, or `None` when there is nothing to read.
///
/// Read as bytes rather than as `f32`: a chunk carries an offset into a mapping, and nothing in the protocol
/// promises it lands where a float wants to start.
unsafe fn first_data<'a>(buffer: *mut PwBuffer) -> Option<&'a [u8]> {
    let spa = unsafe { (*buffer).buffer };
    if spa.is_null() || unsafe { (*spa).n_datas } == 0 {
        return None;
    }
    let data = unsafe { &*(*spa).datas };
    let chunk = unsafe { data.chunk.as_ref()? };
    if data.data.is_null() || data.maxsize == 0 {
        return None;
    }
    // Modulo, not a clamp: an offset is allowed to run past the end of a mapping that is being used as a ring,
    // and `spa/buffer/buffer.h` says to take it that way.
    let offset = (chunk.offset % data.maxsize) as usize;
    let size = (chunk.size as usize).min(data.maxsize as usize - offset);
    Some(unsafe { std::slice::from_raw_parts(data.data.cast::<u8>().add(offset), size) })
}

unsafe extern "C" fn on_process(data: *mut c_void) {
    let capture = unsafe { &mut *data.cast::<Capture<'_>>() };
    // A panic must not unwind into C. Carried out on the caller's thread instead, where it means what it
    // always did: the service thread dies with the message.
    if let Err(panic) = std::panic::catch_unwind(AssertUnwindSafe(|| capture.drain())) {
        capture.panic = Some(panic);
        capture.stop();
    }
}

unsafe extern "C" fn on_state_changed(
    data: *mut c_void,
    _previous: c_int,
    state: c_int,
    error: *const c_char,
) {
    let capture = unsafe { &mut *data.cast::<Capture<'_>>() };
    match state {
        PW_STREAM_STATE_ERROR => {
            capture.failure = Some(match unsafe { error.as_ref() } {
                Some(error) => unsafe { CStr::from_ptr(error) }
                    .to_string_lossy()
                    .into_owned(),
                None => "the stream failed".to_string(),
            });
            capture.stop();
        }
        // Reached when the graph drops the stream — PipeWire restarting, mostly — and again on the way out of
        // `pw_stream_destroy`, where quitting a loop that has already returned is a no-op.
        PW_STREAM_STATE_UNCONNECTED => capture.stop(),
        _ => {}
    }
}

fn run(
    pw: &'static Pw,
    main_loop: MainLoop,
    rate: u32,
    hop: usize,
    on_hop: &mut dyn FnMut(&[f32]) -> ControlFlow<()>,
) -> std::io::Result<()> {
    // The quantum the graph is asked for, so one wakeup carries one hop's worth of new sound rather than a
    // buffer to unpick.
    let latency = CString::new(format!("{hop}/{rate}")).map_err(std::io::Error::other)?;
    let items = [
        SpaDictItem {
            key: c"media.type".as_ptr(),
            value: c"Audio".as_ptr(),
        },
        SpaDictItem {
            key: c"media.category".as_ptr(),
            value: c"Capture".as_ptr(),
        },
        SpaDictItem {
            key: c"stream.capture.sink".as_ptr(),
            value: c"true".as_ptr(),
        },
        SpaDictItem {
            key: c"node.latency".as_ptr(),
            value: latency.as_ptr(),
        },
        SpaDictItem {
            key: c"application.name".as_ptr(),
            value: c"hogar-shell".as_ptr(),
        },
    ];
    let dict = SpaDict {
        flags: 0,
        n_items: items.len() as u32,
        items: items.as_ptr(),
    };

    let properties = unsafe { (pw.properties_new_dict)(&dict) };
    if properties.is_null() {
        return Err(std::io::Error::other("PipeWire gave no properties"));
    }

    let capture = Box::into_raw(Box::new(Capture {
        pw,
        main_loop,
        stream: std::ptr::null_mut(),
        hop,
        pending: Vec::with_capacity(hop),
        on_hop,
        stopped: false,
        failure: None,
        panic: None,
    }));

    let stream = unsafe {
        (pw.stream_new_simple)(
            (pw.main_loop_get_loop)(main_loop),
            c"visualiser".as_ptr(),
            properties,
            &EVENTS,
            capture.cast(),
        )
    };
    if stream.is_null() {
        drop(unsafe { Box::from_raw(capture) });
        return Err(std::io::Error::other("PipeWire gave no stream"));
    }
    unsafe { (*capture).stream = stream };

    let format = audio_format(rate);
    let mut params = [format.0.as_ptr().cast::<c_void>()];
    let connected = unsafe {
        (pw.stream_connect)(
            stream,
            PW_DIRECTION_INPUT,
            PW_ID_ANY,
            PW_STREAM_FLAG_AUTOCONNECT | PW_STREAM_FLAG_MAP_BUFFERS,
            params.as_mut_ptr(),
            params.len() as u32,
        )
    };

    // A stream that failed while connecting has already asked the loop to quit, and a quit that arrives before
    // `pw_main_loop_run` is a quit nobody hears — so the flag is read here rather than trusted to the loop.
    let started = connected >= 0 && !unsafe { (*capture).stopped };
    if started {
        unsafe { (pw.main_loop_run)(main_loop) };
    }
    unsafe { (pw.stream_destroy)(stream) };

    let capture = unsafe { Box::from_raw(capture) };
    if let Some(panic) = capture.panic {
        std::panic::resume_unwind(panic);
    }
    if !started {
        return Err(if connected < 0 {
            std::io::Error::from_raw_os_error(-connected)
        } else {
            std::io::Error::other(
                capture
                    .failure
                    .unwrap_or_else(|| "the stream never connected".to_string()),
            )
        });
    }
    if let Some(failure) = capture.failure {
        tracing::warn!("the audio capture failed ({failure})");
    }
    Ok(())
}

/// One property: a key, its flags, then a value pod of a size, a type and four bytes padded to eight.
const PROPERTY_WORDS: usize = 6;
/// The pod's own size and type, the object's type and id, then the five properties below.
const FORMAT_WORDS: usize = 4 + 5 * PROPERTY_WORDS;

const NO_FLAGS: u32 = 0;
const VALUE_WORD: u32 = 4;
const PAD: u32 = 0;

/// PipeWire reads a pod through pointers into it, so it has to be aligned where it expects to find one.
#[repr(align(8))]
struct FormatPod([u32; FORMAT_WORDS]);

/// The one format this stream will accept: f32, mono, at `rate`.
///
/// Mono because a visualiser draws one row of bars, and the stream's own adapter sums the channels for less
/// than a fold per hop would cost here.
///
/// Built by hand because the C spelling of this is `spa_format_audio_raw_build`, a `static inline` in a header
/// — there is no such symbol to load. The shape is in `spa/pod/pod.h`: a pod is a body size, a type and a body
/// padded to eight bytes; an object pod's body is its own type and id followed by properties.
fn audio_format(rate: u32) -> FormatPod {
    let mut pod = FormatPod([0; FORMAT_WORDS]);
    let body = (FORMAT_WORDS - 2) * size_of::<u32>();
    pod.0[..4].copy_from_slice(&[
        body as u32,
        SPA_TYPE_OBJECT,
        SPA_TYPE_OBJECT_FORMAT,
        SPA_PARAM_ENUM_FORMAT,
    ]);
    for (index, (key, kind, value)) in [
        (SPA_FORMAT_MEDIA_TYPE, SPA_TYPE_ID, SPA_MEDIA_TYPE_AUDIO),
        (SPA_FORMAT_MEDIA_SUBTYPE, SPA_TYPE_ID, SPA_MEDIA_SUBTYPE_RAW),
        (
            SPA_FORMAT_AUDIO_FORMAT,
            SPA_TYPE_ID,
            SPA_AUDIO_FORMAT_F32_LE,
        ),
        (SPA_FORMAT_AUDIO_RATE, SPA_TYPE_INT, rate),
        (SPA_FORMAT_AUDIO_CHANNELS, SPA_TYPE_INT, 1),
    ]
    .into_iter()
    .enumerate()
    {
        let at = 4 + index * PROPERTY_WORDS;
        pod.0[at..at + PROPERTY_WORDS]
            .copy_from_slice(&[key, NO_FLAGS, VALUE_WORD, kind, value, PAD]);
    }
    pod
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The pod is the one thing here a compiler cannot check: a wrong size or a misplaced pad is not a build
    /// error, it is a stream that negotiates something else and a buffer read as the wrong type.
    #[test]
    fn the_format_pod_is_the_object_pipewire_expects_to_parse() {
        let pod = audio_format(44_100);
        assert_eq!(size_of_val(&pod.0), 136);
        assert_eq!(pod.0[0] as usize, size_of_val(&pod.0) - 8, "the body size");
        assert_eq!(pod.0[1], SPA_TYPE_OBJECT);
        assert_eq!(pod.0[2], SPA_TYPE_OBJECT_FORMAT);
        assert_eq!(pod.0[3], SPA_PARAM_ENUM_FORMAT);

        let expected = [
            (SPA_FORMAT_MEDIA_TYPE, SPA_TYPE_ID, SPA_MEDIA_TYPE_AUDIO),
            (SPA_FORMAT_MEDIA_SUBTYPE, SPA_TYPE_ID, SPA_MEDIA_SUBTYPE_RAW),
            (
                SPA_FORMAT_AUDIO_FORMAT,
                SPA_TYPE_ID,
                SPA_AUDIO_FORMAT_F32_LE,
            ),
            (SPA_FORMAT_AUDIO_RATE, SPA_TYPE_INT, 44_100),
            (SPA_FORMAT_AUDIO_CHANNELS, SPA_TYPE_INT, 1),
        ];
        for (index, (key, kind, value)) in expected.into_iter().enumerate() {
            let at = 4 + index * PROPERTY_WORDS;
            assert_eq!(
                pod.0[at..at + PROPERTY_WORDS],
                [key, NO_FLAGS, VALUE_WORD, kind, value, PAD],
                "property {index}"
            );
        }
    }

    /// Every offset the callbacks read is a promise about a C struct, and getting one wrong reads a valid
    /// pointer out of the middle of another field. These are the layouts in `spa/buffer/buffer.h`.
    #[test]
    fn the_buffer_structs_have_the_layout_the_headers_describe() {
        assert_eq!(size_of::<SpaChunk>(), 16);
        assert_eq!(std::mem::offset_of!(SpaChunk, offset), 0);
        assert_eq!(std::mem::offset_of!(SpaChunk, size), 4);

        assert_eq!(size_of::<SpaData>(), 40);
        assert_eq!(std::mem::offset_of!(SpaData, mapoffset), 16);
        assert_eq!(std::mem::offset_of!(SpaData, maxsize), 20);
        assert_eq!(std::mem::offset_of!(SpaData, data), 24);
        assert_eq!(std::mem::offset_of!(SpaData, chunk), 32);

        // The one field order worth spelling out: both counts come before both pointers, which is not the
        // order a reader guesses.
        assert_eq!(std::mem::offset_of!(SpaBuffer, n_datas), 4);
        assert_eq!(std::mem::offset_of!(SpaBuffer, datas), 16);
    }
}
