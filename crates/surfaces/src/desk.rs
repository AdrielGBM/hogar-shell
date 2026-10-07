//! What each screen's wallpaper answers to besides its picture ([`services::wallpaper::Desk`]), kept current from the compositor's workspace and window feeds while a region follows it.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use platform_wayland::{EventSender, Interest};
use services::wallpaper::{self, Desk};
use telar::{RwSignal, detached, signal};

type Unwatch = Box<dyn FnOnce()>;

/// Where the desks are read from and what says they may have moved: the compositor's, except in a test.
#[derive(Clone, Copy)]
struct Feeds {
    watch: fn() -> Vec<Unwatch>,
    read: fn(Option<&str>) -> Desk,
}

impl Feeds {
    const LIVE: Self = Self {
        watch: watch_live,
        read: wallpaper::desk_now,
    };
}

thread_local! {
    static DESKS: RwSignal<HashMap<Option<String>, Desk>> = detached(|| signal(HashMap::new()));
    static FOLLOWERS: RefCell<HashMap<Option<String>, usize>> = RefCell::new(HashMap::new());
    static WATCHES: RefCell<Vec<Unwatch>> = const { RefCell::new(Vec::new()) };
    static FEEDS: Cell<Feeds> = const { Cell::new(Feeds::LIVE) };
}

/// The desk of `output`, read as a signal: what reads it follows windows opening and workspaces changing.
pub fn of(output: Option<&str>) -> Desk {
    let key = output.map(str::to_string);
    DESKS.with(|desks| desks.with(|held| held.get(&key).copied().unwrap_or_default()))
}

pub fn now(output: Option<&str>) -> Desk {
    let key = output.map(str::to_string);
    DESKS.with(|desks| desks.peek_with(|held| held.get(&key).copied().unwrap_or_default()))
}

pub fn show(output: Option<&str>, desk: Desk) {
    let key = output.map(str::to_string);
    DESKS.with(|desks| {
        if desks.peek_with(|held| held.get(&key) != Some(&desk)) {
            desks.update(|held| {
                held.insert(key, desk);
            });
        }
    });
}

/// Reads `output`'s desk now and keeps it current on each workspace and window change for as long as the owner calling this lives; the feeds are watched while anything follows a desk and dropped once nothing does.
pub fn follow(output: Option<&str>) {
    let key = output.map(str::to_string);
    let feeds = FEEDS.with(Cell::get);
    let newly = FOLLOWERS.with(|followers| {
        let mut followers = followers.borrow_mut();
        let count = followers.entry(key.clone()).or_default();
        *count += 1;
        *count == 1
    });
    if newly && !DESKS.with(|desks| desks.peek_with(|held| held.contains_key(&key))) {
        show(output, (feeds.read)(output));
    }
    if WATCHES.with(|watches| watches.borrow().is_empty()) {
        let started = (feeds.watch)();
        WATCHES.with(|watches| *watches.borrow_mut() = started);
    }
    telar::on_cleanup(move || unfollow(key));
}

fn unfollow(key: Option<String>) {
    let (last_here, none_left) = FOLLOWERS.with(|followers| {
        let mut followers = followers.borrow_mut();
        let last_here = match followers.get_mut(&key) {
            Some(count) if *count > 1 => {
                *count -= 1;
                false
            }
            Some(_) => {
                followers.remove(&key);
                true
            }
            None => false,
        };
        (last_here, followers.is_empty())
    });
    if last_here {
        DESKS.with(|desks| {
            if desks.peek_with(|held| held.contains_key(&key)) {
                desks.update(|held| {
                    held.remove(&key);
                });
            }
        });
    }
    if none_left {
        for unwatch in WATCHES.with(RefCell::take) {
            unwatch();
        }
    }
}

fn refresh() {
    let read = FEEDS.with(Cell::get).read;
    let followed: Vec<Option<String>> =
        FOLLOWERS.with(|followers| followers.borrow().keys().cloned().collect());
    for output in followed {
        show(output.as_deref(), read(output.as_deref()));
    }
}

fn watch_live() -> Vec<Unwatch> {
    [
        platform_wayland::app_watch(services::hyprland::subscribe, |_| refresh()),
        platform_wayland::app_watch(services::windows::subscribe, |_| refresh()),
        platform_wayland::app_watch(workspaces_moved, |()| refresh()),
    ]
    .into_iter()
    .flatten()
    .map(|token| Box::new(move || platform_wayland::unwatch(token)) as Unwatch)
    .collect()
}

/// A tick on every change `ext-workspace-v1` publishes: what [`wallpaper::desk_now`] reads the workspace up from on any compositor, Hyprland or not.
fn workspaces_moved(tx: EventSender<()>) {
    let interest = Interest::new();
    let owned = interest.clone();
    platform_wayland::watch_workspaces(&interest, move |_| {
        if !tx.send(()) {
            owned.retire();
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    thread_local! {
        static WATCHED: Cell<usize> = const { Cell::new(0) };
        static UNWATCHED: Cell<usize> = const { Cell::new(0) };
        static LIVE_DESK: Cell<Desk> = Cell::new(Desk::default());
    }

    fn fake_watch() -> Vec<Unwatch> {
        WATCHED.with(|watched| watched.set(watched.get() + 1));
        ["workspaces", "windows"]
            .into_iter()
            .map(|_| {
                Box::new(|| UNWATCHED.with(|unwatched| unwatched.set(unwatched.get() + 1)))
                    as Unwatch
            })
            .collect()
    }

    fn fake_read(_: Option<&str>) -> Desk {
        LIVE_DESK.with(Cell::get)
    }

    fn compositor_shows(desk: Desk) {
        LIVE_DESK.with(|live| live.set(desk));
        refresh();
    }

    /// A switch to an empty workspace reaches every followed desk through the feeds, which are watched once however many follow and dropped with the last follower.
    #[test]
    fn a_followed_desk_tracks_the_feeds_and_the_feeds_go_with_the_last_follower() {
        FEEDS.with(|feeds| {
            feeds.set(Feeds {
                watch: fake_watch,
                read: fake_read,
            })
        });
        let empty_on_the_right = Desk {
            covered: false,
            along: Some(1.0),
        };
        compositor_shows(Desk {
            covered: true,
            along: Some(0.0),
        });

        let first = telar::owner_scope();
        follow(Some("DP-1"));
        let first_owner = first.id();
        drop(first);
        let second = telar::owner_scope();
        follow(Some("DP-1"));
        let second_owner = second.id();
        drop(second);
        assert_eq!(
            WATCHED.with(Cell::get),
            1,
            "one set of watches for every follower"
        );
        assert_eq!(now(Some("DP-1")).along, Some(0.0));

        compositor_shows(empty_on_the_right);
        assert_eq!(now(Some("DP-1")), empty_on_the_right);
        assert_eq!(
            now(Some("HDMI-A-1")),
            Desk::default(),
            "nothing follows this one"
        );

        telar::dispose_owner(first_owner);
        assert_eq!(UNWATCHED.with(Cell::get), 0, "a desk is still followed");
        assert_eq!(now(Some("DP-1")), empty_on_the_right);

        telar::dispose_owner(second_owner);
        assert_eq!(UNWATCHED.with(Cell::get), 2, "every watch is dropped");
        assert_eq!(
            now(Some("DP-1")),
            Desk::default(),
            "and the desk is forgotten"
        );

        compositor_shows(Desk {
            covered: true,
            along: Some(0.5),
        });
        assert_eq!(
            now(Some("DP-1")),
            Desk::default(),
            "nothing refreshes it now"
        );

        let again = telar::owner_scope();
        follow(Some("DP-1"));
        let again_owner = again.id();
        drop(again);
        assert_eq!(WATCHED.with(Cell::get), 2, "a new follower watches afresh");
        assert_eq!(now(Some("DP-1")).along, Some(0.5));
        telar::dispose_owner(again_owner);
    }
}
