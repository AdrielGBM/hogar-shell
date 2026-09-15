use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use raw_window_handle::{
    DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, RawDisplayHandle,
    RawWindowHandle, WaylandDisplayHandle, WaylandWindowHandle, WindowHandle,
};
use telar::Window;

#[derive(Clone)]
pub struct LayerWindow {
    inner: Arc<Inner>,
}

struct Inner {
    surface_ptr: NonNull<c_void>,
    display_ptr: NonNull<c_void>,
    width: AtomicU32,
    height: AtomicU32,
    // scale_factor × 1000, so it fits an atomic (f64 has no atomic).
    scale_milli: AtomicU32,
    request_redraw: Box<dyn Fn() + Send + Sync>,
}

// SAFETY: the pointers are libwayland `wl_surface`/`wl_display` handles; libwayland's client library is internally synchronized for concurrent request submission, which is the only cross-thread use here (the wgpu render thread submits present requests while the worker thread runs the event loop). No wl_* event *reading* happens off the worker thread.
unsafe impl Send for Inner {}
unsafe impl Sync for Inner {}

impl LayerWindow {
    pub fn new(
        surface_ptr: NonNull<c_void>,
        display_ptr: NonNull<c_void>,
        width: u32,
        height: u32,
        scale_factor: f64,
        request_redraw: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                surface_ptr,
                display_ptr,
                width: AtomicU32::new(width),
                height: AtomicU32::new(height),
                scale_milli: AtomicU32::new((scale_factor * 1000.0) as u32),
                request_redraw: Box::new(request_redraw),
            }),
        }
    }

    pub fn set_size(&self, width: u32, height: u32) {
        self.inner.width.store(width, Ordering::Relaxed);
        self.inner.height.store(height, Ordering::Relaxed);
    }

    pub fn set_scale_factor(&self, scale_factor: f64) {
        self.inner
            .scale_milli
            .store((scale_factor * 1000.0) as u32, Ordering::Relaxed);
    }
}

impl HasWindowHandle for LayerWindow {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        let raw = RawWindowHandle::Wayland(WaylandWindowHandle::new(self.inner.surface_ptr));
        // SAFETY: the wl_surface outlives this window (the surface's thread owns both and drops the window before the surface), and the handle is only borrowed for the returned lifetime.
        Ok(unsafe { WindowHandle::borrow_raw(raw) })
    }
}

impl HasDisplayHandle for LayerWindow {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        let raw = RawDisplayHandle::Wayland(WaylandDisplayHandle::new(self.inner.display_ptr));
        // SAFETY: the wl_display outlives this window (owned by the same thread), borrowed only for the lifetime.
        Ok(unsafe { DisplayHandle::borrow_raw(raw) })
    }
}

impl Window for LayerWindow {
    fn width(&self) -> u32 {
        self.inner.width.load(Ordering::Relaxed)
    }
    fn height(&self) -> u32 {
        self.inner.height.load(Ordering::Relaxed)
    }
    fn request_redraw(&self) {
        (self.inner.request_redraw)();
    }
    fn scale_factor(&self) -> f64 {
        self.inner.scale_milli.load(Ordering::Relaxed) as f64 / 1000.0
    }
    fn is_offscreen(&self) -> bool {
        false
    }
    fn retains_presented_contents(&self) -> bool {
        true
    }
}
