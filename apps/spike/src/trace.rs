//! The protocol state a client's `WAYLAND_DEBUG` log describes, replayed from the client's side.
//!
//! What the compositor is told about a frame is double-buffered surface state: `attach`, `damage`/`damage_buffer` and `set_input_region` accumulate on a `wl_surface` and take effect together at its `commit`. So the unit this module produces is the [`Commit`] — everything one `wl_surface.commit` carried — tagged with which layer surface it belongs to (the namespace passed to `get_layer_surface`), the size of the buffer it shows and the logical size the compositor configured. Pointer button presses are kept alongside, attributed to the surface the pointer had entered, because that is the compositor's own answer to "which surface took this click".
//!
//! Object ids are recycled once the compositor acknowledges a destruction, so every `new id` starts that id's state over; nothing here assumes an id means the same object for the whole log.

use std::collections::HashMap;

use crate::geometry::{PxRect, PxRegion};
use crate::wire::{self, Arg, Direction, Message, Stamp};

/// A rect a commit declared damaged, in the space the request named.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Damage {
    /// `wl_surface.damage_buffer`: buffer pixels.
    Buffer(PxRect),
    /// `wl_surface.damage`: surface-local logical pixels. Mesa's and NVIDIA's WSI send `(0, 0, INT32_MAX, INT32_MAX)` here to mean "all of it".
    Surface(PxRect),
}

/// An input region as a commit declared it.
#[derive(Clone, Debug, PartialEq)]
pub enum InputRegion {
    /// `set_input_region(nil)`: the whole surface takes input.
    Everything,
    /// The region's rects in surface-local logical pixels. Empty means the surface is click-through.
    Rects(PxRegion),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Commit {
    pub stamp: Stamp,
    /// Index into [`Trace::surfaces`].
    pub surface: usize,
    /// A buffer was attached in this commit, which is what makes it a frame rather than a state change.
    pub attached: bool,
    /// The buffer showing once this commit lands, in buffer pixels, where its creation was in the log.
    pub buffer: Option<(i64, i64)>,
    /// The logical size last configured for the surface, or set as its viewport destination.
    pub logical: Option<(i64, i64)>,
    pub damage: Vec<Damage>,
    /// Set when this commit carried a `set_input_region`.
    pub input_region: Option<InputRegion>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Surface {
    pub id: u32,
    pub namespace: Option<String>,
}

/// A pointer button going down on one of this client's surfaces.
#[derive(Clone, Debug, PartialEq)]
pub struct Press {
    pub stamp: Stamp,
    /// Index into [`Trace::surfaces`]; `None` if the pointer had not entered any surface this log knows.
    pub surface: Option<usize>,
    pub x: f64,
    pub y: f64,
    pub button: u32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Trace {
    pub surfaces: Vec<Surface>,
    pub commits: Vec<Commit>,
    pub presses: Vec<Press>,
    /// Lines that parsed as protocol messages.
    pub messages: usize,
    /// Messages whose stamp is not wall-clock time and so cannot be placed on the spike's timeline.
    pub relative_stamps: usize,
}

impl Trace {
    pub fn namespace(&self, surface: usize) -> Option<&str> {
        self.surfaces.get(surface)?.namespace.as_deref()
    }
}

#[derive(Default)]
struct Pending {
    attach: Option<Option<u32>>,
    damage: Vec<Damage>,
    input_region: Option<InputRegion>,
}

struct Live {
    surface: usize,
    pending: Pending,
    buffer: Option<(i64, i64)>,
    logical: Option<(i64, i64)>,
}

#[derive(Default)]
struct Pointer {
    focus: Option<usize>,
    x: f64,
    y: f64,
}

#[derive(Default)]
struct Replay {
    trace: Trace,
    surfaces: HashMap<u32, Live>,
    regions: HashMap<u32, PxRegion>,
    buffers: HashMap<u32, (i64, i64)>,
    params: HashMap<u32, (i64, i64)>,
    // Each of these names a surface by the object it hangs off, so a configure or a viewport size reaches the right one.
    layer_surfaces: HashMap<u32, u32>,
    viewports: HashMap<u32, u32>,
    pointers: HashMap<u32, Pointer>,
}

/// Replays a whole log. Lines that are not protocol messages are skipped.
pub fn replay(log: &str) -> Trace {
    let mut replay = Replay::default();
    for line in log.lines() {
        let line = wire::strip_ansi(line);
        if let Some(message) = wire::parse(&line) {
            replay.apply(&message);
        }
    }
    replay.trace
}

fn rect(args: &[Arg<'_>]) -> Option<PxRect> {
    let [x, y, w, h] = [args.first()?, args.get(1)?, args.get(2)?, args.get(3)?].map(|a| a.int());
    Some(PxRect::new(x?, y?, w?, h?))
}

fn size(args: &[Arg<'_>], from: usize) -> Option<(i64, i64)> {
    Some((args.get(from)?.int()?, args.get(from + 1)?.int()?))
}

impl Replay {
    fn apply(&mut self, message: &Message<'_>) {
        self.trace.messages += 1;
        if matches!(message.stamp, Stamp::Relative(_)) {
            self.trace.relative_stamps += 1;
        }
        let id = message.target.id;
        let args = &message.args;
        match (message.direction, message.target.interface, message.name) {
            (Direction::Request, "wl_compositor", "create_surface") => {
                if let Some(new) = args.first().and_then(Arg::new_id) {
                    self.trace.surfaces.push(Surface {
                        id: new,
                        namespace: None,
                    });
                    self.surfaces.insert(
                        new,
                        Live {
                            surface: self.trace.surfaces.len() - 1,
                            pending: Pending::default(),
                            buffer: None,
                            logical: None,
                        },
                    );
                }
            }
            (Direction::Request, "wl_compositor", "create_region") => {
                if let Some(new) = args.first().and_then(Arg::new_id) {
                    self.regions.insert(new, PxRegion::default());
                }
            }
            (Direction::Request, "wl_region", "add") => {
                if let (Some(region), Some(r)) = (self.regions.get_mut(&id), rect(args)) {
                    region.add(r);
                }
            }
            (Direction::Request, "wl_region", "subtract") => {
                if let (Some(region), Some(r)) = (self.regions.get_mut(&id), rect(args)) {
                    region.subtract(r);
                }
            }
            (Direction::Request, "wl_region", "destroy") => {
                self.regions.remove(&id);
            }
            (Direction::Request, "wl_shm_pool", "create_buffer") => {
                if let (Some(new), Some(dims)) = (args.first().and_then(Arg::new_id), size(args, 2))
                {
                    self.buffers.insert(new, dims);
                }
            }
            (Direction::Request, "zwp_linux_buffer_params_v1", "create_immed") => {
                if let (Some(new), Some(dims)) = (args.first().and_then(Arg::new_id), size(args, 1))
                {
                    self.buffers.insert(new, dims);
                }
            }
            (Direction::Request, "zwp_linux_buffer_params_v1", "create") => {
                if let Some(dims) = size(args, 0) {
                    self.params.insert(id, dims);
                }
            }
            (Direction::Event, "zwp_linux_buffer_params_v1", "created") => {
                if let (Some(new), Some(dims)) =
                    (args.first().and_then(Arg::new_id), self.params.get(&id))
                {
                    self.buffers.insert(new, *dims);
                }
            }
            (Direction::Request, "wp_single_pixel_buffer_manager_v1", "create_u32_rgba_buffer") => {
                if let Some(new) = args.first().and_then(Arg::new_id) {
                    self.buffers.insert(new, (1, 1));
                }
            }
            (Direction::Request, "zwlr_layer_shell_v1", "get_layer_surface") => {
                let layer_surface = args.first().and_then(Arg::new_id);
                let surface = args.get(1).and_then(Arg::object).map(|o| o.id);
                if let (Some(layer_surface), Some(surface)) = (layer_surface, surface) {
                    self.layer_surfaces.insert(layer_surface, surface);
                    let namespace = args.get(4).and_then(Arg::str).map(str::to_owned);
                    if let Some(live) = self.surfaces.get(&surface) {
                        self.trace.surfaces[live.surface].namespace = namespace;
                    }
                }
            }
            (Direction::Event, "zwlr_layer_surface_v1", "configure") => {
                if let (Some(surface), Some(dims)) = (self.layer_surfaces.get(&id), size(args, 1))
                    && let Some(live) = self.surfaces.get_mut(surface)
                    && dims.0 > 0
                    && dims.1 > 0
                {
                    live.logical = Some(dims);
                }
            }
            (Direction::Request, "wp_viewporter", "get_viewport") => {
                let viewport = args.first().and_then(Arg::new_id);
                let surface = args.get(1).and_then(Arg::object).map(|o| o.id);
                if let (Some(viewport), Some(surface)) = (viewport, surface) {
                    self.viewports.insert(viewport, surface);
                }
            }
            (Direction::Request, "wp_viewport", "set_destination") => {
                if let (Some(surface), Some(dims)) = (self.viewports.get(&id), size(args, 0))
                    && let Some(live) = self.surfaces.get_mut(surface)
                    && dims.0 > 0
                    && dims.1 > 0
                {
                    live.logical = Some(dims);
                }
            }
            (Direction::Request, "wl_surface", name) => {
                self.surface_request(id, name, args, message.stamp)
            }
            (Direction::Request, "wl_seat", "get_pointer") => {
                if let Some(new) = args.first().and_then(Arg::new_id) {
                    self.pointers.insert(new, Pointer::default());
                }
            }
            (Direction::Event, "wl_pointer", name) => {
                self.pointer_event(id, name, args, message.stamp)
            }
            _ => {}
        }
    }

    fn surface_request(&mut self, id: u32, name: &str, args: &[Arg<'_>], stamp: Stamp) {
        let Some(live) = self.surfaces.get_mut(&id) else {
            return;
        };
        match name {
            "attach" => {
                live.pending.attach = Some(args.first().and_then(Arg::object).map(|o| o.id));
            }
            "damage" => live.pending.damage.extend(rect(args).map(Damage::Surface)),
            "damage_buffer" => live.pending.damage.extend(rect(args).map(Damage::Buffer)),
            "set_input_region" => {
                let region = match args.first() {
                    Some(Arg::Object(region)) => InputRegion::Rects(
                        self.regions.get(&region.id).cloned().unwrap_or_default(),
                    ),
                    _ => InputRegion::Everything,
                };
                live.pending.input_region = Some(region);
            }
            "commit" => {
                let pending = std::mem::take(&mut live.pending);
                if let Some(attached) = pending.attach {
                    live.buffer = attached.and_then(|buffer| self.buffers.get(&buffer).copied());
                }
                self.trace.commits.push(Commit {
                    stamp,
                    surface: live.surface,
                    attached: matches!(pending.attach, Some(Some(_))),
                    buffer: live.buffer,
                    logical: live.logical,
                    damage: pending.damage,
                    input_region: pending.input_region,
                });
            }
            "destroy" => {
                self.surfaces.remove(&id);
            }
            _ => {}
        }
    }

    fn pointer_event(&mut self, id: u32, name: &str, args: &[Arg<'_>], stamp: Stamp) {
        let Some(pointer) = self.pointers.get_mut(&id) else {
            return;
        };
        match name {
            "enter" => {
                let surface = args.get(1).and_then(Arg::object).map(|o| o.id);
                pointer.focus = surface
                    .and_then(|s| self.surfaces.get(&s))
                    .map(|live| live.surface);
                pointer.x = args.get(2).and_then(Arg::number).unwrap_or(0.0);
                pointer.y = args.get(3).and_then(Arg::number).unwrap_or(0.0);
            }
            "leave" => pointer.focus = None,
            "motion" => {
                pointer.x = args.get(1).and_then(Arg::number).unwrap_or(pointer.x);
                pointer.y = args.get(2).and_then(Arg::number).unwrap_or(pointer.y);
            }
            "button" if args.get(3).and_then(Arg::int) == Some(1) => {
                self.trace.presses.push(Press {
                    stamp,
                    surface: pointer.focus,
                    x: pointer.x,
                    y: pointer.y,
                    button: args.get(2).and_then(Arg::int).unwrap_or(0) as u32,
                });
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A software frame as the spike's transparent surfaces send it (telar's `WaylandAlphaPresenter`), preceded by the layer surface's creation and configure, in libwayland 1.26's format.
    const SOFTWARE: &str = "\
[10:00:00.000100]  -> wl_compositor#4.create_surface(new id wl_surface#25)
[10:00:00.000200]  -> zwlr_layer_shell_v1#9.get_layer_surface(new id zwlr_layer_surface_v1#26, wl_surface#25, nil, 2, \"hogar-shell-spike-top\")
[10:00:00.000300]  -> zwlr_layer_surface_v1#26.set_size(0, 0)
[10:00:00.000400]  -> wl_surface#25.commit()
[10:00:00.010000] {Default Queue} zwlr_layer_surface_v1#26.configure(4242, 1920, 1080)
[10:00:00.020000]  -> wl_shm#6.create_pool(new id wl_shm_pool#40, fd 17, 8294400)
[10:00:00.020100]  -> wl_shm_pool#40.create_buffer(new id wl_buffer#41, 0, 1920, 1080, 7680, 0)
[10:00:00.020200]  -> wl_surface#25.attach(wl_buffer#41, 0, 0)
[10:00:00.020300]  -> wl_surface#25.damage_buffer(0, 0, 1920, 1080)
[10:00:00.020400]  -> wl_surface#25.commit()
[10:00:01.000000]  -> wl_compositor#4.create_region(new id wl_region#50)
[10:00:01.000100]  -> wl_region#50.add(0, 0, 1920, 36)
[10:00:01.000200]  -> wl_region#50.add(0, 1044, 1920, 36)
[10:00:01.000300]  -> wl_surface#25.set_input_region(wl_region#50)
[10:00:01.000400]  -> wl_region#50.destroy()
[10:00:01.000500]  -> wl_surface#25.attach(wl_buffer#41, 0, 0)
[10:00:01.000600]  -> wl_surface#25.damage_buffer(912, 4, 96, 28)
[10:00:01.000700]  -> wl_surface#25.commit()
";

    #[test]
    fn a_commit_carries_what_was_attached_and_damaged_before_it() {
        let trace = replay(SOFTWARE);
        assert_eq!(trace.surfaces.len(), 1);
        assert_eq!(trace.namespace(0), Some("hogar-shell-spike-top"));
        assert_eq!(trace.commits.len(), 3);

        let initial = &trace.commits[0];
        assert!(
            !initial.attached,
            "the first commit only carries the layer state, and asks for a configure"
        );
        assert!(initial.damage.is_empty());

        let full = &trace.commits[1];
        assert!(full.attached);
        assert_eq!(full.buffer, Some((1920, 1080)));
        assert_eq!(full.logical, Some((1920, 1080)));
        assert_eq!(
            full.damage,
            vec![Damage::Buffer(PxRect::new(0, 0, 1920, 1080))]
        );

        let tick = &trace.commits[2];
        assert_eq!(tick.stamp, Stamp::TimeOfDay(36_001_000_700));
        assert_eq!(
            tick.damage,
            vec![Damage::Buffer(PxRect::new(912, 4, 96, 28))]
        );
        let Some(InputRegion::Rects(region)) = &tick.input_region else {
            panic!("the region set before this commit lands with it");
        };
        assert_eq!(region.rect_count(), 2);
        assert!(region.covers(PxRect::new(100, 1050, 50, 20)));
        assert!(!region.touches(PxRect::new(800, 500, 100, 100)));
    }

    #[test]
    fn a_region_is_copied_when_set_so_a_later_edit_does_not_reach_it() {
        let log = "\
[10:00:00.000100]  -> wl_compositor#4.create_surface(new id wl_surface#25)
[10:00:00.000200]  -> wl_compositor#4.create_region(new id wl_region#50)
[10:00:00.000300]  -> wl_region#50.add(0, 0, 10, 10)
[10:00:00.000400]  -> wl_surface#25.set_input_region(wl_region#50)
[10:00:00.000500]  -> wl_region#50.add(0, 0, 100, 100)
[10:00:00.000600]  -> wl_surface#25.commit()
[10:00:00.000700]  -> wl_surface#25.set_input_region(nil)
[10:00:00.000800]  -> wl_surface#25.commit()
";
        let trace = replay(log);
        let Some(InputRegion::Rects(region)) = &trace.commits[0].input_region else {
            panic!("expected a rect region");
        };
        assert_eq!(region.area(), 100);
        assert_eq!(trace.commits[1].input_region, Some(InputRegion::Everything));
    }

    #[test]
    fn hardware_frames_damage_in_surface_space_and_size_their_dmabufs() {
        let log = "\
[10:00:00.000100]  -> wl_compositor#4.create_surface(new id wl_surface#25)
[10:00:00.000200] {mesa vk display queue}  -> zwp_linux_dmabuf_v1#7.create_params(new id zwp_linux_buffer_params_v1#60)
[10:00:00.000300] {mesa vk display queue}  -> zwp_linux_buffer_params_v1#60.create_immed(new id wl_buffer#61, 3840, 2160, 875713089, 0)
[10:00:00.000400] {mesa vk display queue}  -> wl_surface#25.attach(wl_buffer#61, 0, 0)
[10:00:00.000500] {mesa vk display queue}  -> wl_surface#25.damage(0, 0, 2147483647, 2147483647)
[10:00:00.000600] {mesa vk display queue}  -> wl_surface#25.commit()
";
        let trace = replay(log);
        let frame = &trace.commits[0];
        assert!(frame.attached);
        assert_eq!(frame.buffer, Some((3840, 2160)));
        assert_eq!(
            frame.damage,
            vec![Damage::Surface(PxRect::new(
                0,
                0,
                2_147_483_647,
                2_147_483_647
            ))]
        );
    }

    #[test]
    fn a_recycled_id_is_a_new_surface() {
        let log = "\
[10:00:00.000100]  -> wl_compositor#4.create_surface(new id wl_surface#30)
[10:00:00.000200]  -> zwlr_layer_shell_v1#9.get_layer_surface(new id zwlr_layer_surface_v1#31, wl_surface#30, nil, 3, \"hogar-shell-spike-drawer\")
[10:00:00.000300]  -> wl_surface#30.destroy()
[10:00:00.000400] {Display Queue} wl_display#1.delete_id(30)
[10:00:00.000500]  -> wl_compositor#4.create_surface(new id wl_surface#30)
[10:00:00.000600]  -> zwlr_layer_shell_v1#9.get_layer_surface(new id zwlr_layer_surface_v1#31, wl_surface#30, nil, 3, \"hogar-shell-spike-stack\")
[10:00:00.000700]  -> wl_surface#30.commit()
";
        let trace = replay(log);
        assert_eq!(trace.surfaces.len(), 2);
        assert_eq!(trace.commits[0].surface, 1);
        assert_eq!(
            trace.namespace(trace.commits[0].surface),
            Some("hogar-shell-spike-stack")
        );
    }

    #[test]
    fn a_press_belongs_to_the_surface_the_pointer_entered() {
        let log = "\
[10:00:00.000100]  -> wl_compositor#4.create_surface(new id wl_surface#25)
[10:00:00.000200]  -> wl_compositor#4.create_surface(new id wl_surface#27)
[10:00:00.000300]  -> wl_seat#8.get_pointer(new id wl_pointer#12)
[10:00:01.000000] {Default Queue} wl_pointer#12.enter(90, wl_surface#25, 400.00000000, 20.00000000)
[10:00:01.000100] {Default Queue} wl_pointer#12.motion(1000, 410.50000000, 18.00000000)
[10:00:01.000200] {Default Queue} wl_pointer#12.button(91, 1001, 272, 1)
[10:00:01.000300] {Default Queue} wl_pointer#12.button(92, 1002, 272, 0)
[10:00:01.000400] {Default Queue} wl_pointer#12.leave(93, wl_surface#25)
[10:00:01.000500] {Default Queue} wl_pointer#12.enter(94, wl_surface#27, 800.00000000, 500.00000000)
[10:00:01.000600] {Default Queue} wl_pointer#12.button(95, 1003, 272, 1)
";
        let trace = replay(log);
        assert_eq!(trace.presses.len(), 2, "a release is not a press");
        assert_eq!(trace.presses[0].surface, Some(0));
        assert_eq!((trace.presses[0].x, trace.presses[0].y), (410.5, 18.0));
        assert_eq!(trace.presses[0].button, 272);
        assert_eq!(trace.presses[1].surface, Some(1));
        assert_eq!((trace.presses[1].x, trace.presses[1].y), (800.0, 500.0));
    }

    #[test]
    fn lines_from_anything_else_are_skipped() {
        let trace = replay("warning: something\n\nthread 'main' panicked\n");
        assert_eq!(trace.messages, 0);
        assert!(trace.commits.is_empty());
    }
}
