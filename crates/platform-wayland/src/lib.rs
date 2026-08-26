mod capture;
mod clipboard;
mod config;
mod gamma;
mod globals;
mod idle;
mod interest;
mod link;
mod lock;
mod placement;
mod platform;
mod power;
mod toplevel_control;
mod toplevels;
mod window;
mod workspaces;

pub use capture::{
    Backend as CaptureBackend, Capture, CaptureArea, CaptureError, IMAGE_COPY_CAPTURE_INTERFACES,
    SCREENCOPY_INTERFACES, TOPLEVEL_CAPTURE_INTERFACES, capture, capture_supported,
    capture_toplevel, toplevel_capture_supported,
};
pub use clipboard::{clipboard_supported, set_selection};
pub use config::{Anchor, KeyboardInteractivity, Layer, LayerConfig, OutputDescriptor};
pub use gamma::{
    GAMMA_INTERFACE, MAX_TEMPERATURE, MIN_TEMPERATURE, NEUTRAL_TEMPERATURE,
    current as current_temperature, gamma_supported, neutral as neutral_gamma, warm,
};
pub use globals::{advertises, advertises_all};
pub use idle::{IdleHandle, idle_notification, idle_supported};
pub use interest::Interest;
pub use link::SurfaceUpdate;
pub use lock::{LockHandle, lock_session, lock_supported, session_is_locked};
pub use placement::{
    KeyboardMode, SurfaceAlign, SurfaceAnchor, SurfacePlacement, SurfaceRole, SurfaceSize,
};
pub use platform::{
    EventSender, LayerShellPlatform, Subscription, SurfaceHandle, WatchToken, detached,
    enumerate_outputs, interval, live_surfaces, on_close, on_outputs_changed, open_reservation,
    open_surface, outputs, request_close, request_margin, request_size, run_on_start,
    set_surface_fonts, timeout, unwatch, watch,
};
pub use power::{
    OUTPUT_POWER_INTERFACE, output_power_on, output_power_supported, set_output_power,
};
pub use toplevel_control::{
    ManagedToplevel, ManagedToplevelId, TOPLEVEL_MANAGER_INTERFACE, close as close_toplevel,
    current as current_managed_toplevels, focus as focus_toplevel, focused as focused_toplevel,
    set_fullscreen as set_toplevel_fullscreen, set_maximized as set_toplevel_maximized,
    set_minimized as set_toplevel_minimized, toplevel_control_supported,
    watch as watch_managed_toplevels,
};
pub use toplevels::{
    TOPLEVEL_LIST_INTERFACE, Toplevel, ToplevelId, current as current_toplevels,
    toplevels_supported, watch as watch_toplevels,
};
pub use window::LayerWindow;
pub use workspaces::{
    WORKSPACE_INTERFACE, Workspace, WorkspaceId, activate as activate_workspace,
    current as current_workspaces, watch as watch_workspaces, workspaces_supported,
};
