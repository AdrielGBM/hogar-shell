//! What is waiting, as a widget: how many, from whom, and — only for the signed-in user — what each one says. Every field is read through [`Host::reveal`] against the field declared here, so a build for [`Audience::Anyone`](ui::host::Audience::Anyone) cannot draw private text; a body is never drawn at any size.

use telar::{LayoutError, LayoutItem, ReactiveList, RwSignal, signal};

use config::theme::{FontRole, NordTheme};
use services::notifications::{self, SharedSnapshot, Snapshot};
use ui::card::{Card, Density};
use ui::descriptor::{FieldDef, Privacy, SourceDef};
use ui::host::{Host, WidgetSize};
use ui::scale::space;
use util::reactive::{derive, fixed_text};

pub const COUNT: FieldDef = FieldDef {
    name: "count",
    privacy: Privacy::Public,
};
pub const APPS: FieldDef = FieldDef {
    name: "apps",
    privacy: Privacy::Public,
};
pub const SUMMARY: FieldDef = FieldDef {
    name: "summary",
    privacy: Privacy::Private,
};
pub const BODY: FieldDef = FieldDef {
    name: "body",
    privacy: Privacy::Private,
};

pub const SOURCE: SourceDef = SourceDef {
    id: "notifications",
    fields: &[COUNT, APPS, SUMMARY, BODY],
};

/// How long a summary may run in a row before it is cut, so a long one elides rather than pushing the row past its widget.
const SUMMARY_CHARS: usize = 40;

#[derive(Clone, Debug, PartialEq, Eq)]
struct Entry {
    id: u32,
    app: String,
    summary: String,
}

pub fn widget(host: &Host) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let snapshot = signal(notifications::snapshot_now());
    let sink = snapshot;
    platform_wayland::watch(notifications::subscribe, move |next: SharedSnapshot| {
        sink.set(Some(next))
    });
    reading(host, snapshot)
}

fn reading(
    host: &Host,
    snapshot: RwSignal<Option<SharedSnapshot>>,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let theme = telar::use_theme::<NordTheme>();
    let viewer = host.clone();
    let count = derive(snapshot, move |s| {
        viewer
            .reveal(&COUNT, s.as_deref().map_or(0, |s| s.active.len()))
            .to_string()
    });
    let viewer = host.clone();
    let senders = derive(snapshot, move |s| {
        let apps = viewer.reveal(&APPS, s.as_deref().map(apps).unwrap_or_default());
        match (s.as_deref().map_or(0, |s| s.active.len()), apps.is_empty()) {
            (0, _) => telar::t!("notifications.none"),
            (waiting, true) => telar::t!("notifications.waiting", count = waiting.to_string()),
            (_, false) => apps.join(", "),
        }
    });

    let card = Card::titled(telar::t!("notifications.title")).icon(fixed_text("bell"));
    if host.is_small() {
        return card.figure(count).detail(senders).build(Density::Widget);
    }
    let limit = if host.widget_size() == Some(WidgetSize::L) {
        6
    } else {
        2
    };
    let viewer = host.clone();
    card.trailing(count)
        .detail(senders)
        .child(move || {
            let rows = ReactiveList::new(
                move || {
                    snapshot
                        .get()
                        .map(|s| entries(&s, &viewer, limit))
                        .unwrap_or_default()
                },
                |entry: &Entry| entry.id.to_string(),
                move |entry: Entry| {
                    ui::widget::label_value(
                        fixed_text(entry.app),
                        fixed_text(entry.summary),
                        theme.font(FontRole::Caption),
                        theme.subtle,
                        theme.text,
                    )
                },
                space::sm(),
            )?;
            Ok(Box::new(rows) as Box<dyn LayoutItem>)
        })
        .build(Density::Widget)
}

/// Each app with something waiting, once, in the order they first appear.
fn apps(snapshot: &Snapshot) -> Vec<String> {
    let mut apps: Vec<String> = Vec::new();
    for entry in &snapshot.active {
        if !apps.contains(&entry.app_name) {
            apps.push(entry.app_name.clone());
        }
    }
    apps
}

/// The newest `limit` notifications, each with its summary only if `host` may show it.
fn entries(snapshot: &Snapshot, host: &Host, limit: usize) -> Vec<Entry> {
    snapshot
        .active
        .iter()
        .rev()
        .take(limit)
        .map(|entry| Entry {
            id: entry.id,
            app: host.reveal(&APPS, entry.app_name.clone()),
            summary: host.reveal(&SUMMARY, clipped(&entry.summary)),
        })
        .collect()
}

fn clipped(text: &str) -> String {
    let text = text.trim();
    if text.chars().count() <= SUMMARY_CHARS {
        return text.to_string();
    }
    let kept: String = text.chars().take(SUMMARY_CHARS - 1).collect();
    format!("{}…", kept.trim_end())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use telar::{
        AvailableSpace, ComponentList, Container, DrawCommand, LayoutStyle, compute_layout,
        new_container,
    };

    use services::notifications::{Notification, Urgency};
    use ui::host::{Audience, Representation};

    use super::*;

    const SECRET_SUMMARY: &str = "Your code is 424242";
    const SECRET_BODY: &str = "Meet me behind the station";

    fn waiting() -> SharedSnapshot {
        let note = |id: u32, app: &str| Notification {
            id,
            app_name: app.to_string(),
            app_icon: String::new(),
            summary: SECRET_SUMMARY.to_string(),
            body: SECRET_BODY.to_string(),
            actions: Vec::new(),
            urgency: Urgency::Normal,
            popup: false,
            image: None,
        };
        Arc::new(Snapshot {
            active: vec![note(1, "Signal"), note(2, "Mail"), note(3, "Signal")],
            unread: 3,
            dnd: false,
            muted_apps: Vec::new(),
        })
    }

    fn host(size: WidgetSize, audience: Audience) -> Host {
        ui::preview::host_on(
            Arc::new(config::Config::starter()),
            "notifications",
            Representation::Widget(size),
            size.extent(),
        )
        .shown_to(audience)
    }

    /// Every string the reading puts on screen, laid out in its footprint.
    fn drawn_text(size: WidgetSize, audience: Audience) -> Vec<String> {
        telar::reset_layout_runtime();
        telar::set_locale("en");
        telar::set_theme(NordTheme::new());
        let scope = telar::owner_scope();
        let owner = scope.id();
        let host = host(size, audience);
        let item = host
            .build(|host| reading(host, signal(Some(waiting()))))
            .expect("the reading builds");
        let extent = size.extent();
        let page = || {
            LayoutStyle::new()
                .flex_column()
                .width(extent.width)
                .height(extent.height)
        };
        let root = new_container(page(), &[item.layout_node()]).expect("a root");
        let tree = ComponentList::new(Container::new(page(), vec![item]).expect("a page"));
        compute_layout(
            root,
            AvailableSpace::Definite(extent.width),
            AvailableSpace::Definite(extent.height),
        )
        .expect("the reading lays out");
        let text = tree
            .commands()
            .iter()
            .filter_map(|command| match command {
                DrawCommand::Text { text, .. } => Some(text.to_string()),
                _ => None,
            })
            .collect();
        drop(scope);
        telar::dispose_owner(owner);
        text
    }

    #[test]
    fn a_reading_for_anyone_draws_who_is_waiting_and_never_what_they_said() {
        for size in WidgetSize::ALL {
            let shown = drawn_text(size, Audience::Anyone);
            assert!(
                !shown
                    .iter()
                    .any(|text| text.contains(SECRET_SUMMARY) || text.contains(SECRET_BODY)),
                "{size:?} drew private text on a screen anyone can read: {shown:?}"
            );
            assert!(
                shown.iter().any(|text| text.contains("Signal")),
                "{size:?} lost the public app names: {shown:?}"
            );
        }
    }

    /// The check above has to be able to fail: the same reading built for the owner does draw the summary, so its absence for anyone else is the reveal and not an empty tree.
    #[test]
    fn a_reading_for_the_owner_draws_the_summaries_and_still_no_body() {
        for size in [WidgetSize::M, WidgetSize::L] {
            let shown = drawn_text(size, Audience::Owner);
            assert!(
                shown.iter().any(|text| text.contains(SECRET_SUMMARY)),
                "{size:?}: {shown:?}"
            );
            assert!(!shown.iter().any(|text| text.contains(SECRET_BODY)));
        }
    }

    #[test]
    fn the_newest_come_first_and_each_app_is_named_once() {
        let snapshot = waiting();
        let owner = host(WidgetSize::M, Audience::Owner);
        let ids: Vec<u32> = entries(&snapshot, &owner, 2).iter().map(|e| e.id).collect();
        assert_eq!(ids, vec![3, 2]);
        assert_eq!(apps(&snapshot), vec!["Signal", "Mail"]);
        let anyone = host(WidgetSize::M, Audience::Anyone);
        assert!(
            entries(&snapshot, &anyone, 6)
                .iter()
                .all(|e| e.summary.is_empty())
        );
    }
}
