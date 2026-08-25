//! The weather, from Open-Meteo.
//!
//! Open-Meteo because it needs no API key: a shell that asked its user to register for one before it could show a temperature would ship with the feature effectively off. Two calls, both cached: a geocoding lookup that turns a place name into coordinates (once, and remembered), and the forecast itself.
//!
//! Readings are always Celsius and km/h. Converting at the source would mean every surface having to know which unit the service happened to be configured in; the shell already has one place that turns a temperature into text for a user ([`TemperatureUnit`](config::TemperatureUnit)), and this feeds it.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use platform_wayland::EventSender;
use serde::{Deserialize, Serialize};

use config::WeatherConfig;
use util::broadcast::{Broadcast, Service};
use util::paths;

const FORECAST_URL: &str = "https://api.open-meteo.com/v1/forecast";
const GEOCODE_URL: &str = "https://geocoding-api.open-meteo.com/v1/search";
/// IP geolocation, used only when no location is configured. Chosen because it answers plain JSON over HTTPS with no key and no cookie.
const GEOIP_URL: &str = "https://ipapi.co/json/";

const HTTP_TIMEOUT: Duration = Duration::from_secs(15);

/// How long after a failed fetch before trying again. Far shorter than the refresh interval: a laptop that opened its lid with no network yet should not wait a quarter of an hour for its first reading.
const RETRY: Duration = Duration::from_secs(60);

/// The sky, in the handful of states worth drawing a different icon for.
///
/// WMO's code table has twenty-eight entries that distinguish "slight" from "moderate" drizzle, which is more than any icon set draws and more than a forecast line can say. These are the groups the distinctions collapse into.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Condition {
    Clear,
    MostlyClear,
    Cloudy,
    Overcast,
    Fog,
    Drizzle,
    Rain,
    FreezingRain,
    Snow,
    Showers,
    SnowShowers,
    Thunderstorm,
    #[default]
    Unknown,
}

impl Condition {
    /// Maps a WMO weather code (what Open-Meteo reports) onto a drawable condition.
    pub fn from_wmo(code: i32) -> Self {
        match code {
            0 => Self::Clear,
            1 | 2 => Self::MostlyClear,
            3 => Self::Overcast,
            45 | 48 => Self::Fog,
            51 | 53 | 55 | 56 | 57 => Self::Drizzle,
            61 | 63 | 65 => Self::Rain,
            66 | 67 => Self::FreezingRain,
            71 | 73 | 75 | 77 => Self::Snow,
            80..=82 => Self::Showers,
            85 | 86 => Self::SnowShowers,
            95 | 96 | 99 => Self::Thunderstorm,
            _ => Self::Unknown,
        }
    }

    /// The stable slug, for IPC and config. Deliberately not the translated name: a script branching on the weather must not change behaviour when the user switches the UI language.
    pub fn id(self) -> &'static str {
        match self {
            Self::Clear => "clear",
            Self::MostlyClear => "mostly_clear",
            Self::Cloudy => "cloudy",
            Self::Overcast => "overcast",
            Self::Fog => "fog",
            Self::Drizzle => "drizzle",
            Self::Rain => "rain",
            Self::FreezingRain => "freezing_rain",
            Self::Snow => "snow",
            Self::Showers => "showers",
            Self::SnowShowers => "snow_showers",
            Self::Thunderstorm => "thunderstorm",
            Self::Unknown => "unknown",
        }
    }

    /// The translated description a card shows. One `t!` per condition rather than a key built from [`id`]: the macro checks its key against the catalogs at compile time, and a computed key would opt out of that.
    pub fn label(self) -> String {
        match self {
            Self::Clear => telar::t!("weather.clear"),
            Self::MostlyClear => telar::t!("weather.mostly_clear"),
            Self::Cloudy => telar::t!("weather.cloudy"),
            Self::Overcast => telar::t!("weather.overcast"),
            Self::Fog => telar::t!("weather.fog"),
            Self::Drizzle => telar::t!("weather.drizzle"),
            Self::Rain => telar::t!("weather.rain"),
            Self::FreezingRain => telar::t!("weather.freezing_rain"),
            Self::Snow => telar::t!("weather.snow"),
            Self::Showers => telar::t!("weather.showers"),
            Self::SnowShowers => telar::t!("weather.snow_showers"),
            Self::Thunderstorm => telar::t!("weather.thunderstorm"),
            Self::Unknown => telar::t!("weather.unknown"),
        }
    }
}

/// One day of the forecast. Dates are the API's `YYYY-MM-DD`, which is what a calendar column keys on.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Day {
    pub date: String,
    pub code: i32,
    pub high: f32,
    pub low: f32,
    /// Chance of precipitation, 0–100.
    pub precipitation: i32,
}

impl Day {
    pub fn condition(&self) -> Condition {
        Condition::from_wmo(self.code)
    }
}

/// The current conditions plus the days ahead. Serialised as-is to the cache, so a shell that starts offline shows the last reading it had instead of a blank card.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Weather {
    pub place: String,
    pub code: i32,
    /// Degrees Celsius.
    pub temperature: f32,
    pub feels_like: f32,
    /// Relative humidity, 0–100.
    pub humidity: i32,
    /// Wind speed in km/h.
    pub wind: f32,
    pub is_day: bool,
    pub days: Vec<Day>,
    /// Unix seconds when this reading was fetched — what makes a cached one show its age rather than pass as current.
    pub fetched_at: u64,
}

impl Weather {
    pub fn condition(&self) -> Condition {
        Condition::from_wmo(self.code)
    }

    /// Whether the reading is older than `interval` — a card can then say so instead of showing yesterday's sky as today's.
    pub fn is_stale(&self, interval: Duration) -> bool {
        now().saturating_sub(self.fetched_at) > interval.as_secs()
    }
}

/// `[weather]` resolves to one, so the type is the config's; this is the name the fetch reads it by.
pub use config::policy::Coordinates;

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(HTTP_TIMEOUT))
        .build()
        .into()
}

fn get_json(agent: &ureq::Agent, url: &str) -> Option<serde_json::Value> {
    let body = agent
        .get(url)
        .call()
        .ok()?
        .body_mut()
        .read_to_string()
        .ok()?;
    serde_json::from_str(&body).ok()
}

/// Where to ask about: the configured coordinates, else the configured place name geocoded, else this connection's own approximate location.
///
/// The IP lookup is last on purpose — it is the only step that tells a third party anything, and configuring either of the other two avoids it entirely.
fn locate(agent: &ureq::Agent, config: &WeatherConfig) -> Option<(Coordinates, String)> {
    if let Some(coordinates) = config.coordinates() {
        let place = if config.location.trim().is_empty() {
            format!("{:.2}, {:.2}", coordinates.latitude, coordinates.longitude)
        } else {
            config.location.trim().to_string()
        };
        return Some((coordinates, place));
    }
    let place = config.location.trim();
    if !place.is_empty() {
        return geocode(agent, place);
    }
    geolocate_by_ip(agent)
}

fn geocode(agent: &ureq::Agent, place: &str) -> Option<(Coordinates, String)> {
    let url = format!("{GEOCODE_URL}?name={}&count=1&format=json", encode(place));
    let json = get_json(agent, &url)?;
    let first = json.get("results")?.as_array()?.first()?;
    let coordinates = Coordinates {
        latitude: first.get("latitude")?.as_f64()? as f32,
        longitude: first.get("longitude")?.as_f64()? as f32,
    };
    let name = first
        .get("name")
        .and_then(|n| n.as_str())
        .unwrap_or(place)
        .to_string();
    Some((coordinates, name))
}

fn geolocate_by_ip(agent: &ureq::Agent) -> Option<(Coordinates, String)> {
    let json = get_json(agent, GEOIP_URL)?;
    let coordinates = Coordinates {
        latitude: json.get("latitude")?.as_f64()? as f32,
        longitude: json.get("longitude")?.as_f64()? as f32,
    };
    let name = json
        .get("city")
        .and_then(|c| c.as_str())
        .unwrap_or_default()
        .to_string();
    Some((coordinates, name))
}

/// Percent-encodes a query value. A city name can carry a space or an accent, and only these few characters need escaping for a query string — pulling in a URL crate for one parameter would not earn its place.
fn encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char)
            }
            b' ' => out.push_str("%20"),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

fn forecast_url(at: Coordinates, days: u32) -> String {
    format!(
        "{FORECAST_URL}?latitude={:.4}&longitude={:.4}\
         &current=temperature_2m,relative_humidity_2m,apparent_temperature,is_day,weather_code,wind_speed_10m\
         &daily=weather_code,temperature_2m_max,temperature_2m_min,precipitation_probability_max\
         &timezone=auto&forecast_days={days}",
        at.latitude, at.longitude
    )
}

/// Turns Open-Meteo's answer into a reading. The daily block comes back as parallel arrays rather than a list of objects, which is why the days are zipped by index here.
fn parse(json: &serde_json::Value, place: &str) -> Option<Weather> {
    let current = json.get("current")?;
    let number = |value: Option<&serde_json::Value>| value.and_then(|v| v.as_f64()).unwrap_or(0.0);

    let daily = json.get("daily");
    let column = |name: &str| -> Vec<f64> {
        daily
            .and_then(|d| d.get(name))
            .and_then(|c| c.as_array())
            .map(|values| values.iter().map(|v| v.as_f64().unwrap_or(0.0)).collect())
            .unwrap_or_default()
    };
    let dates: Vec<String> = daily
        .and_then(|d| d.get("time"))
        .and_then(|c| c.as_array())
        .map(|values| {
            values
                .iter()
                .map(|v| v.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default();
    let (codes, highs, lows, chances) = (
        column("weather_code"),
        column("temperature_2m_max"),
        column("temperature_2m_min"),
        column("precipitation_probability_max"),
    );

    let days = dates
        .iter()
        .enumerate()
        .map(|(index, date)| Day {
            date: date.clone(),
            code: codes.get(index).copied().unwrap_or(0.0) as i32,
            high: highs.get(index).copied().unwrap_or(0.0) as f32,
            low: lows.get(index).copied().unwrap_or(0.0) as f32,
            precipitation: chances.get(index).copied().unwrap_or(0.0) as i32,
        })
        .collect();

    Some(Weather {
        place: place.to_string(),
        code: number(current.get("weather_code")) as i32,
        temperature: number(current.get("temperature_2m")) as f32,
        feels_like: number(current.get("apparent_temperature")) as f32,
        humidity: number(current.get("relative_humidity_2m")) as i32,
        wind: number(current.get("wind_speed_10m")) as f32,
        is_day: number(current.get("is_day")) != 0.0,
        days,
        fetched_at: now(),
    })
}

/// One full fetch: locate, ask, parse. `None` on any failure, which the caller treats as "keep what we had".
fn fetch(config: &WeatherConfig) -> Option<Weather> {
    let agent = agent();
    let (at, place) = locate(&agent, config)?;
    let json = get_json(&agent, &forecast_url(at, config.forecast_days()))?;
    parse(&json, &place)
}

fn cache_path() -> std::path::PathBuf {
    paths::cache_dir().join("weather.json")
}

fn load_cache() -> Option<Weather> {
    serde_json::from_str(&std::fs::read_to_string(cache_path()).ok()?).ok()
}

fn save_cache(weather: &Weather) {
    let Ok(text) = serde_json::to_string(weather) else {
        return;
    };
    let path = paths::ensure_dir(paths::cache_dir()).join("weather.json");
    if let Err(e) = std::fs::write(&path, text) {
        tracing::warn!("weather: cannot write {}: {e}", path.display());
    }
}

static WEATHER: Service<Weather> = Service::new("hogar-shell-weather", run);

fn settings() -> WeatherConfig {
    config::shared_config()
        .map(|c| c.weather.clone())
        .unwrap_or_default()
}

/// Publishes the cached reading first so a card has something the moment it opens, then refreshes on the configured interval. A failed refresh keeps the previous reading rather than blanking the card: yesterday's forecast with a visible timestamp is more use than nothing.
fn run(out: &Arc<Broadcast<Weather>>) {
    let config = settings();
    if let Some(cached) = load_cache() {
        out.publish(cached);
    }
    loop {
        let wait = match fetch(&config) {
            Some(weather) => {
                save_cache(&weather);
                out.publish(weather);
                config.refresh()
            }
            None => RETRY,
        };
        if !out.wanted() {
            return;
        }
        std::thread::sleep(wait);
    }
}

/// Registers `tx` for readings — unless `[weather] enabled` is off, in which case no request is ever made. Worth guarding here rather than inside the producer: the first thing the producer does is ask a third party where this connection is, which is not something a disabled section should do at all.
pub fn subscribe(tx: EventSender<Weather>) {
    if !settings().enabled {
        return;
    }
    WEATHER.subscribe(tx);
}

pub fn current() -> Option<Weather> {
    if !settings().enabled {
        return None;
    }
    WEATHER.current()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wmo_codes_collapse_into_the_conditions_worth_drawing() {
        assert_eq!(Condition::from_wmo(0), Condition::Clear);
        assert_eq!(Condition::from_wmo(2), Condition::MostlyClear);
        assert_eq!(Condition::from_wmo(3), Condition::Overcast);
        assert_eq!(Condition::from_wmo(48), Condition::Fog);
        assert_eq!(Condition::from_wmo(55), Condition::Drizzle);
        assert_eq!(Condition::from_wmo(65), Condition::Rain);
        assert_eq!(Condition::from_wmo(75), Condition::Snow);
        assert_eq!(Condition::from_wmo(82), Condition::Showers);
        assert_eq!(Condition::from_wmo(99), Condition::Thunderstorm);
        // A code the table gains later reads as unknown rather than as clear sky.
        assert_eq!(Condition::from_wmo(7), Condition::Unknown);
    }

    #[test]
    fn an_open_meteo_answer_parses_into_current_conditions_and_days() {
        let raw = r#"{
            "current": {"temperature_2m": 18.4, "relative_humidity_2m": 61, "apparent_temperature": 17.2,
                        "is_day": 1, "weather_code": 61, "wind_speed_10m": 12.6},
            "daily": {"time": ["2026-07-26","2026-07-27"], "weather_code": [61, 0],
                      "temperature_2m_max": [21.3, 25.0], "temperature_2m_min": [12.1, 13.4],
                      "precipitation_probability_max": [80, 5]}
        }"#;
        let json: serde_json::Value = serde_json::from_str(raw).unwrap();
        let weather = parse(&json, "Madrid").expect("a normal answer parses");

        assert_eq!(weather.place, "Madrid");
        assert_eq!(weather.temperature, 18.4);
        assert_eq!(weather.humidity, 61);
        assert!(weather.is_day);
        assert_eq!(weather.condition(), Condition::Rain);
        assert_eq!(weather.days.len(), 2);
        assert_eq!(weather.days[0].date, "2026-07-26");
        assert_eq!(weather.days[0].precipitation, 80);
        assert_eq!(weather.days[1].condition(), Condition::Clear);
    }

    #[test]
    fn an_answer_with_no_forecast_still_yields_the_current_conditions() {
        // Not hypothetical: `forecast_days=1` and a request that drops the daily block both land here, and a missing forecast should cost the days rather than the whole reading.
        let json: serde_json::Value =
            serde_json::from_str(r#"{"current": {"temperature_2m": 9.0, "weather_code": 3}}"#)
                .unwrap();
        let weather = parse(&json, "here").expect("the current block is enough");
        assert_eq!(weather.temperature, 9.0);
        assert_eq!(weather.condition(), Condition::Overcast);
        assert!(weather.days.is_empty());
        assert!(!weather.is_day, "an absent field is not invented");

        assert!(
            parse(&serde_json::json!({"error": true}), "here").is_none(),
            "an answer with no current conditions is not a reading"
        );
    }

    #[test]
    fn a_place_name_survives_the_query_string() {
        assert_eq!(encode("Berlin"), "Berlin");
        assert_eq!(encode("San Francisco"), "San%20Francisco");
        assert_eq!(encode("Málaga"), "M%C3%A1laga");
        assert_eq!(
            encode("a&b=c"),
            "a%26b%3Dc",
            "a separator cannot leak into the URL"
        );
    }

    #[test]
    fn a_reading_knows_when_it_has_gone_stale() {
        let fresh = Weather {
            fetched_at: now(),
            ..Weather::default()
        };
        assert!(!fresh.is_stale(Duration::from_secs(900)));
        let old = Weather {
            fetched_at: now().saturating_sub(3600),
            ..Weather::default()
        };
        assert!(old.is_stale(Duration::from_secs(900)));
        // A cache written before the clock was set reads as stale rather than as impossibly fresh.
        assert!(Weather::default().is_stale(Duration::from_secs(900)));
    }

    #[test]
    fn the_forecast_url_carries_everything_a_card_needs() {
        let url = forecast_url(
            Coordinates {
                latitude: 40.4168,
                longitude: -3.7038,
            },
            7,
        );
        assert!(url.contains("latitude=40.4168") && url.contains("longitude=-3.7038"));
        assert!(url.contains("forecast_days=7"));
        assert!(url.contains("timezone=auto"), "days must be local days");
        for field in ["temperature_2m", "weather_code", "temperature_2m_max"] {
            assert!(url.contains(field), "'{field}' is missing from the request");
        }
    }
}
