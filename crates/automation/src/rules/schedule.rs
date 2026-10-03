//! When a `schedule` or `every` rule is due, worked out against a clock the caller passes in, so the arithmetic is tested without waiting for one.

use std::ops::Range;
use std::time::{Duration, Instant};

use chrono::{Datelike, NaiveDateTime, NaiveTime, TimeDelta, Weekday};
use util::report::Message;

/// Times of day and the days they apply on, as a `schedule` trigger writes them: `07:30, 19:00 mon-fri`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Schedule {
    times: Vec<NaiveTime>,
    days: [bool; 7],
}

/// What is wrong with a schedule, and which bytes of it say so.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScheduleError {
    pub span: Range<usize>,
    pub message: Message,
}

impl Schedule {
    /// Reads `07:30`, `07:30, 19:00`, `08:00 mon-fri`, `10:00 sat,sun`, `09:00 weekdays` or `11:00 weekends`: times as `HH:MM`, then the days they apply on, every day when none is named.
    pub fn parse(text: &str) -> Result<Self, ScheduleError> {
        let mut times = Vec::new();
        let mut days = [false; 7];
        let mut named_a_day = false;
        for (word, span) in words(text) {
            if word.contains(':') {
                times.push(time(word).ok_or_else(|| ScheduleError {
                    span,
                    message: util::message!("finding.schedule_time", word = word),
                })?);
                continue;
            }
            let named = weekdays(word).ok_or_else(|| ScheduleError {
                span,
                message: util::message!("finding.schedule_word", word = word),
            })?;
            for day in named {
                days[day.num_days_from_monday() as usize] = true;
            }
            named_a_day = true;
        }
        if times.is_empty() {
            return Err(ScheduleError {
                span: 0..text.len(),
                message: util::message!("finding.schedule_needs_time"),
            });
        }
        times.sort_unstable();
        times.dedup();
        if !named_a_day {
            days = [true; 7];
        }
        Ok(Self { times, days })
    }

    /// The first moment after `now` the schedule names.
    pub fn next_after(&self, now: NaiveDateTime) -> NaiveDateTime {
        for ahead in 0..=7 {
            let date = now.date() + TimeDelta::days(ahead);
            if !self.days[date.weekday().num_days_from_monday() as usize] {
                continue;
            }
            if let Some(at) = self
                .times
                .iter()
                .map(|time| date.and_time(*time))
                .find(|at| *at > now)
            {
                return at;
            }
        }
        unreachable!("a parsed schedule names at least one time on at least one day of every week")
    }
}

/// Each word of `text`, split on spaces and commas, with where it sits.
fn words(text: &str) -> Vec<(&str, Range<usize>)> {
    let separator = |c: char| c.is_whitespace() || c == ',';
    let mut found = Vec::new();
    let mut start = None;
    for (at, c) in text.char_indices().chain([(text.len(), ' ')]) {
        match (separator(c), start) {
            (true, Some(from)) => {
                found.push((&text[from..at], from..at));
                start = None;
            }
            (false, None) => start = Some(at),
            _ => {}
        }
    }
    found
}

fn time(word: &str) -> Option<NaiveTime> {
    let (hours, minutes) = word.split_once(':')?;
    if hours.is_empty() || hours.len() > 2 || minutes.len() != 2 {
        return None;
    }
    NaiveTime::from_hms_opt(hours.parse().ok()?, minutes.parse().ok()?, 0)
}

fn weekdays(word: &str) -> Option<Vec<Weekday>> {
    let word = word.to_ascii_lowercase();
    match word.as_str() {
        "weekdays" => return Some(span_of(Weekday::Mon, Weekday::Fri)),
        "weekends" => return Some(vec![Weekday::Sat, Weekday::Sun]),
        _ => {}
    }
    match word.split_once('-') {
        Some((from, to)) => Some(span_of(day(from)?, day(to)?)),
        None => Some(vec![day(&word)?]),
    }
}

fn day(word: &str) -> Option<Weekday> {
    Some(match word {
        "mon" => Weekday::Mon,
        "tue" => Weekday::Tue,
        "wed" => Weekday::Wed,
        "thu" => Weekday::Thu,
        "fri" => Weekday::Fri,
        "sat" => Weekday::Sat,
        "sun" => Weekday::Sun,
        _ => return None,
    })
}

/// The days from `from` to `to`, wrapping past Sunday: `fri-mon` is four days.
fn span_of(from: Weekday, to: Weekday) -> Vec<Weekday> {
    let mut days = vec![from];
    let mut day = from;
    while day != to {
        day = day.succ();
        days.push(day);
    }
    days
}

/// How late a scheduled time may be noticed and still count: past this the machine was asleep through it, and it is skipped rather than run late.
const GRACE: TimeDelta = TimeDelta::minutes(1);

/// A [`Schedule`] being kept: the next time it names, and whether a reading of the clock has reached it.
#[derive(Clone, Debug)]
pub struct Alarm {
    schedule: Schedule,
    next: NaiveDateTime,
}

impl Alarm {
    pub fn new(schedule: Schedule, now: NaiveDateTime) -> Self {
        let next = schedule.next_after(now);
        Self { schedule, next }
    }

    /// Whether the clock reading `now` reaches the next time, answering `true` once for it. A time passed by more than a minute — the machine slept through it — answers `false` and is skipped.
    pub fn ring(&mut self, now: NaiveDateTime) -> bool {
        if now < self.next {
            return false;
        }
        let late = now - self.next;
        self.next = self.schedule.next_after(now);
        late <= GRACE
    }

    /// How long from `now` until the next time.
    pub fn wait(&self, now: NaiveDateTime) -> Duration {
        (self.next - now).to_std().unwrap_or(Duration::ZERO)
    }

    pub fn next(&self) -> NaiveDateTime {
        self.next
    }
}

/// An `every` interval being kept, counted from when the rule was loaded.
#[derive(Clone, Debug)]
pub struct Metronome {
    period: Duration,
    next: Instant,
}

impl Metronome {
    pub fn new(period: Duration, now: Instant) -> Self {
        Self {
            period,
            next: now + period,
        }
    }

    /// Whether `now` has reached the next beat, answering `true` once however many beats a stall let pass.
    pub fn beat(&mut self, now: Instant) -> bool {
        if now < self.next {
            return false;
        }
        while self.next <= now {
            self.next += self.period;
        }
        true
    }

    pub fn wait(&self, now: Instant) -> Duration {
        self.next.saturating_duration_since(now)
    }
}

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;

    use super::*;

    fn at(day: u32, hour: u32, minute: u32, second: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 10, day)
            .unwrap()
            .and_hms_opt(hour, minute, second)
            .unwrap()
    }

    #[test]
    fn a_schedule_rings_at_each_time_on_its_days_against_an_injected_clock() {
        // 2026-10-02 is a Friday.
        let schedule = Schedule::parse("07:30, 19:00 mon-fri").unwrap();
        let mut alarm = Alarm::new(schedule, at(2, 7, 0, 0));
        assert_eq!(alarm.next(), at(2, 7, 30, 0));
        assert_eq!(alarm.wait(at(2, 7, 0, 0)), Duration::from_secs(30 * 60));

        assert!(!alarm.ring(at(2, 7, 29, 59)));
        assert!(alarm.ring(at(2, 7, 30, 0)));
        assert!(!alarm.ring(at(2, 7, 30, 1)), "once for each time");
        assert_eq!(alarm.next(), at(2, 19, 0, 0));
        assert!(alarm.ring(at(2, 19, 0, 1)));
        assert_eq!(
            alarm.next(),
            at(5, 7, 30, 0),
            "the weekend is skipped: Monday morning is next"
        );
    }

    #[test]
    fn a_time_the_machine_slept_through_is_skipped_rather_than_run_late() {
        let mut alarm = Alarm::new(Schedule::parse("08:00").unwrap(), at(1, 7, 0, 0));
        assert!(
            !alarm.ring(at(1, 9, 15, 0)),
            "an hour late is a missed time"
        );
        assert_eq!(alarm.next(), at(2, 8, 0, 0));
        assert!(
            alarm.ring(at(2, 8, 0, 45)),
            "a few seconds late still counts"
        );
    }

    #[test]
    fn a_schedule_reads_days_as_names_ranges_and_groups() {
        let sunday_morning = at(4, 6, 0, 0);
        let next = |text: &str| Schedule::parse(text).unwrap().next_after(sunday_morning);
        assert_eq!(next("10:00 weekends"), at(4, 10, 0, 0));
        assert_eq!(next("10:00 weekdays"), at(5, 10, 0, 0));
        assert_eq!(next("05:00, 9:15 sat,sun"), at(4, 9, 15, 0));
        assert_eq!(next("10:00 fri-mon"), at(4, 10, 0, 0));
        assert_eq!(next("05:00"), at(5, 5, 0, 0));
        assert_eq!(
            Schedule::parse("tue").unwrap_err().message.key(),
            Some("finding.schedule_needs_time")
        );
        assert_eq!(Schedule::parse("25:00").unwrap_err().span, 0..5);
    }

    #[test]
    fn an_interval_beats_once_however_long_a_stall() {
        let start = Instant::now();
        let mut metronome = Metronome::new(Duration::from_secs(10), start);
        assert!(!metronome.beat(start + Duration::from_secs(9)));
        assert!(metronome.beat(start + Duration::from_secs(10)));
        assert!(!metronome.beat(start + Duration::from_secs(11)));
        assert!(metronome.beat(start + Duration::from_secs(45)));
        assert_eq!(
            metronome.wait(start + Duration::from_secs(45)),
            Duration::from_secs(5),
            "and keeps its phase"
        );
    }
}
