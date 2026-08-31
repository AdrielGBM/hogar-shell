//! The Caps- and Num-Lock indicators.

telar::rsx_modules!(::config::theme::NordTheme);

use telar::{Color, LayoutError, LayoutItem, ReadSignal};

use config::LockStatusConfig;
use services::lockkeys::LockKeys;

/// Which lock an indicator stands for. Doubles as the reactive list's key, so one indicator appearing or going away never rebuilds the other.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Lock {
    Caps,
    Num,
}

impl Lock {
    pub fn glyph(self) -> &'static str {
        match self {
            Lock::Caps => ui::glyph::caps_lock(),
            Lock::Num => ui::glyph::num_lock(),
        }
    }

    pub fn engaged(self, keys: LockKeys) -> bool {
        match self {
            Lock::Caps => keys.caps,
            Lock::Num => keys.num,
        }
    }
}

/// The indicators to draw: the ones `[lock_status]` enables, minus the idle ones when `hide_inactive` asks for a bar that only speaks up while a lock is actually engaged.
pub fn shown(keys: LockKeys, config: LockStatusConfig) -> Vec<Lock> {
    [(Lock::Caps, config.caps), (Lock::Num, config.num)]
        .into_iter()
        .filter(|(_, enabled)| *enabled)
        .map(|(lock, _)| lock)
        .filter(|lock| !config.hide_inactive || lock.engaged(keys))
        .collect()
}

/// One indicator glyph, tinted live from the lock state.
///
/// Built here rather than in the view because the view's `for` is reactive: it constructs each item afresh whenever that lock comes back, so its content has to be an expression (`build`) rather than a widget bound once in `[logic]`. Engaged takes the chip's own foreground, so it reads at full strength under every container variant; idle recedes to `idle` rather than vanishing, which keeps the module visible — and the bar's width stable — as soon as it is added.
#[derive(telar::Props)]
pub struct IndicatorProps {
    pub lock: Lock,
    pub keys: ReadSignal<LockKeys>,
    pub fg: ReadSignal<Color>,
    pub idle: Color,
    pub size: f32,
}

pub fn indicator(
    props: IndicatorProps,
    _children: telar::Children,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let IndicatorProps { lock, keys, fg, idle, size } = props;
    ui::icon::icon_view(
        move || lock.glyph().to_string(),
        move || {
            if lock.engaged(keys.get()) {
                fg.get()
            } else {
                idle
            }
        },
        size,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOTH_ON: LockKeys = LockKeys {
        caps: true,
        num: true,
    };

    #[test]
    fn both_indicators_stay_in_place_by_default() {
        let config = LockStatusConfig::default();
        assert_eq!(
            shown(LockKeys::default(), config),
            vec![Lock::Caps, Lock::Num],
            "an idle indicator is drawn muted, so the bar's width never shifts"
        );
        assert_eq!(shown(BOTH_ON, config), vec![Lock::Caps, Lock::Num]);
    }

    #[test]
    fn hide_inactive_leaves_only_the_engaged_locks() {
        let config = LockStatusConfig {
            hide_inactive: true,
            ..LockStatusConfig::default()
        };
        assert!(shown(LockKeys::default(), config).is_empty());
        assert_eq!(
            shown(
                LockKeys {
                    caps: true,
                    num: false
                },
                config
            ),
            vec![Lock::Caps]
        );
        assert_eq!(shown(BOTH_ON, config), vec![Lock::Caps, Lock::Num]);
    }

    #[test]
    fn a_disabled_indicator_never_appears_however_the_key_is_set() {
        let config = LockStatusConfig {
            caps: false,
            num: true,
            hide_inactive: false,
        };
        assert_eq!(shown(BOTH_ON, config), vec![Lock::Num]);
        assert_eq!(shown(LockKeys::default(), config), vec![Lock::Num]);
    }
}
