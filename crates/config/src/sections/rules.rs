use serde::{Deserialize, Serialize};

/// A rule (`[[rules]]`): when something happens, run commands, and optionally keep a value in a variable.
///
/// A rule is session automation rather than something drawn, so it lives here rather than in a layout, means the same thing whichever layout is on screen, and keeps running while the session is locked. Its expressions read what a layout binding reads — a module's readings (`$battery.level`), a variable (`$name`) and the last event of a kind (`$event.session_locked`) — but not a layout's own `[sources]`. `hogar-shell rule list` shows every rule and when it last fired; `hogar-shell rule run <id>` runs one's commands now.
///
/// A rule that fires more than 10 times within one second is taken to be caught in a loop — two rules setting each other off through a variable, or one setting itself off — so it is suspended, and reported as failing, until the config is next loaded. A rule cannot run a rule: `rule run` in `run` is refused.
///
/// For example, `id = "low-battery"`, `trigger = { edge = "$battery.level < 15 && !$battery.charging" }` and `run = ["toast show Battery low"]` says so once each time the charge drops under 15 %.
#[derive(Deserialize, Serialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct RuleConfig {
    /// The rule's name: what `hogar-shell rule run` takes and what a problem with it is reported under. Letters, digits, `-` and `_`, and no two rules share one.
    pub id: String,
    /// What sets the rule off: exactly one of `event`, `edge`, `schedule` or `every`.
    pub trigger: RuleTrigger,
    /// An expression checked each time the trigger fires, which has to give `true` for the rule to run. Unset, the rule runs every time it fires.
    pub when: Option<String>,
    /// The commands the rule runs, in order, each a line `hogar-shell --list` names (`toast show hi`, `var set mood calm`, `shell run notify-send hi`). Checked without being run when the config loads; the first one the shell refuses stops the rest.
    pub run: Vec<String>,
    /// A value kept in a variable after the commands have run: `{ var = "<name>", value = "<expression>" }`, the expression evaluated as the rule fires and the variable taking its type. A variable is readable on the lock screen, so `config check` warns about a value that reads something the lock screen hides.
    pub store: Option<RuleStore>,
    /// Off keeps the rule written down without running it; `hogar-shell rule run` still runs its commands.
    pub enabled: bool,
}

impl RuleConfig {
    /// The most times a rule may fire within one second before it is suspended as caught in a loop; the `[[rules]]` doc says so in words.
    pub const FIRINGS_PER_SECOND: u32 = 10;
}

impl Default for RuleConfig {
    fn default() -> Self {
        Self {
            id: String::new(),
            trigger: RuleTrigger::default(),
            when: None,
            run: Vec::new(),
            store: None,
            enabled: true,
        }
    }
}

/// What sets a rule off. Exactly one of the four is written.
#[derive(Deserialize, Serialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(default)]
pub struct RuleTrigger {
    /// An event, by name: `started`, `wallpaper_changed`, `colors_changed`, `theme_mode_changed`, `session_locked`, `session_unlocked`, `logging_out`, `rebooting`, `shutting_down`, `wifi_enabled`, `wifi_disabled`, `bluetooth_enabled`, `bluetooth_disabled`, `battery_state_changed`, `battery_under_threshold` or `power_profile_changed` (the scripting guide says when each is raised). Fires once for each such event after the rule is loaded.
    pub event: Option<String>,
    /// An expression that gives `true` or `false`. Fires each time it turns from `false` to `true`, once per crossing however often its readings change. Its first answer only records: a rule loaded while the expression already holds waits for the next crossing.
    pub edge: Option<String>,
    /// Times of day, local, as `HH:MM`, with the days they apply on: `07:30`, `07:30, 19:00`, `08:00 mon-fri`, `10:00 sat,sun`, `09:00 weekdays`, `11:00 weekends`. Every day when no day is named. A time the machine slept through is skipped, not run late.
    pub schedule: Option<String>,
    /// An interval: `30s`, `5m`, `1h`. Fires that long after the rule is loaded and every that long after; never more often than `[automation] min_interval_seconds`.
    pub every: Option<String>,
}

impl RuleTrigger {
    /// The keys a trigger is written with, in the order they are listed and checked.
    pub const KEYS: [&'static str; 4] = ["event", "edge", "schedule", "every"];

    /// Each key this trigger writes, with its value, in [`RuleTrigger::KEYS`] order.
    pub fn written(&self) -> Vec<(&'static str, &str)> {
        Self::KEYS
            .into_iter()
            .zip([&self.event, &self.edge, &self.schedule, &self.every])
            .filter_map(|(key, value)| value.as_deref().map(|value| (key, value)))
            .collect()
    }
}

/// Where a rule keeps a value: a variable, and the expression that gives it.
#[derive(Deserialize, Serialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(default)]
pub struct RuleStore {
    /// The variable's name, as an expression reads it without the `$`.
    pub var: String,
    /// The expression whose value the variable is set to.
    pub value: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_trigger_lists_what_it_writes_in_key_order() {
        let trigger = RuleTrigger {
            every: Some("5m".to_string()),
            event: Some("started".to_string()),
            ..RuleTrigger::default()
        };
        assert_eq!(trigger.written(), [("event", "started"), ("every", "5m")]);
        assert!(RuleTrigger::default().written().is_empty());
    }
}
