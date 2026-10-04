//! The column notification popups, toasts and OSDs arrive in, drawn by the layout's `stack` areas. [`host`] holds the window of every stack with cards to show and lets it go once the last one has left, so an idle session keeps its overlay window closed.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::os::unix::net::UnixStream;
use std::rc::Rc;
use std::sync::Arc;

use platform_wayland::{timeout, watch};
use telar::{
    Container, LayoutError, LayoutItem, LayoutStyle, ReactiveList, ReadSignal, RwSignal,
    Transition, detached, memo, signal, use_theme,
};

use config::policy::Urgency;
use config::theme::NordTheme;
use config::{Config, StackConfig};
use layout::{
    AreaId, CardKind, LayerKind, ResolvedArea, ResolvedAreaKind, RoutedCard, StackOutputPolicy,
};
use services::hyprland::{self, ActiveWindow, Client};
use services::notifications::{Notification, SharedSnapshot, Snapshot};
use services::toaster::{self, Toast};
use surfaces::area::Surround;
use surfaces::layer_window::Hold;
use surfaces::pinned::{self, Side};
use surfaces::reconcile::{self, StackSite, stacks};
use surfaces::transient;
use ui::chrome::{card_gap, content_radius};
use ui::descriptor::Built;
use util::broadcast::Store;

use crate::osd::OsdKind;
use swipe::Column;

pub(crate) mod swipe;

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

    fn kind(&self) -> CardKind {
        match self {
            Card::Notification(_) => CardKind::Notification,
            Card::Toast(_) => CardKind::Toast,
            Card::Osd(_) => CardKind::Osd,
        }
    }

    /// What a stack's routes are matched against.
    fn routed(&self) -> RoutedCard<'_> {
        match self {
            Card::Notification(n) => RoutedCard {
                kind: CardKind::Notification,
                app: Some(&n.app_name),
                urgency: Some(urgency_of(n.urgency)),
            },
            _ => RoutedCard {
                kind: self.kind(),
                app: None,
                urgency: None,
            },
        }
    }
}

/// The single-slot OSD, as a source the column can subscribe to like the other two.
///
/// A store rather than a signal because the windows it feeds come and go as the column fills and empties, and a signal made inside a window goes with it.
static OSD: Store<Option<OsdKind>> = Store::new(|| None);

thread_local! {
    /// Bumped on every OSD trigger, so the expiry scheduled by the one that was replaced fires against a generation that no longer matches and does nothing.
    static OSD_GENERATION: Cell<u64> = const { Cell::new(0) };
    static ARRIVALS: RefCell<Arrivals> = RefCell::new(Arrivals::default());
    /// What the column is holding, as the host last saw it. The host has to answer "is there anything to show" while no window is drawing the column, and the daemon publishes rather than answers.
    static LIVE: RefCell<Live> = RefCell::new(Live::default());
    static HELD: RefCell<Vec<(Option<String>, LayerKind, Hold)>> = const { RefCell::new(Vec::new()) };
    static FOCUS: RefCell<Option<String>> = const { RefCell::new(None) };
    static COVER: RefCell<(Vec<Client>, String)> = const { RefCell::new((Vec::new(), String::new())) };
    /// The output a stack that follows focus is drawn on, which every such stack reads: owned by no window, since the windows reading it come and go as the column moves.
    static SHOWN_ON: RwSignal<Option<String>> = detached(|| signal(None));
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

#[derive(Default, Clone)]
struct Live {
    snapshot: Arc<Snapshot>,
    toasts: Vec<Toast>,
    osd: Option<OsdKind>,
}

/// Every card a column could show, unordered: [`column`]'s ordering is a side effect only a drawn column may cause.
fn cards_now(live: &Live, covering: bool, config: &Config) -> Vec<Card> {
    let mut cards: Vec<Card> =
        crate::notifications::popping(&live.snapshot, &config.notifications, covering)
            .into_iter()
            .map(Card::Notification)
            .collect();
    cards.extend(live.toasts.iter().cloned().map(Card::Toast));
    cards.extend(live.osd.map(Card::Osd));
    cards
}

/// Whether the focused window covers the screen, from the host's own reading of it.
fn covering(config: &Config) -> bool {
    config.notifications.fullscreen != config::FullscreenPopups::On
        && COVER.with(|cover| {
            let (clients, active) = &*cover.borrow();
            !active.is_empty() && clients.iter().any(|c| c.address == *active && c.fullscreen)
        })
}

/// A card that has left is not here: the column plays its exit as the list drops it.
fn column(live: &Live, covering: bool, config: &Config) -> Vec<Card> {
    let cards = cards_now(live, covering, config);
    let ordered = ARRIVALS.with(|arrivals| arrivals.borrow_mut().order(cards));
    admit(ordered, config.stack.visible())
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

/// Subscribes on its own, beside the stack areas, because whether there is anything to show has to be answered while no window is drawing the column.
pub fn host() {
    FOCUS.with(|focus| *focus.borrow_mut() = transient::focused_output());
    watch(hyprland::subscribe_clients, |clients: Vec<Client>| {
        COVER.with(|cover| cover.borrow_mut().0 = clients);
        reconcile();
    });
    watch(hyprland::subscribe_active_window, |window: ActiveWindow| {
        COVER.with(|cover| cover.borrow_mut().1 = window.address);
        reconcile();
    });
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

pub fn reconcile_config() {
    reconcile();
}

/// The fullscreen policy is deliberately not asked here: the column re-evaluates it, and a window held up with nothing in it is invisible and click-through.
fn reconcile() {
    let sites = stacks();
    let shown_on = shown_on(&sites, FOCUS.with(|focus| focus.borrow().clone()));
    let drawn_on = SHOWN_ON.with(|drawn| *drawn);
    if drawn_on.peek() != shown_on {
        drawn_on.set(shown_on.clone());
    }
    let config = config::config_for(shown_on.as_deref());
    let cards = LIVE.with(|live| cards_now(&live.borrow(), covering(&config), &config));
    let mut wanted: Vec<(Option<String>, LayerKind)> = Vec::new();
    if !cards.is_empty() {
        for site in &sites {
            let lands = cards
                .iter()
                .any(|card| landing(card, &sites, &site.output) == Some(&site.area));
            let showing =
                lands && (site.policy != StackOutputPolicy::Focused || site.output == shown_on);
            let at = (site.output.clone(), site.layer);
            if showing && !wanted.contains(&at) {
                wanted.push(at);
            }
        }
    } else if sites.is_empty() {
        tracing::debug!("the layout has no stack area, so no card is shown");
    }
    let exit = config.animation.tween_ms(200, 2_000).duration;
    let released: Vec<(Option<String>, LayerKind, Hold)> = HELD.with(|held| {
        let mut held = held.borrow_mut();
        let (keep, go) = std::mem::take(&mut *held)
            .into_iter()
            .partition(|(output, layer, _)| wanted.contains(&(output.clone(), *layer)));
        *held = keep;
        go
    });
    for (output, layer, hold) in released {
        transient::release(hold, output.as_deref(), layer, exit);
    }
    for (output, layer) in wanted {
        let already = HELD.with(|held| {
            held.borrow()
                .iter()
                .any(|(o, l, _)| *o == output && *l == layer)
        });
        if already {
            continue;
        }
        if let Some(hold) = transient::hold(output.as_deref(), layer) {
            HELD.with(|held| held.borrow_mut().push((output, layer, hold)));
        }
    }
}

fn shown_on(sites: &[StackSite], focused: Option<String>) -> Option<String> {
    let following = || {
        sites
            .iter()
            .filter(|site| site.policy == StackOutputPolicy::Focused)
    };
    match following().any(|site| site.output == focused) {
        true => focused,
        false => following()
            .next()
            .map_or(focused, |site| site.output.clone()),
    }
}

/// Follows the output focus is on through Hyprland's event socket, for as long as the socket is there.
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
            focus_events(
                BufReader::new(events).lines().map_while(Result::ok),
                |monitor| tx.send(monitor),
            );
        },
        focus_moved,
    );
}

/// Hands `moved` every output focus moves to, in the order the event socket's `lines` say so, until it answers that nobody is listening.
fn focus_events(lines: impl Iterator<Item = String>, mut moved: impl FnMut(String) -> bool) {
    for line in lines {
        if let Some(monitor) = services::hyprland::monitor_from_focus_event(&line)
            && !moved(monitor)
        {
            break;
        }
    }
}

fn focus_moved(monitor: String) {
    FOCUS.with(|focus| *focus.borrow_mut() = Some(monitor));
    reconcile();
}

fn landing<'a>(card: &Card, sites: &'a [StackSite], output: &Option<String>) -> Option<&'a AreaId> {
    reconcile::stack_for(sites, output, &card.routed()).map(|site| &site.area)
}

fn urgency_of(urgency: Urgency) -> layout::Urgency {
    match urgency {
        Urgency::Low => layout::Urgency::Low,
        Urgency::Normal => layout::Urgency::Normal,
        Urgency::Critical => layout::Urgency::Critical,
    }
}

/// A stack that follows focus is drawn only on the output the host shows it on, and moving it there is a move rather than a departure and an arrival: the column on the output focus left goes at once, its cards playing no exit, and the one on the output focus reached is built already holding them, so none plays an entrance.
pub fn area(area: &ResolvedArea, surround: Surround) -> Built {
    let ResolvedAreaKind::Stack {
        anchor,
        offset,
        width,
        output_policy,
        ..
    } = &area.kind
    else {
        return Err(LayoutError::Engine(format!(
            "'{}' is a {} area, not a stack",
            area.id,
            area.kind.name()
        )));
    };
    let config = Arc::clone(surround.config);
    let output = surround.output.map(str::to_string);
    let id = area.id.clone();
    let follows = *output_policy == StackOutputPolicy::Focused;
    let width = *width;
    util::state::set_context(Column { width });

    let snapshot = signal(Arc::new(Snapshot::default()));
    watch(
        services::notifications::subscribe,
        move |snap: SharedSnapshot| snapshot.set(snap),
    );
    let toasts = signal(toaster::current());
    watch(toaster::subscribe, move |live: Vec<Toast>| toasts.set(live));
    let osd = signal(OSD.get());
    watch(
        |tx| OSD.subscribe(tx),
        move |live: Option<OsdKind>| osd.set(live),
    );
    let covering = crate::notifications::covering_focus(&config.notifications);
    let here = {
        let output = output.clone();
        let shown_on = SHOWN_ON.with(|shown| *shown);
        memo(move || !follows || shown_on.with(|shown| *shown == output))
    };

    let built_with = Arc::clone(&config);
    let cards = Rc::new(move || {
        let live = Live {
            snapshot: snapshot.get(),
            toasts: toasts.get(),
            osd: osd.get(),
        };
        let sites = stacks();
        let cards: Vec<Card> = column(
            &live,
            covering.as_ref().is_some_and(|c| c.get()),
            &built_with,
        )
        .into_iter()
        .filter(|card| landing(card, &sites, &output) == Some(&id))
        .collect();
        // This is the moment a notification is on screen, and so the moment its expiry may start. The daemon spends the arming on the first call, so a card that stays up is not handed a fresh clock on every repaint.
        for card in &cards {
            if let Card::Notification(n) = card {
                services::notifications::shown(n.id);
            }
        }
        cards
    });

    let theme = use_theme::<NordTheme>();
    let radius = content_radius();
    let gap = card_gap();
    let (across, down) = pinned::sides(*anchor);
    let from_end = down == Side::End;
    let slide_from = match across {
        Side::Start => telar::Edge::Left,
        Side::Middle | Side::End => telar::Edge::Right,
    };
    let tween = config.animation.tween_ms(200, 2_000);
    // The transition is set once the list has built, so what it holds then appears settled, and only a card arriving after that slides in.
    let drawn = ReactiveList::with_style(
        LayoutStyle::new().flex_column(),
        move || here.get().then_some(()).into_iter().collect(),
        |_: &()| (),
        move |()| -> Built {
            let cards = Rc::clone(&cards);
            let row_config = Arc::clone(&config);
            let list = ReactiveList::keyed(
                move || cards(),
                Card::slot,
                move |card: ReadSignal<Card>| row(card, &row_config, theme, radius, gap, from_end),
            )?
            .with_transition(Transition::slide(slide_from, TRAVEL, tween))
            .animate_layout(tween);
            Ok(Box::new(list))
        },
    )?;

    let placed = pinned::column(surround.bounds, *anchor, width, *offset);
    let justify = match down {
        Side::Start => telar::JustifyContent::START,
        Side::Middle => telar::JustifyContent::CENTER,
        Side::End => telar::JustifyContent::END,
    };
    Ok(Box::new(surfaces::area::empty_space(
        area,
        surround,
        surfaces::area::dressed(
            &area.style,
            &surround.theme,
            surfaces::area::at(placed)
                .flex_column()
                .justify_content(justify),
            vec![Box::new(drawn)],
        )?,
        surfaces::area::is_filled(&area.style, &surround.theme),
    )))
}

/// Sideways, matching the swipe, so a card that arrives along the same axis reads as the same object.
const TRAVEL: f32 = 28.0;

/// One card slot: whatever card that slot now holds, built again when its contents change and kept when only its place does.
fn row(
    card: ReadSignal<Card>,
    config: &Arc<Config>,
    theme: NordTheme,
    radius: f32,
    gap: f32,
    from_end: bool,
) -> Built {
    let config = Arc::clone(config);
    let content = ReactiveList::with_style(
        LayoutStyle::new().flex_column(),
        move || vec![card.get()],
        Card::key,
        move |card: Card| build(card, &config, theme, radius),
    )?;
    let style = LayoutStyle::new().flex_column();
    let style = match from_end {
        true => style.padding_top(gap),
        false => style.padding_bottom(gap),
    };
    Ok(Box::new(Container::new(style, vec![Box::new(content)])?))
}

fn build(
    card: Card,
    config: &Config,
    theme: NordTheme,
    radius: f32,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    match card {
        Card::Notification(n) => crate::notifications::popup_card(
            &n,
            &config.notifications,
            &config.stack,
            theme,
            radius,
        ),
        Card::Toast(t) => crate::toast::card(&t, theme, radius),
        Card::Osd(kind) => Ok(crate::osd::osd_content(kind, theme)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use layout::Route;

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

    fn site(output: &str, policy: StackOutputPolicy, routes: Vec<Route>) -> StackSite {
        StackSite {
            output: Some(output.to_string()),
            layer: LayerKind::Overlay,
            area: AreaId::new(format!("stack-{output}")),
            policy,
            routes,
        }
    }

    #[test]
    fn a_stack_that_follows_focus_lands_somewhere_when_nothing_says_where_focus_is() {
        let sites = vec![
            site("DP-1", StackOutputPolicy::Focused, Vec::new()),
            site("HDMI-A-1", StackOutputPolicy::Focused, Vec::new()),
        ];
        assert_eq!(shown_on(&sites, None), Some("DP-1".to_string()));
        assert_eq!(
            shown_on(&sites, Some("HDMI-A-1".to_string())),
            Some("HDMI-A-1".to_string())
        );
        assert_eq!(
            shown_on(&sites, Some("eDP-1".to_string())),
            Some("DP-1".to_string()),
            "focus on a screen with no stack sends the column to one that has"
        );
    }

    #[test]
    fn a_card_lands_in_the_first_stack_whose_routes_take_it() {
        let critical = Route {
            kind: Some(CardKind::Notification),
            urgency: Some(layout::Urgency::Critical),
            ..Route::default()
        };
        let sites = vec![
            site("DP-1", StackOutputPolicy::Here, vec![critical]),
            StackSite {
                area: AreaId::new("corner"),
                ..site("DP-1", StackOutputPolicy::Here, Vec::new())
            },
        ];
        let output = Some("DP-1".to_string());
        let mut urgent = note(1);
        if let Card::Notification(n) = &mut urgent {
            n.urgency = Urgency::Critical;
        }
        assert_eq!(
            landing(&urgent, &sites, &output),
            Some(&AreaId::new("stack-DP-1"))
        );
        assert_eq!(
            landing(&note(2), &sites, &output),
            Some(&AreaId::new("corner"))
        );
        assert_eq!(
            landing(&toast(toaster::Event::Vpn, "VPN on"), &sites, &output),
            Some(&AreaId::new("corner")),
            "a route naming a kind turns every other kind away"
        );
        assert_eq!(
            landing(&note(3), &sites, &Some("HDMI-A-1".to_string())),
            None,
            "a screen with no stack shows no card"
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

    const SCREENS: [&str; 2] = ["DP-1", "HDMI-A-1"];

    /// The shipped layout planned on two screens side by side and published, so its stacks are where cards are offered.
    fn published() -> Vec<surfaces::reconcile::Desktop> {
        let outputs: Vec<platform_wayland::OutputDescriptor> = SCREENS
            .iter()
            .enumerate()
            .map(|(at, name)| platform_wayland::OutputDescriptor {
                name: Some(name.to_string()),
                logical_size: Some((1920, 1080)),
                position: (1920 * at as i32, 0),
                scale: 1,
            })
            .collect();
        let (desktops, _) = surfaces::reconcile::plan(
            std::path::Path::new("/nonexistent/config.toml"),
            &Arc::new(Config::default()),
            &layout::built_in(),
            &layout::Library::default(),
            &outputs,
            &|_| None,
        );
        surfaces::reconcile::publish(&desktops);
        desktops
    }

    /// One screen's stack column, drawn as its overlay window draws it.
    struct Drawn {
        tree: telar::ComponentList,
        root: telar::NodeId,
    }

    impl Drawn {
        fn of(desktop: &surfaces::reconcile::Desktop) -> Self {
            let stack = desktop
                .resolved
                .areas()
                .find(|(_, area)| matches!(area.kind, ResolvedAreaKind::Stack { .. }))
                .map(|(_, area)| area.clone())
                .expect("the shipped stack");
            let surround = Surround {
                config: &desktop.config,
                theme: desktop.config.resolve_theme(),
                output: desktop.output.as_deref(),
                layer: LayerKind::Overlay,
                bounds: telar::Rect::new(0.0, 0.0, 1920.0, 1080.0),
                reserved: desktop.reserved,
                audience: ui::host::Audience::Owner,
            };
            let item = area(&stack, surround).expect("the column builds");
            let page = Container::new(LayoutStyle::new().width(1920.0).height(1080.0), vec![item])
                .expect("a screen");
            let root = page.layout_node();
            Self {
                tree: telar::ComponentList::new(page),
                root,
            }
        }

        /// What the column draws, laid out as it is now.
        fn drawn(&self) -> Vec<telar::DrawCommand> {
            telar::compute_layout(
                self.root,
                telar::AvailableSpace::Definite(1920.0),
                telar::AvailableSpace::Definite(1080.0),
            )
            .expect("the column lays out");
            self.tree.commands().clone()
        }
    }

    /// Whether two frames look the same, drawn by two trees: every command alike but the ids that say which tree drew it — a card sliding in or fading out draws under a transform or an opacity the settled one does not.
    fn alike(new: &[telar::DrawCommand], old: &[telar::DrawCommand]) -> bool {
        let look = |commands: &[telar::DrawCommand]| -> Vec<String> {
            commands
                .iter()
                .filter(|command| {
                    !matches!(
                        command,
                        telar::DrawCommand::PushElement { .. } | telar::DrawCommand::PopElement
                    )
                })
                .map(|command| format!("{command:?}"))
                .collect()
        };
        look(new) == look(old)
    }

    fn focus_on(script: &str) -> usize {
        let mut moves = 0;
        focus_events(script.lines().map(str::to_string), |monitor| {
            moves += 1;
            focus_moved(monitor);
            true
        });
        moves
    }

    /// TA-1, T-7.4: focus moving to another screen, as Hyprland's event socket says it, moves the column there. The screen focus left is at once what it is with no column — no card plays its exit there — and the screen focus reached draws the column exactly as a settled one is drawn — no card plays its entrance; a line that is not a focus move moves nothing.
    #[test]
    fn following_focus_moves_the_column_without_an_exit_or_an_entrance() {
        telar::reset_layout_runtime();
        telar::set_locale("en");
        telar::set_theme(Config::default().resolve_theme());
        let scope = telar::owner_scope();
        let desktops = published();
        show_osd(OsdKind::Volume);
        focus_on("focusedmon>>DP-1,1");
        let [left, right] = [&desktops[0], &desktops[1]].map(Drawn::of);
        let (column, none) = (left.drawn(), right.drawn());
        assert!(
            !alike(&column, &none),
            "the column is on the focused screen, and nowhere else"
        );

        let moves =
            focus_on("workspace>>2\nfocusedmon>>HDMI-A-1,2\nactivewindow>>firefox,Mozilla Firefox");
        assert_eq!(moves, 1, "only the focus line is a move");
        assert_eq!(
            SHOWN_ON.with(|shown| shown.peek()),
            Some("HDMI-A-1".to_string())
        );
        assert_eq!(telar::exits_in_flight().get(), 0, "no card is leaving");
        assert!(
            alike(&left.drawn(), &none),
            "the screen focus left shows no column, at once"
        );
        assert!(
            alike(&right.drawn(), &column),
            "the screen focus reached shows the column as it was, with nothing sliding in"
        );

        focus_on("focusedmon>>DP-1,1");
        assert_eq!(telar::exits_in_flight().get(), 0);
        assert!(
            alike(&left.drawn(), &column) && alike(&right.drawn(), &none),
            "and back"
        );
        clear_osd();
        drop((left, right));
        telar::dispose_owner(scope.id());
    }
}
