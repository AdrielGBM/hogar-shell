//! The column of cards the shell pins to a screen edge and takes away again.
//!
//! **Three surfaces became one.** A notification popup, an in-shell toast and the OSD a volume change flashes were three stacks with three `edge`s, three widths and three timeouts, and being three is what let them open in three different places, overlap each other on a narrow screen, and have no one of them able to know. They are one column now, in arrival order, and where it sits is `[stack]`.
//!
//! What stayed apart is what is actually different. Each card is still built by the module that owns it — a notification by the card with its actions and its swipe, a toast by its own, an OSD by its meter — and each still comes from its own source. This merges them; it knows nothing about what any of them mean.
//!
//! **Only two things vary per card**, and both used to be a whole config section each:
//!
//! - **Its key**, so a second reading about the same thing replaces the first in place rather than pushing a copy underneath it. A wheel spun ten notches is one OSD, not ten.
//! - **Whether it expires**, which is not a second timeout: a `critical` notification under `critical_sticky` waits to be dealt with, and everything else goes at `[stack] timeout_ms`. Each source still times its own cards out — the daemon its notifications, the toaster its toasts — because the one thing they cannot rely on is a surface being up to do it for them.
//!
//! **Every card answers to the same gesture.** Dragged aside it goes; pressed, it does whatever it is *for* — a notification runs its action, and a toast and an OSD, which are reports rather than offers, do nothing. Taking the pointer was the third thing that varied, and no longer does: the rule used to be that an OSD must never be in the way of the click behind it, which made it the one card the user could not get rid of and made the column behave like three surfaces again depending on which card was on top. What that bought — a click passing through the OSD to the window underneath — was real, and is the price of this. `[stack] clear_threshold` switches the gesture off for every card if the trade is the wrong one.
//!
//! Only a notification carries a ✕, and only because it is the one card with somewhere else to be: swiping it puts it in the history, so there has to be a way to say "not there either". A toast and an OSD have no history to be kept out of.
//!
//! The surface exists only while the column has something in it — opened on the first card, dropped with the last — so an idle session carries no overlay at all. That is the toast host's old rule applied to the notification popup too, which used to stay mapped around the clock because the daemon owns the timing.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::os::unix::net::UnixStream;
use std::sync::Arc;

use platform_wayland::{SurfaceHandle, timeout, watch};
use telar::{LayoutError, LayoutItem, ReactiveList, signal, use_theme};

use config::theme::NordTheme;
use config::{AnimationConfig, Config, StackConfig};
use services::notifications::{Notification, SharedSnapshot, Snapshot};
use services::toaster::{self, Toast};
use ui::panel::{PanelSurface, card_gap, content_radius};
use ui::placement::Placement;
use util::broadcast::Store;

use crate::osd::OsdKind;

pub(crate) mod swipe;
pub(crate) mod transition;

const NAMESPACE: &str = "hogar-shell-stack";

/// The least the surface will ask for, when the compositor has not reported its output's size yet — one card's worth, so a column that opens before the first `wl_output` event is small rather than absent.
const MIN_HEIGHT: u32 = 132;

/// One card in the column, and the module that owns it.
///
/// An enum rather than a boxed builder because the list keys, orders and compares cards without building them: a reactive list rebuilds only the rows whose key changed, and a closure is not comparable.
#[derive(Clone)]
pub enum Card {
    Notification(Notification),
    Toast(Toast),
    /// The reading is not carried: the card subscribes to the service and follows the level while it is up, which is the whole point of an OSD — one frozen at the value it opened with would be worse than none.
    Osd(OsdKind),
}

impl Card {
    /// What the list keys on: the same key twice is the same card, redrawn in its slot rather than added under the one already there.
    fn key(&self) -> String {
        match self {
            Card::Notification(n) => {
                format!("notification\u{1}{}", crate::notifications::card_key(n))
            }
            Card::Toast(t) => format!("toast\u{1}{}", t.key()),
            Card::Osd(kind) => format!("osd\u{1}{}", kind.id()),
        }
    }

    /// What arrival is stamped against — the key with the *contents* left out, so a card replaced by a newer one about the same thing keeps the place it already had instead of dropping to the bottom of the column.
    fn slot(&self) -> String {
        match self {
            Card::Notification(n) => format!("notification\u{1}{}", n.id),
            Card::Toast(t) => format!("toast\u{1}{:?}", t.event),
            Card::Osd(kind) => format!("osd\u{1}{}", kind.id()),
        }
    }

    /// What decides a card's place beyond when it arrived: a `critical` notification goes where it will be read rather than where it happened to land.
    fn urgent(&self) -> bool {
        matches!(self, Card::Notification(n) if crate::notifications::is_critical(n))
    }

    /// Who is speaking. Every provider is guaranteed a card on screen — see [`admit`].
    fn provider(&self) -> &'static str {
        match self {
            Card::Notification(_) => "notification",
            Card::Toast(_) => "toast",
            Card::Osd(_) => "osd",
        }
    }
}

/// The single-slot OSD, as a source the column can subscribe to like the other two.
///
/// A store rather than a signal because the surface it feeds is opened and dropped as the column fills and empties, and a signal made inside a surface goes with it.
static OSD: Store<Option<OsdKind>> = Store::new(|| None);

/// Bumped when a card's exit has finished, so the column re-runs and stops drawing it.
///
/// A `Store` for the same reason [`OSD`] is one: the surface it wakes is opened and dropped as the column fills and empties, and a signal made inside a surface goes with it.
static DEPARTURES: Store<u64> = Store::new(|| 0);

thread_local! {
    /// Bumped on every OSD trigger, so the expiry scheduled by the one that was replaced fires against a generation that no longer matches and does nothing. Cheaper than cancelling a timer, and the same arbitration the hover popout uses.
    static OSD_GENERATION: Cell<u64> = const { Cell::new(0) };
    /// Every slot the sources held last pass, so the next one can tell what has gone. Taken before the cap: a card queued behind `admit` is still alive, and treating it as gone would animate away a card that stayed.
    static LAST_ALIVE: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    /// The last card each slot held and where it was drawn, kept so a departing one can still be drawn — in its own place — after every source has dropped it. Pruned when its exit finishes.
    static DEPARTED: RefCell<HashMap<String, (usize, Card)>> = RefCell::new(HashMap::new());
    static ARRIVALS: RefCell<Arrivals> = RefCell::new(Arrivals::default());
    /// What the column is holding, as the host last saw it. The host has to answer "is there anything to show" while there is no surface to ask it on, and the daemon publishes rather than answers.
    static LIVE: RefCell<Live> = RefCell::new(Live::default());
    static OPEN: RefCell<Option<Stack>> = const { RefCell::new(None) };
}

/// Takes the OSD off the column, for the swipe that dismisses it. The slot is this module's, so clearing it is too — the OSD card itself only knows that it was dragged aside.
pub(crate) fn clear_osd() {
    OSD.update(|slot| *slot = None);
}

/// Shows `kind`'s OSD, replacing whatever OSD was up, and schedules it away after `[stack] timeout_ms`.
///
/// Replacing rather than stacking is the OSD's own rule and always was: a user spinning the volume wheel is saying one thing repeatedly, not ten things.
pub fn show_osd(kind: OsdKind) {
    OSD.update(|slot| *slot = Some(kind));
    let generation = OSD_GENERATION.with(|g| {
        let next = g.get().wrapping_add(1);
        g.set(next);
        next
    });
    let after = config::config()
        .map(|c| c.stack.lifetime())
        .unwrap_or_else(|| StackConfig::default().lifetime());
    timeout(after, move || {
        if OSD_GENERATION.with(Cell::get) == generation {
            OSD.update(|slot| *slot = None);
        }
    });
}

/// The order the column is drawn in: what arrived first is nearest the edge it grows from, and a `critical` notification is above all of it.
///
/// Arrival is stamped here rather than carried on the cards, because none of the three sources can supply an ordinal the other two can be compared against — the toaster's ids, the daemon's ids and a single OSD slot are three counters that know nothing of each other. What the column *can* see is which slots it had last time.
#[derive(Default)]
struct Arrivals {
    seen: HashMap<String, u64>,
    next: u64,
}

impl Arrivals {
    fn order(&mut self, mut cards: Vec<Card>) -> Vec<Card> {
        let live: Vec<String> = cards.iter().map(Card::slot).collect();
        for slot in &live {
            if !self.seen.contains_key(slot) {
                self.next = self.next.wrapping_add(1);
                self.seen.insert(slot.clone(), self.next);
            }
        }
        // A slot that has gone is forgotten, so the same thing arriving again is a new arrival rather than one that keeps a place it earned an hour ago.
        self.seen.retain(|slot, _| live.contains(slot));
        let at = |card: &Card| self.seen.get(&card.slot()).copied().unwrap_or_default();
        cards.sort_by_key(|card| (!card.urgent(), at(card)));
        cards
    }
}

/// What the three sources are saying, kept so the host can decide whether there is a column at all.
#[derive(Default, Clone)]
struct Live {
    snapshot: Arc<Snapshot>,
    toasts: Vec<Toast>,
    osd: Option<OsdKind>,
}

/// Whether the column has anything at all — the only question the host asks, and deliberately not [`column`].
///
/// **Ordering is a side effect, and only the surface may cause it.** [`Arrivals`] stamps what is new and forgets what has gone, so a caller running it against a *different* view of the same sources re-stamps cards the other one can still see — and a re-stamped card is a card that jumps to the bottom of the column. The host and the surface each hold their own copies of three live sources and update on their own schedules, so they disagree constantly for a frame at a time. That is what made cards trade places while an OSD came and went. A card still playing its exit keeps the surface up, and that is the whole reason the last card's exit reaches the screen at all: drop the surface on the frame the source empties and there is nothing left to animate.
fn has_cards(live: &Live, config: &Config) -> bool {
    live.osd.is_some()
        || !live.toasts.is_empty()
        || !crate::notifications::popping(&live.snapshot, &config.notifications, false).is_empty()
        || transition::anything_leaving()
}

/// Every card that should be on screen right now, in order and admitted per [`admit`].
///
/// "Right now" includes the ones on their way out. A card whose source has dropped it is still drawn until its exit has played, because the list disposes a row the instant it leaves the source and a card that vanishes mid-frame is one the user cannot tell apart from a card that was replaced. Holding it here — rather than asking `ReactiveList` for an exit hook it does not have — keeps the whole thing inside the column.
fn column(live: &Live, covering: bool, config: &Config) -> Vec<Card> {
    let mut cards: Vec<Card> =
        crate::notifications::popping(&live.snapshot, &config.notifications, covering)
            .into_iter()
            .map(Card::Notification)
            .collect();
    cards.extend(live.toasts.iter().cloned().map(Card::Toast));
    cards.extend(live.osd.map(Card::Osd));
    let ordered = ARRIVALS.with(|arrivals| arrivals.borrow_mut().order(cards));
    // What every source still holds, taken *before* the cap. A card queued behind [`admit`] has not gone anywhere — playing its exit would say it had, and the next pass would have to take it back.
    let alive: Vec<String> = ordered.iter().map(Card::slot).collect();
    let mut shown = admit(ordered, config.stack.visible());
    reconcile_departures(&mut shown, &alive, &config.animation);
    shown
}

/// Starts the exit of everything that has gone since the last pass, and keeps drawing what has not finished — **in the place it held**, which is the whole difficulty.
///
/// A card removed from the middle leaves a hole, and putting its ghost back anywhere else says the wrong thing twice: the cards below it jump up to close the hole, and the exit then plays at the bottom of the column against a card that was never there. So each one is remembered with the index it was drawn at and re-inserted there, oldest position first, so a run of departures unwinds in the order it was drawn rather than in whatever order a hash map happens to iterate.
///
/// `alive` is every card the sources still hold, *before* the cap: what [`admit`] leaves out is queued, not gone, and animating it away would be a lie the next pass has to retract.
///
/// The previous pass's cards are remembered here rather than derived, because "gone" is a difference between two readings and only the surface takes them: the host holds its own copy of the same sources on its own schedule (see [`has_cards`]), so a departure it computed would disagree with this one for a frame at a time.
fn reconcile_departures(shown: &mut Vec<Card>, alive: &[String], animation: &AnimationConfig) {
    for slot in transition::settled() {
        DEPARTED.with(|held| held.borrow_mut().remove(&slot));
    }
    for slot in alive {
        transition::returning(slot);
    }
    let previous = LAST_ALIVE.with(|held| held.replace(alive.to_vec()));
    for slot in previous.into_iter().filter(|slot| !alive.contains(slot)) {
        if DEPARTED.with(|held| !held.borrow().contains_key(&slot)) {
            continue;
        }
        let after = transition::leaving(&slot, animation);
        if after.is_zero() {
            continue;
        }
        // The list only rebuilds when its source re-runs, and nothing else will ask it to once the card is gone from every source — so the pass that drops the finished card has to be scheduled here.
        timeout(after, || {
            // Both, and for different reasons: the tick re-runs the column so it stops drawing the card, and the host has to be asked again whether there is anything left to keep the surface up for.
            DEPARTURES.update(|tick| *tick = tick.wrapping_add(1));
            reconcile();
        });
    }
    // Kept whole rather than by slot: an exit draws the card, so the card has to outlive the source that had it.
    DEPARTED.with(|held| {
        let mut held = held.borrow_mut();
        for (at, card) in shown.iter().enumerate() {
            held.insert(card.slot(), (at, card.clone()));
        }
        let mut returning: Vec<(usize, Card)> = held
            .iter()
            .filter(|(slot, _)| transition::still_leaving(slot))
            .map(|(_, (at, card))| (*at, card.clone()))
            .collect();
        returning.sort_by_key(|(at, _)| *at);
        for (at, card) in returning {
            shown.insert(at.min(shown.len()), card);
        }
    });
}

/// Which of `ordered` fit on screen, and which wait.
///
/// **Every provider that has something to say gets one card, before capacity is shared out.** A plain cap does not work here, and the way it fails is the point: with four notifications up, a brightness change would be queued behind them — so the reading you asked for by pressing a key is the one thing you cannot see, until notifications you did not ask about have gone. A provider that is *answering* the user has to be able to answer.
///
/// The guarantee wins over `capacity`, so a column can hold more cards than `[stack] max_visible` when more providers than that are speaking at once. That is the honest trade: the alternative is a provider silenced by a number that was chosen to bound *notifications*.
///
/// Everything past the guarantee shares what is left in arrival order, and the rest waits for room.
fn admit(ordered: Vec<Card>, capacity: usize) -> Vec<Card> {
    let mut speaking: Vec<&'static str> = Vec::new();
    let guaranteed: Vec<bool> = ordered
        .iter()
        .map(|card| {
            let first = !speaking.contains(&card.provider());
            if first {
                speaking.push(card.provider());
            }
            first
        })
        .collect();
    let mut spare = capacity.saturating_sub(speaking.len());
    let mut shown = Vec::with_capacity(ordered.len());
    for (card, guaranteed) in ordered.into_iter().zip(guaranteed) {
        if guaranteed {
            shown.push(card);
        } else if spare > 0 {
            spare -= 1;
            shown.push(card);
        }
    }
    shown
}

/// The open column: its surface, and the screen it opened on so a focus change can tell it has to move.
struct Stack {
    output: Option<String>,
    /// Held for its `Drop` and read by nothing: dropping the handle is what unmaps the surface, which is how an empty column leaves no overlay behind.
    #[allow(dead_code)]
    handle: SurfaceHandle,
}

/// Brings the column up. Called once from `setup_shell`, on the driver thread, and long-lived: a config reload changes what the next surface looks like, not whether the shell is listening.
///
/// Three subscriptions of its own, beside the ones the surface's content makes. They answer a different question — *is there anything to show* — and it has to be answered while there is no surface to ask it on.
pub fn host() {
    watch(
        services::notifications::subscribe,
        |snap: SharedSnapshot| {
            LIVE.with(|live| live.borrow_mut().snapshot = snap);
            reconcile();
        },
    );
    watch(toaster::subscribe, |toasts: Vec<Toast>| {
        LIVE.with(|live| live.borrow_mut().toasts = toasts);
        reconcile();
    });
    watch(
        |tx| OSD.subscribe(tx),
        |osd: Option<OsdKind>| {
            LIVE.with(|live| live.borrow_mut().osd = osd);
            reconcile();
        },
    );
    follow_focus();
}

/// Opens the column when it has something to say and drops it when it does not.
///
/// The fullscreen policy is deliberately not asked here: it is a reactive reading the *content* re-evaluates, and a host that suppressed on it would need a second subscription to the compositor to answer a question whose only wrong answer is a surface that is up holding nothing — invisible, click-through, and gone on the next change anyway.
fn reconcile() {
    let output = surfaces::shell::focused_output();
    let config = config::config_for(output.as_deref());
    let empty = LIVE.with(|live| !has_cards(&live.borrow(), &config));
    OPEN.with(|open| {
        let mut open = open.borrow_mut();
        match (open.is_some(), empty) {
            // Dropping the handle is what unmaps it, so an empty column leaves no overlay behind.
            (true, true) => *open = None,
            (false, false) => *open = Some(open_stack(output, &config)),
            _ => {}
        }
    });
}

fn open_stack(output: Option<String>, config: &Config) -> Stack {
    let placement = placement(&config.stack, output.as_deref())
        .margin(config.panel_margin(config.stack.edge))
        .output(output.clone());
    let handle =
        PanelSurface::new(placement, |env| cards(env).expect("stack build failed")).open_handle();
    Stack { output, handle }
}

/// Where the column sits, and the shape its cards lay out in. The surface and the column come from one placement so a column holding fewer cards than it is sized for still hugs the edge it is pinned to.
fn placement(config: &StackConfig, output: Option<&str>) -> Placement {
    Placement::stack(NAMESPACE, config.edge, config.align)
        .size(config.width.max(120.0) as u32, room_along(config, output))
}

/// How much room the surface asks for along its edge: **all of it.**
///
/// A layer surface names its size before it knows what it will hold, so this used to be a guess — a per-card height times how many cards were allowed. A guess is the wrong shape of answer here, and it failed exactly where you would expect: a notification with a long body is taller than any number picked for the OSD's meter, so a full column overflowed the surface and the last card was clipped — or swapped in and out as the cards above it changed height.
///
/// Asking for the whole edge costs nothing, and that is the point: the surface carves its input region out of what the cards actually draw ([`Input::FromContent`]), so every pixel the column does not fill is click-through and belongs to the window underneath. It is the same trade the hover popout makes.
///
/// [`Input::FromContent`]: ui::placement::Input::FromContent
fn room_along(config: &StackConfig, output: Option<&str>) -> u32 {
    let outputs = platform_wayland::outputs();
    let screen = match output {
        Some(name) => outputs.iter().find(|o| o.name.as_deref() == Some(name)),
        None => outputs.first(),
    };
    let along = screen.and_then(|o| o.logical_size).map(|(width, height)| {
        if config.edge.is_horizontal() {
            height
        } else {
            width
        }
    });
    along.unwrap_or(MIN_HEIGHT as i32).max(MIN_HEIGHT as i32) as u32
}

/// A focus change moves the column, because a layer surface names its output when it is created — so following focus is opening it again somewhere else rather than moving what is there.
fn follow_focus() {
    let dir = services::hyprland::socket_dir();
    watch(
        move |tx| {
            let Some(dir) = dir else {
                return;
            };
            let Ok(events) = UnixStream::connect(dir.join(".socket2.sock")) else {
                return;
            };
            for line in BufReader::new(events).lines().map_while(Result::ok) {
                if let Some(monitor) = services::hyprland::monitor_from_focus_event(&line)
                    && !tx.send(monitor)
                {
                    break;
                }
            }
        },
        |monitor: String| {
            let elsewhere = OPEN.with(|open| {
                open.borrow()
                    .as_ref()
                    .is_some_and(|stack| stack.output.as_deref() != Some(monitor.as_str()))
            });
            if elsewhere {
                OPEN.with(|open| *open.borrow_mut() = None);
                reconcile();
            }
        },
    );
}

/// Follows a config change: the column takes the new look where it stands, and is opened again if the edit moved it somewhere the compositor has to be told about.
pub fn reconcile_config() {
    OPEN.with(|open| *open.borrow_mut() = None);
    reconcile();
}

/// The live column, built on the surface's own thread. Each source is subscribed to again here, because these signals belong to this surface and die with it.
fn cards(env: &config::SurfaceEnv) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let config = &env.config;
    let snapshot = signal(Arc::new(Snapshot::default()));
    let sink = snapshot.clone();
    watch(
        services::notifications::subscribe,
        move |snap: SharedSnapshot| sink.set(snap),
    );
    let toasts = signal(toaster::current());
    let sink = toasts.clone();
    watch(toaster::subscribe, move |live: Vec<Toast>| sink.set(live));
    let osd = signal(OSD.get());
    let sink = osd.clone();
    watch(
        |tx| OSD.subscribe(tx),
        move |live: Option<OsdKind>| sink.set(live),
    );
    // The column re-runs when a card's exit is over, which no source can tell it: by then every one of them has already dropped the card.
    let departures = signal(DEPARTURES.get());
    let sink = departures.clone();
    watch(
        |tx| DEPARTURES.subscribe(tx),
        move |tick: u64| sink.set(tick),
    );
    let covering = crate::notifications::covering_focus(&config.notifications);

    let theme = use_theme::<NordTheme>();
    let radius = content_radius();
    let width = config.stack.width;
    let animation = config.animation.clone();
    let owned = config.clone();
    let source = move || {
        let _ = departures.get();
        let live = Live {
            snapshot: snapshot.get(),
            toasts: toasts.get(),
            osd: osd.get(),
        };
        let cards = column(&live, covering.as_ref().is_some_and(|c| c.get()), &owned);
        // This is the moment a notification is on screen, and so the moment its expiry may start. The daemon spends the arming on the first call, so a card that stays up is not handed a fresh clock on every repaint — and one that waited behind a full column gets its whole life when it finally arrives.
        for card in &cards {
            if let Card::Notification(n) = card {
                services::notifications::shown(n.id);
            }
        }
        cards
    };
    let list = ReactiveList::with_style(
        placement(&config.stack, env.output.as_deref()).column(card_gap()),
        source,
        Card::key,
        move |card: Card| build(card, theme, radius, width, &animation),
    )?;
    Ok(Box::new(list))
}

/// One card, built by whichever module owns it. The column knows how to place a card and nothing about what is on it.
fn build(
    card: Card,
    theme: NordTheme,
    radius: f32,
    width: f32,
    animation: &AnimationConfig,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let slot = card.slot();
    let content = match card {
        Card::Notification(n) => crate::notifications::popup_card(&n, theme, radius, width),
        Card::Toast(t) => crate::toast::card(&t, theme, radius),
        Card::Osd(kind) => Ok(crate::osd::osd_content(kind, theme)),
    }?;
    // Here rather than in each of the three, so a notification, a toast and an OSD arrive and leave alike.
    transition::arriving(&slot, content, animation)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// How many providers there are, and so the most cards the column can be made to hold whatever `[stack] max_visible` says. Tied to [`Card`]'s variants by hand; `a_column_grows_past_its_cap_rather_than_silence_a_provider` is what notices when a fourth arrives and this was not updated.
    const PROVIDERS: usize = 3;

    fn toast(event: toaster::Event, title: &str) -> Card {
        Card::Toast(Toast::sample(event, "icon", title, ""))
    }

    /// A card that is replaced keeps the place it already had.
    ///
    /// The key carries the card's *contents* so the list redraws it; the slot does not, so a second reading about the same thing is the same slot. Stamping arrival against the key instead would send every replacement to the bottom of the column — a volume OSD that jumps down the screen on every notch.
    #[test]
    fn a_replaced_card_keeps_its_place() {
        let mut arrivals = Arrivals::default();
        arrivals.order(vec![
            toast(toaster::Event::Vpn, "VPN on"),
            toast(toaster::Event::Dnd, "Do not disturb"),
        ]);
        let replaced = arrivals.order(vec![
            toast(toaster::Event::Dnd, "Do not disturb"),
            toast(toaster::Event::Vpn, "VPN off"),
        ]);
        assert!(
            matches!(&replaced[0], Card::Toast(t) if t.event == toaster::Event::Vpn),
            "the replacement is redrawn where the card it replaced was, whatever order it arrives in"
        );
    }

    /// A slot that has gone is forgotten, so the same thing arriving again is a new arrival — otherwise a notification dismissed and re-sent would reappear above cards that have been waiting.
    #[test]
    fn a_card_that_went_away_does_not_keep_its_old_place() {
        let mut arrivals = Arrivals::default();
        arrivals.order(vec![toast(toaster::Event::Vpn, "VPN on")]);
        arrivals.order(vec![toast(toaster::Event::Dnd, "Do not disturb")]);
        assert_eq!(arrivals.seen.len(), 1, "the VPN slot is gone and forgotten");

        let both = arrivals.order(vec![
            toast(toaster::Event::Vpn, "VPN on"),
            toast(toaster::Event::Dnd, "Do not disturb"),
        ]);
        assert!(
            matches!(&both[0], Card::Toast(t) if t.event == toaster::Event::Dnd),
            "the one that never left is still the older arrival"
        );
    }

    fn note(id: u32) -> Card {
        Card::Notification(Notification {
            id,
            app_name: "test".into(),
            app_icon: String::new(),
            summary: format!("note {id}"),
            body: String::new(),
            actions: Vec::new(),
            urgency: services::notifications::Urgency::Normal,
            popup: true,
            image: None,
        })
    }

    /// **The card a provider is answering with must reach the screen.**
    ///
    /// A plain cap fails here in the way that matters: with the column full of notifications, pressing the brightness key would queue the OSD behind them, so the one reading the user actually asked for is the one they cannot see until notifications they never asked about have gone.
    #[test]
    fn every_provider_is_guaranteed_a_card() {
        let full = vec![note(1), note(2), note(3), note(4)];
        let mut with_osd = full.clone();
        with_osd.push(Card::Osd(OsdKind::Brightness));

        let shown = admit(with_osd, 4);
        assert!(
            shown.iter().any(|card| matches!(card, Card::Osd(_))),
            "the OSD is answering a keypress and cannot be queued behind a full column"
        );
        assert_eq!(
            shown.len(),
            4,
            "and it costs the oldest notification its slot"
        );
        assert!(
            !shown
                .iter()
                .any(|card| matches!(card, Card::Notification(n) if n.id == 4)),
            "the notification that gives way is the last in, not the first"
        );

        // With nothing else speaking, notifications have the whole column.
        assert_eq!(admit(full, 4).len(), 4);
    }

    /// The guarantee wins over the cap, so more providers than `max_visible` means a taller column rather than a provider silenced by a number that was chosen to bound notifications.
    #[test]
    fn a_column_grows_past_its_cap_rather_than_silence_a_provider() {
        let all = vec![
            note(1),
            toast(toaster::Event::Vpn, "VPN on"),
            Card::Osd(OsdKind::Volume),
        ];
        assert_eq!(
            admit(all, 1).len(),
            PROVIDERS,
            "one card each, cap or no cap"
        );
    }

    /// The column caps what it holds, and it is the only thing that does: a notification queue trimmed to `max_visible` on its way in as well would hide cards the column had made room for.
    #[test]
    fn the_column_is_capped_once_by_the_stack_and_not_by_its_sources() {
        let config = Config {
            stack: StackConfig {
                max_visible: 2,
                ..StackConfig::default()
            },
            ..Config::starter()
        };
        let live = Live {
            toasts: vec![
                Toast::sample(toaster::Event::Vpn, "i", "one", ""),
                Toast::sample(toaster::Event::Dnd, "i", "two", ""),
                Toast::sample(toaster::Event::GameMode, "i", "three", ""),
            ],
            ..Live::default()
        };
        assert_eq!(column(&live, false, &config).len(), 2);
    }
}
