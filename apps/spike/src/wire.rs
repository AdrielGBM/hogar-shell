//! One line of libwayland's `WAYLAND_DEBUG` output, parsed.
//!
//! The spike talks to the compositor through `libwayland-client` (wayland-rs's `client_system` backend, which both the platform crate and the software presenter use), so every request it sends and every event it dispatches is printed to stderr by libwayland's own `wl_closure_print`. The format is libwayland's, not wayland-rs's, and it changed in 1.23 — this machine's 1.26 prints:
//!
//! ```text
//! [HH:MM:SS.uuuuuu] [TID#n ][{queue} ][discarded ][ -> ]interface#id.message(arg, arg, …)
//! ```
//!
//! with the stamp taken from `CLOCK_REALTIME` (UTC time of day, microseconds), ` -> ` marking a request the client sent and its absence an event it received, objects printed `interface#id`, a new object `new id interface#id`, strings quoted and unescaped, fixed-point numbers with eight decimals, and `nil` for a null object or string. The colour codes it adds when stderr is a terminal (or `FORCE_COLOR` is set) are stripped by [`strip_ansi`].
//!
//! Older libwayland (≤ 1.22) and wayland-rs's pure-Rust backend print `interface@id` and a stamp of `[ms.µs]` from a wrapping microsecond counter instead; both are accepted, but their stamps are [`Stamp::Relative`] and cannot be lined up against the spike's wall-clock timeline.

use std::borrow::Cow;

/// When libwayland printed a line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stamp {
    /// Microseconds since midnight UTC, from `CLOCK_REALTIME` — libwayland 1.23 and later.
    TimeOfDay(u64),
    /// A microsecond counter that wraps every ~71 minutes and is anchored to nothing — libwayland 1.22 and earlier, and wayland-rs's own printer.
    Relative(u64),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    /// Sent by this client (` -> `).
    Request,
    /// Received and dispatched by this client.
    Event,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ObjectRef<'a> {
    pub interface: &'a str,
    pub id: u32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Arg<'a> {
    Int(i64),
    Fixed(f64),
    Str(&'a str),
    /// A null object or string.
    Nil,
    Object(ObjectRef<'a>),
    /// A newly created object; `id` is `None` where libwayland printed `nil` for it.
    NewId {
        interface: &'a str,
        id: Option<u32>,
    },
    Array(usize),
    Fd(i32),
}

impl<'a> Arg<'a> {
    pub fn int(&self) -> Option<i64> {
        match *self {
            Arg::Int(v) => Some(v),
            _ => None,
        }
    }

    /// A number, whichever way it was printed: a `wl_fixed_t` coordinate and an integer size are both lengths to the reader.
    pub fn number(&self) -> Option<f64> {
        match *self {
            Arg::Int(v) => Some(v as f64),
            Arg::Fixed(v) => Some(v),
            _ => None,
        }
    }

    pub fn object(&self) -> Option<ObjectRef<'a>> {
        match *self {
            Arg::Object(o) => Some(o),
            _ => None,
        }
    }

    pub fn new_id(&self) -> Option<u32> {
        match *self {
            Arg::NewId { id, .. } => id,
            _ => None,
        }
    }

    pub fn str(&self) -> Option<&'a str> {
        match *self {
            Arg::Str(s) => Some(s),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Message<'a> {
    pub stamp: Stamp,
    pub direction: Direction,
    pub target: ObjectRef<'a>,
    pub name: &'a str,
    pub args: Vec<Arg<'a>>,
}

/// Removes the SGR colour sequences libwayland wraps each field in when it believes it is writing to a terminal.
pub fn strip_ansi(line: &str) -> Cow<'_, str> {
    if !line.contains('\x1b') {
        return Cow::Borrowed(line);
    }
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            for end in chars.by_ref() {
                if end.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    Cow::Owned(out)
}

/// Parses one line, or answers `None` for anything that is not a request or event libwayland dispatched — another program's stderr, a panic message, or a `discarded` line for a message that never reached its object.
pub fn parse(line: &str) -> Option<Message<'_>> {
    let line = line.trim_end();
    let rest = line.strip_prefix('[')?;
    let (stamp, rest) = rest.split_once(']')?;
    let stamp = parse_stamp(stamp)?;
    let mut rest = rest.strip_prefix("[rs]").unwrap_or(rest).trim_start();
    if let Some(after) = rest.strip_prefix("TID#") {
        rest = after.split_once(' ')?.1.trim_start();
    }
    if let Some(after) = rest.strip_prefix('{') {
        rest = after.split_once('}')?.1.trim_start();
    }
    if rest.starts_with("discarded") || rest.starts_with("[discarded]") {
        return None;
    }
    let (direction, rest) = match rest.strip_prefix("->") {
        Some(after) => (Direction::Request, after.trim_start()),
        None => (Direction::Event, rest),
    };
    let open = rest.find('(')?;
    let (head, args) = (&rest[..open], &rest[open + 1..]);
    let (object, name) = head.rsplit_once('.')?;
    let target = parse_object(object)?;
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return None;
    }
    Some(Message {
        stamp,
        direction,
        target,
        name,
        args: parse_args(args)?,
    })
}

fn parse_stamp(text: &str) -> Option<Stamp> {
    if let [h, m, s] = text.split(':').collect::<Vec<_>>()[..] {
        let (secs, micros) = s.split_once('.')?;
        let (h, m, secs): (u64, u64, u64) = (h.parse().ok()?, m.parse().ok()?, secs.parse().ok()?);
        let micros: u64 = micros.parse().ok()?;
        return Some(Stamp::TimeOfDay(
            ((h * 60 + m) * 60 + secs) * 1_000_000 + micros,
        ));
    }
    let (millis, frac) = text.trim().split_once('.')?;
    let millis: u64 = millis.parse().ok()?;
    let frac: u64 = frac.parse().ok()?;
    Some(Stamp::Relative(millis * 1000 + frac))
}

fn parse_object(text: &str) -> Option<ObjectRef<'_>> {
    let split = text.rfind(['#', '@'])?;
    let (interface, id) = (&text[..split], &text[split + 1..]);
    if interface.is_empty() {
        return None;
    }
    Some(ObjectRef {
        interface,
        id: id.parse().ok()?,
    })
}

fn parse_args(mut rest: &str) -> Option<Vec<Arg<'_>>> {
    let mut args = Vec::new();
    loop {
        rest = rest.trim_start();
        if let Some(after) = rest.strip_prefix(')') {
            return after.trim().is_empty().then_some(args);
        }
        let (arg, after) = if let Some(quoted) = rest.strip_prefix('"') {
            let close = quoted.find('"')?;
            (Arg::Str(&quoted[..close]), &quoted[close + 1..])
        } else {
            let end = rest.find([',', ')'])?;
            (parse_token(rest[..end].trim())?, &rest[end..])
        };
        args.push(arg);
        rest = after.trim_start();
        rest = rest.strip_prefix(',').unwrap_or(rest);
    }
}

fn parse_token(token: &str) -> Option<Arg<'_>> {
    if token == "nil" || token == "null" {
        return Some(Arg::Nil);
    }
    if let Some(created) = token.strip_prefix("new id ") {
        let split = created.rfind(['#', '@'])?;
        let id = &created[split + 1..];
        return Some(Arg::NewId {
            interface: &created[..split],
            id: if id == "nil" {
                None
            } else {
                Some(id.parse().ok()?)
            },
        });
    }
    if let Some(fd) = token.strip_prefix("fd ") {
        return fd.parse().ok().map(Arg::Fd);
    }
    if let Some(array) = token.strip_prefix("array[") {
        let len = array.split(|c: char| !c.is_ascii_digit()).next()?;
        return len.parse().ok().map(Arg::Array);
    }
    if token.starts_with(|c: char| c.is_ascii_digit() || c == '-') {
        return if token.contains('.') {
            token.parse().ok().map(Arg::Fixed)
        } else {
            token.parse().ok().map(Arg::Int)
        };
    }
    parse_object(token).map(Arg::Object)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obj(interface: &str, id: u32) -> ObjectRef<'_> {
        ObjectRef { interface, id }
    }

    #[test]
    fn a_request_without_a_queue_name() {
        let msg =
            parse("[09:31:07.123456]  -> wl_surface#25.damage_buffer(0, 36, 1920, 40)").unwrap();
        assert_eq!(
            msg.stamp,
            Stamp::TimeOfDay(((9 * 60 + 31) * 60 + 7) * 1_000_000 + 123_456)
        );
        assert_eq!(msg.direction, Direction::Request);
        assert_eq!(msg.target, obj("wl_surface", 25));
        assert_eq!(msg.name, "damage_buffer");
        assert_eq!(
            msg.args,
            vec![Arg::Int(0), Arg::Int(36), Arg::Int(1920), Arg::Int(40)]
        );
    }

    #[test]
    fn a_request_on_a_named_queue() {
        let msg = parse("[23:59:59.000001] {Default Queue}  -> wl_surface#25.commit()").unwrap();
        assert_eq!(msg.stamp, Stamp::TimeOfDay(86_399_000_001));
        assert_eq!(
            (msg.direction, msg.target, msg.name),
            (Direction::Request, obj("wl_surface", 25), "commit")
        );
        assert!(msg.args.is_empty());
    }

    #[test]
    fn an_event_with_a_thread_id_and_a_queue() {
        let msg =
            parse("[10:00:00.000000] TID#4242 {Display Queue} wl_display#1.delete_id(31)").unwrap();
        assert_eq!(msg.direction, Direction::Event);
        assert_eq!(msg.target, obj("wl_display", 1));
        assert_eq!(msg.args, vec![Arg::Int(31)]);
    }

    #[test]
    fn a_new_object_a_null_output_and_a_string() {
        let msg = parse(
            "[10:00:00.000000]  -> zwlr_layer_shell_v1#9.get_layer_surface(new id zwlr_layer_surface_v1#26, wl_surface#25, nil, 2, \"hogar-shell-spike-top\")",
        )
        .unwrap();
        assert_eq!(
            msg.args,
            vec![
                Arg::NewId {
                    interface: "zwlr_layer_surface_v1",
                    id: Some(26)
                },
                Arg::Object(obj("wl_surface", 25)),
                Arg::Nil,
                Arg::Int(2),
                Arg::Str("hogar-shell-spike-top"),
            ]
        );
    }

    #[test]
    fn fixed_point_coordinates_and_negative_numbers() {
        let msg = parse("[10:00:00.000000] {Default Queue} wl_pointer#12.enter(77, wl_surface#25, 812.50000000, -3.00000000)").unwrap();
        assert_eq!(msg.args[2], Arg::Fixed(812.5));
        assert_eq!(msg.args[3], Arg::Fixed(-3.0));
        assert_eq!(msg.args[1].object(), Some(obj("wl_surface", 25)));
    }

    #[test]
    fn a_string_may_hold_commas_and_parentheses() {
        let msg = parse("[10:00:00.000000]  -> xdg_toplevel#30.set_title(\"a, (b)\")").unwrap();
        assert_eq!(msg.args, vec![Arg::Str("a, (b)")]);
    }

    #[test]
    fn fds_arrays_and_unnamed_new_ids() {
        let msg = parse(
            "[10:00:00.000000]  -> wl_shm#6.create_pool(new id wl_shm_pool#40, fd 17, 33177600)",
        )
        .unwrap();
        assert_eq!(msg.args[1], Arg::Fd(17));
        let msg =
            parse("[10:00:00.000000] wl_keyboard#13.enter(5, wl_surface#25, array[8])").unwrap();
        assert_eq!(msg.args[2], Arg::Array(8));
        let msg = parse(
            "[10:00:00.000000]  -> wl_registry#2.bind(3, \"wl_seat\", 7, new id [unknown]#11)",
        )
        .unwrap();
        assert_eq!(
            msg.args[3],
            Arg::NewId {
                interface: "[unknown]",
                id: Some(11)
            }
        );
    }

    #[test]
    fn colour_codes_are_stripped_before_parsing() {
        let coloured = "\x1b[32m[10:00:00.000000] \x1b[31m\x1b[0m -> \x1b[34mwl_surface\x1b[35m#25\x1b[36m.commit\x1b[0m()\x1b[0m";
        let clean = strip_ansi(coloured);
        let msg = parse(&clean).unwrap();
        assert_eq!(
            (msg.direction, msg.target, msg.name),
            (Direction::Request, obj("wl_surface", 25), "commit")
        );
    }

    #[test]
    fn the_pre_1_23_and_wayland_rs_formats_parse_with_relative_stamps() {
        let old = parse("[3781234.567]  -> wl_surface@25.damage_buffer(0, 0, 10, 10)").unwrap();
        assert_eq!(old.stamp, Stamp::Relative(3_781_234_567));
        assert_eq!(old.target, obj("wl_surface", 25));
        let rs = parse("[3781234.567][rs] -> wl_surface@25.attach(wl_buffer@41, 0, 0)").unwrap();
        assert_eq!(rs.args[0], Arg::Object(obj("wl_buffer", 41)));
    }

    #[test]
    fn anything_else_is_not_a_message() {
        assert_eq!(parse("thread 'main' panicked at src/main.rs:1:1:"), None);
        assert_eq!(parse(""), None);
        assert_eq!(
            parse("[10:00:00.000000] discarded [zombie]#31.[event 0](0 fd, 12 byte)"),
            None
        );
        assert_eq!(
            parse("[10:00:00.000000] discarded  -> wl_surface#25.commit()"),
            None
        );
        assert_eq!(
            parse("[10:00:00.000000]  -> wl_surface#25.commit("),
            None,
            "a truncated line is dropped, not guessed at"
        );
    }
}
