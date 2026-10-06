//! The column's cards drawn from sample data, for a surface that shows what a stack would do without anything having been posted: the cards are the column's own builders, fed a card the services never heard of.

use telar::{LayoutError, LayoutItem, use_theme};

use config::StackConfig;
use config::policy::Urgency as Policy;
use config::theme::NordTheme;
use layout::{CardKind, Urgency};
use services::notifications::Notification;
use services::toaster::{Event, Toast};
use surfaces::card_samples::Sample;
use surfaces::reconcile;
use ui::chrome::content_radius;
use ui::descriptor::Built;

use crate::osd::OsdKind;

/// The notification id a sample card carries, one no sender is ever given.
const SAMPLE_ID: u32 = u32::MAX;

/// What the card of a sample looks like in the column of the desktop on `output`, drawn by the builder the real card of its kind uses under that desktop's config as it stands when each sample card is drawn, at the width of the box it is put in. It is not offered to the notification daemon, its history or the toaster, takes no swipe, and an OSD in it shows the sample's level rather than the service's.
pub fn preview_card_on(output: &str) -> impl Fn(&Sample) -> Built + use<> {
    let output = output.to_string();
    move |sample| {
        let config =
            reconcile::with_desktop_now(Some(&output), |desktop| (*desktop.config).clone())
                .unwrap_or_default();
        preview_card_in(sample, &config)
    }
}

fn preview_card_in(sample: &Sample, config: &config::Config) -> Built {
    let theme = use_theme::<NordTheme>();
    let radius = content_radius();
    match sample.kind {
        CardKind::Notification => notification(sample, config, theme, radius),
        CardKind::Toast => {
            let toast = Toast::sample(Event::Dnd, &sample.icon, &sample.title, &sample.body);
            crate::toast::card_swiped(&toast, theme, radius, None)
        }
        CardKind::Osd => Ok(crate::osd::osd_card(
            OsdKind::Volume,
            theme,
            sample.level,
            false,
        )),
    }
}

fn notification(
    sample: &Sample,
    config: &config::Config,
    theme: NordTheme,
    radius: f32,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let notification = Notification {
        id: SAMPLE_ID,
        app_name: sample.app.clone().unwrap_or_default(),
        app_icon: sample.icon.clone(),
        summary: sample.title.clone(),
        body: sample.body.clone(),
        actions: Vec::new(),
        urgency: match sample.urgency {
            Some(Urgency::Critical) => Policy::Critical,
            Some(Urgency::Low) => Policy::Low,
            _ => Policy::Normal,
        },
        popup: true,
        image: None,
    };
    let still = StackConfig {
        clear_threshold: 0.0,
        ..config.stack
    };
    crate::notifications::popup_card(&notification, &config.notifications, &still, theme, radius)
}

#[cfg(test)]
mod tests {
    use telar::{AvailableSpace, ComponentList, Container, DrawCommand, LayoutStyle};

    use crate::test_support::fresh;

    use surfaces::card_samples::Shown;
    use surfaces::transient;

    use super::*;

    const WIDTH: f32 = 380.0;

    fn drawn(card: Box<dyn LayoutItem>) -> Vec<String> {
        crate::test_support::drawn(card, WIDTH, 600.0)
    }

    /// A sample notification is drawn by the builder the column draws a notification with: laid out in the same width beside a real one with the same sender, summary and body, every command is alike.
    #[test]
    fn a_sample_notification_is_the_real_card() {
        let sample = Sample::notification(
            "Signal",
            Urgency::Normal,
            "Marta".to_string(),
            "Are we meeting at 8?".to_string(),
        );
        fresh();
        let preview = drawn(preview_card_on("sample")(&sample).expect("the sample builds"));

        fresh();
        let config = config::Config::default();
        let real = Notification {
            id: 7,
            app_name: "Signal".into(),
            app_icon: String::new(),
            summary: "Marta".into(),
            body: "Are we meeting at 8?".into(),
            actions: Vec::new(),
            urgency: Policy::Normal,
            popup: true,
            image: None,
        };
        let stack = StackConfig {
            clear_threshold: 0.0,
            ..config.stack
        };
        let real = drawn(
            crate::notifications::popup_card(
                &real,
                &config.notifications,
                &stack,
                NordTheme::new(),
                content_radius(),
            )
            .expect("the real card builds"),
        );
        assert!(!preview.is_empty());
        assert_eq!(preview, real);
    }

    /// An OSD sample shows its own level, whatever the volume service last said.
    #[test]
    fn a_sample_osd_shows_its_own_level() {
        fresh();
        services::volume::seed(services::volume::Volume {
            level: 10,
            muted: false,
        });
        let at = |level| {
            fresh();
            drawn(preview_card_on("sample")(&Sample::osd(level)).expect("the sample builds"))
        };
        assert_ne!(at(30), at(80));
        assert_eq!(at(30), at(30));
    }

    const SCREEN: telar::Rect = telar::Rect {
        x: 0.0,
        y: 0.0,
        width: 1920.0,
        height: 1080.0,
    };

    /// Where three toasts are drawn, in the order the stack lists them, laid out in a row at `anchor`.
    fn toasts_in_a_row(anchor: layout::Anchor) -> Vec<telar::Rect> {
        fresh();
        let scope = telar::owner_scope();
        let titles = ["First", "Second", "Third"];
        let shown: Vec<Shown> = titles
            .iter()
            .enumerate()
            .map(|(at, title)| {
                let mut sample = Sample::toast("icon", title.to_string(), String::new());
                sample.id = at as u64;
                Shown::Card(sample)
            })
            .collect();
        let lane = surfaces::pinned::row(SCREEN, anchor, layout::Offset::ZERO);
        let list = surfaces::card_samples::lane(
            move || shown.clone(),
            move || Some((lane, anchor)),
            layout::StackFlow::Row,
            preview_card_on("sample"),
            |_: &surfaces::card_samples::Launcher| {
                Err(telar::LayoutError::Engine("no launcher".into()))
            },
        )
        .expect("the row builds");
        let page = Container::new(
            LayoutStyle::new().width(SCREEN.width).height(SCREEN.height),
            vec![list],
        )
        .expect("a screen");
        let root = page.layout_node();
        let tree = ComponentList::new(page);
        telar::compute_layout(
            root,
            AvailableSpace::Definite(SCREEN.width),
            AvailableSpace::Definite(SCREEN.height),
        )
        .expect("the row lays out");
        let mut cards = Vec::new();
        let mut biggest: Option<telar::Rect> = None;
        for command in tree.commands().iter() {
            match command {
                DrawCommand::Rect { rect, .. } if rect.width < SCREEN.width / 2.0 => {
                    if biggest
                        .is_none_or(|held| rect.width * rect.height > held.width * held.height)
                    {
                        biggest = Some(*rect);
                    }
                }
                DrawCommand::Text { text, .. } if titles.contains(&&**text) => {
                    cards.extend(biggest.take());
                }
                _ => {}
            }
        }
        drop(tree);
        telar::dispose_owner(scope.id());
        cards
    }

    /// Three toasts in a bottom-anchored row sit side by side on one line against the bottom of the screen, in the order they arrived, a gap apart.
    #[test]
    fn a_bottom_row_lays_three_toasts_side_by_side() {
        let gap = ui::chrome::card_gap();
        let cards = toasts_in_a_row(layout::Anchor::Bottom);
        assert_eq!(cards.len(), 3, "{cards:?}");
        for pair in cards.windows(2) {
            assert_eq!(pair[1].x, pair[0].x + pair[0].width + gap, "{cards:?}");
            assert_eq!(pair[0].y, pair[1].y, "all on one line: {cards:?}");
        }
        assert_eq!(
            cards[0].y + cards[0].height,
            SCREEN.height - transient::DEFAULT_GAP
        );
    }

    /// A row grows from the side its anchor names: from the left edge to the right, from the right edge to the left, and from the middle both ways.
    #[test]
    fn a_row_grows_from_its_anchors_side() {
        let edge = transient::DEFAULT_GAP;
        let from_left = toasts_in_a_row(layout::Anchor::BottomLeft);
        assert_eq!(from_left.len(), 3);
        assert_eq!(from_left[0].x, edge, "the first card is at the left edge");

        let from_right = toasts_in_a_row(layout::Anchor::BottomRight);
        assert_eq!(from_right.len(), 3);
        let last = from_right[2];
        assert_eq!(
            last.x + last.width,
            SCREEN.width - edge,
            "the last card is at the right edge"
        );

        let from_middle = toasts_in_a_row(layout::Anchor::Bottom);
        let (start, end) = (from_middle[0].x, from_middle[2].x + from_middle[2].width);
        assert!(
            ((start + end) / 2.0 - SCREEN.width / 2.0).abs() < 1.0,
            "centred on the middle: {from_middle:?}"
        );
    }
}
