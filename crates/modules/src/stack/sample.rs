//! The column's cards drawn from sample data, for a surface that shows what a stack would do without anything having been posted: the cards are the column's own builders, fed a card the services never heard of.

use telar::{LayoutError, LayoutItem, use_theme};

use config::StackConfig;
use config::policy::Urgency as Policy;
use config::theme::NordTheme;
use layout::{CardKind, Urgency};
use services::notifications::Notification;
use services::toaster::{Event, Toast};
use surfaces::card_samples::Sample;
use ui::chrome::content_radius;
use ui::descriptor::Built;

use crate::osd::OsdKind;

/// The notification id a sample card carries, one no sender is ever given.
const SAMPLE_ID: u32 = u32::MAX;

/// What the card of `sample` looks like in the column, drawn by the builder the real card of its kind uses, at the width of the box it is put in. It is not offered to the notification daemon, its history or the toaster, takes no swipe, and an OSD in it shows the sample's level rather than the service's.
pub fn preview_card(sample: &Sample) -> Built {
    let theme = use_theme::<NordTheme>();
    let radius = content_radius();
    let config = config::config().map(|c| (*c).clone()).unwrap_or_default();
    match sample.kind {
        CardKind::Notification => notification(sample, &config, theme, radius),
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

    use super::*;

    const WIDTH: f32 = 380.0;

    fn drawn(card: Box<dyn LayoutItem>) -> Vec<String> {
        let page = Container::new(LayoutStyle::new().width(WIDTH).height(600.0), vec![card])
            .expect("a page");
        let root = page.layout_node();
        let tree = ComponentList::new(page);
        telar::compute_layout(
            root,
            AvailableSpace::Definite(WIDTH),
            AvailableSpace::Definite(600.0),
        )
        .expect("the card lays out");
        tree.commands()
            .iter()
            .filter(|command| {
                !matches!(
                    command,
                    DrawCommand::PushElement { .. } | DrawCommand::PopElement
                )
            })
            .map(|command| format!("{command:?}"))
            .collect()
    }

    fn fresh() {
        telar::reset_layout_runtime();
        telar::set_theme(NordTheme::new());
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
        let preview = drawn(preview_card(&sample).expect("the sample builds"));

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
            drawn(preview_card(&Sample::osd(level)).expect("the sample builds"))
        };
        assert_ne!(at(30), at(80));
        assert_eq!(at(30), at(30));
    }
}
