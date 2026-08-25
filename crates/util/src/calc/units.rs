//! Unit conversion for the launcher's calculator: `3 km in mi`, `100 c in f`, `1 gib in mb`.
//!
//! A static table rather than a dependency, for the same reason the evaluator next door is one: this runs on every keystroke, and the whole feature is a few hundred rows of arithmetic. Every unit converts through one base per dimension, and a conversion between two dimensions is refused rather than guessed — a query that is really an application name must fall through to the app search.
//!
//! Temperature is why the conversion is affine rather than a ratio: 0 °C is not 0 K, and scaling alone would put freezing water at absolute zero.

use super::evaluate;

/// What kind of quantity a unit measures. Two units only convert into each other within one of these.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Dimension {
    Length,
    Mass,
    Time,
    Data,
    Temperature,
    Speed,
    Angle,
    Area,
    Volume,
}

/// One unit, and how to get from it to its dimension's base: `base = value * scale + offset`.
struct Unit {
    /// What an answer in this unit is labelled with.
    symbol: &'static str,
    /// Every spelling that resolves here, lowercase. The first is not special; the symbol is.
    names: &'static [&'static str],
    dimension: Dimension,
    scale: f64,
    offset: f64,
}

const fn unit(
    symbol: &'static str,
    names: &'static [&'static str],
    dimension: Dimension,
    scale: f64,
) -> Unit {
    Unit {
        symbol,
        names,
        dimension,
        scale,
        offset: 0.0,
    }
}

/// The table. Bases: metre, gram, second, byte, kelvin, metre per second, radian, square metre, litre.
static UNITS: &[Unit] = &[
    unit(
        "m",
        &["m", "metre", "metres", "meter", "meters"],
        Dimension::Length,
        1.0,
    ),
    unit(
        "km",
        &["km", "kilometre", "kilometres", "kilometer", "kilometers"],
        Dimension::Length,
        1000.0,
    ),
    unit(
        "cm",
        &[
            "cm",
            "centimetre",
            "centimetres",
            "centimeter",
            "centimeters",
        ],
        Dimension::Length,
        0.01,
    ),
    unit(
        "mm",
        &[
            "mm",
            "millimetre",
            "millimetres",
            "millimeter",
            "millimeters",
        ],
        Dimension::Length,
        0.001,
    ),
    unit(
        "µm",
        &["um", "µm", "micrometre", "micrometer", "micron", "microns"],
        Dimension::Length,
        1e-6,
    ),
    unit(
        "nm",
        &["nm", "nanometre", "nanometer"],
        Dimension::Length,
        1e-9,
    ),
    unit("mi", &["mi", "mile", "miles"], Dimension::Length, 1609.344),
    unit("yd", &["yd", "yard", "yards"], Dimension::Length, 0.9144),
    unit("ft", &["ft", "foot", "feet"], Dimension::Length, 0.3048),
    unit("in", &["in", "inch", "inches"], Dimension::Length, 0.0254),
    unit(
        "nmi",
        &["nmi", "nauticalmile", "nauticalmiles"],
        Dimension::Length,
        1852.0,
    ),
    unit(
        "ly",
        &["ly", "lightyear", "lightyears"],
        Dimension::Length,
        9.4607304725808e15,
    ),
    unit(
        "g",
        &["g", "gram", "grams", "gramme", "grammes"],
        Dimension::Mass,
        1.0,
    ),
    unit(
        "kg",
        &["kg", "kilogram", "kilograms", "kilo", "kilos"],
        Dimension::Mass,
        1000.0,
    ),
    unit(
        "mg",
        &["mg", "milligram", "milligrams"],
        Dimension::Mass,
        0.001,
    ),
    unit(
        "t",
        &["t", "tonne", "tonnes", "metricton"],
        Dimension::Mass,
        1e6,
    ),
    unit(
        "lb",
        &["lb", "lbs", "pound", "pounds"],
        Dimension::Mass,
        453.59237,
    ),
    unit(
        "oz",
        &["oz", "ounce", "ounces"],
        Dimension::Mass,
        28.349523125,
    ),
    unit(
        "st",
        &["st", "stone", "stones"],
        Dimension::Mass,
        6350.29318,
    ),
    unit(
        "s",
        &["s", "sec", "secs", "second", "seconds"],
        Dimension::Time,
        1.0,
    ),
    unit(
        "ms",
        &["ms", "millisecond", "milliseconds"],
        Dimension::Time,
        1e-3,
    ),
    unit(
        "µs",
        &["us", "µs", "microsecond", "microseconds"],
        Dimension::Time,
        1e-6,
    ),
    unit(
        "ns",
        &["ns", "nanosecond", "nanoseconds"],
        Dimension::Time,
        1e-9,
    ),
    unit(
        "min",
        &["min", "mins", "minute", "minutes"],
        Dimension::Time,
        60.0,
    ),
    unit(
        "h",
        &["h", "hr", "hrs", "hour", "hours"],
        Dimension::Time,
        3600.0,
    ),
    unit("d", &["d", "day", "days"], Dimension::Time, 86400.0),
    unit("wk", &["wk", "week", "weeks"], Dimension::Time, 604800.0),
    // A year is the Julian one, which is what makes `1 y in d` answer 365.25 rather than depending on which year.
    unit(
        "y",
        &["y", "yr", "yrs", "year", "years"],
        Dimension::Time,
        31557600.0,
    ),
    unit("B", &["b", "byte", "bytes"], Dimension::Data, 1.0),
    unit("bit", &["bit", "bits"], Dimension::Data, 0.125),
    // SI for the decimal prefixes and IEC for the binary ones, so `1 gb in gib` is a real question with a real answer instead of both spellings meaning whichever the shell picked.
    unit("kB", &["kb", "kilobyte", "kilobytes"], Dimension::Data, 1e3),
    unit("MB", &["mb", "megabyte", "megabytes"], Dimension::Data, 1e6),
    unit("GB", &["gb", "gigabyte", "gigabytes"], Dimension::Data, 1e9),
    unit(
        "TB",
        &["tb", "terabyte", "terabytes"],
        Dimension::Data,
        1e12,
    ),
    unit(
        "PB",
        &["pb", "petabyte", "petabytes"],
        Dimension::Data,
        1e15,
    ),
    unit(
        "KiB",
        &["kib", "kibibyte", "kibibytes"],
        Dimension::Data,
        1024.0,
    ),
    unit(
        "MiB",
        &["mib", "mebibyte", "mebibytes"],
        Dimension::Data,
        1048576.0,
    ),
    unit(
        "GiB",
        &["gib", "gibibyte", "gibibytes"],
        Dimension::Data,
        1073741824.0,
    ),
    unit(
        "TiB",
        &["tib", "tebibyte", "tebibytes"],
        Dimension::Data,
        1099511627776.0,
    ),
    Unit {
        symbol: "°C",
        names: &["c", "°c", "celsius", "centigrade"],
        dimension: Dimension::Temperature,
        scale: 1.0,
        offset: 273.15,
    },
    Unit {
        symbol: "°F",
        names: &["f", "°f", "fahrenheit"],
        dimension: Dimension::Temperature,
        scale: 5.0 / 9.0,
        offset: 255.37222222222223,
    },
    Unit {
        symbol: "K",
        names: &["k", "kelvin"],
        dimension: Dimension::Temperature,
        scale: 1.0,
        offset: 0.0,
    },
    unit(
        "m/s",
        &["mps", "m/s", "metrepersecond"],
        Dimension::Speed,
        1.0,
    ),
    unit(
        "km/h",
        &["kmh", "kph", "km/h", "kmph"],
        Dimension::Speed,
        1.0 / 3.6,
    ),
    unit("mph", &["mph", "mi/h"], Dimension::Speed, 0.44704),
    unit("ft/s", &["fps", "ft/s"], Dimension::Speed, 0.3048),
    unit(
        "kn",
        &["kn", "kt", "knot", "knots"],
        Dimension::Speed,
        0.5144444444444445,
    ),
    unit("rad", &["rad", "radian", "radians"], Dimension::Angle, 1.0),
    unit(
        "°",
        &["deg", "degree", "degrees", "°"],
        Dimension::Angle,
        std::f64::consts::PI / 180.0,
    ),
    unit(
        "grad",
        &["grad", "gradian", "gradians"],
        Dimension::Angle,
        std::f64::consts::PI / 200.0,
    ),
    unit(
        "m²",
        &["m2", "m²", "sqm", "squaremetre", "squaremeter"],
        Dimension::Area,
        1.0,
    ),
    unit("km²", &["km2", "km²", "sqkm"], Dimension::Area, 1e6),
    unit("cm²", &["cm2", "cm²"], Dimension::Area, 1e-4),
    unit("ft²", &["ft2", "ft²", "sqft"], Dimension::Area, 0.09290304),
    unit(
        "mi²",
        &["mi2", "mi²", "sqmi"],
        Dimension::Area,
        2589988.110336,
    ),
    unit(
        "ha",
        &["ha", "hectare", "hectares"],
        Dimension::Area,
        10000.0,
    ),
    unit("acre", &["acre", "acres"], Dimension::Area, 4046.8564224),
    unit(
        "l",
        &["l", "litre", "litres", "liter", "liters"],
        Dimension::Volume,
        1.0,
    ),
    unit(
        "ml",
        &[
            "ml",
            "millilitre",
            "millilitres",
            "milliliter",
            "milliliters",
        ],
        Dimension::Volume,
        0.001,
    ),
    unit(
        "cl",
        &["cl", "centilitre", "centiliter"],
        Dimension::Volume,
        0.01,
    ),
    unit(
        "m³",
        &["m3", "m³", "cubicmetre", "cubicmeter"],
        Dimension::Volume,
        1000.0,
    ),
    unit(
        "gal",
        &["gal", "gallon", "gallons"],
        Dimension::Volume,
        3.785411784,
    ),
    unit(
        "qt",
        &["qt", "quart", "quarts"],
        Dimension::Volume,
        0.946352946,
    ),
    unit(
        "pt",
        &["pt", "pint", "pints"],
        Dimension::Volume,
        0.473176473,
    ),
    unit("cup", &["cup", "cups"], Dimension::Volume, 0.2365882365),
    unit(
        "floz",
        &["floz", "fluidounce", "fluidounces"],
        Dimension::Volume,
        0.0295735295625,
    ),
];

/// The words that mean "convert this into". The `->` forms need no spaces around them, which is why they are matched separately.
const SPACED_KEYWORDS: &[&str] = &[" in ", " to ", " as ", " into "];
const SYMBOL_KEYWORDS: &[&str] = &["->", "→", "=>"];

/// A converted answer: the number and what to label it with.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Quantity {
    pub value: f64,
    pub unit: &'static str,
}

/// Converts `input` — `<expression> <unit> in <unit>` — or `None` when it is not that.
///
/// Every part has to hold: a valid expression, two known units, and one dimension between them. A query that merely contains the word "in" therefore costs a failed lookup and nothing else.
pub fn convert(input: &str) -> Option<Quantity> {
    let (left, right) = split_conversion(input)?;
    let (value, from) = split_quantity(left)?;
    let to = resolve(right.trim())?;
    if from.dimension != to.dimension {
        return None;
    }
    let base = value * from.scale + from.offset;
    let converted = (base - to.offset) / to.scale;
    converted.is_finite().then_some(Quantity {
        value: converted,
        unit: to.symbol,
    })
}

/// Splits `input` at its conversion keyword.
///
/// The *last* occurrence, which is what makes `12 in in cm` work: `in` is both a unit and the keyword, and a left-to-right split would take the inch for the preposition and leave "in cm" as the target.
fn split_conversion(input: &str) -> Option<(&str, &str)> {
    let lowered = input.to_ascii_lowercase();
    let mut best: Option<(usize, usize)> = None;
    for keyword in SPACED_KEYWORDS.iter().chain(SYMBOL_KEYWORDS) {
        if let Some(at) = lowered.rfind(keyword)
            && best.is_none_or(|(previous, _)| at > previous)
        {
            best = Some((at, keyword.len()));
        }
    }
    let (at, length) = best?;
    let left = input.get(..at)?.trim();
    let right = input.get(at + length..)?.trim();
    (!left.is_empty() && !right.is_empty()).then_some((left, right))
}

/// Splits `<expression><unit>` into the number it evaluates to and the unit it is in.
///
/// Tried longest-unit-first, so `90 min` is ninety minutes rather than ninety *inches* with a stray `m` — and the remainder has to evaluate, which is what keeps a word ending in a unit's name from reading as a quantity.
fn split_quantity(text: &str) -> Option<(f64, &'static Unit)> {
    let text = text.trim();
    for (at, _) in text.char_indices().skip(1) {
        let (left, right) = text.split_at(at);
        let Some(unit) = resolve(right.trim()) else {
            continue;
        };
        if let Some(value) = evaluate(left.trim()) {
            return Some((value, unit));
        }
    }
    None
}

/// The unit `name` spells, case- and plural-insensitively.
fn resolve(name: &str) -> Option<&'static Unit> {
    let name = name.trim().to_lowercase();
    if name.is_empty() {
        return None;
    }
    // Spaces inside a written-out unit ("square metre", "fluid ounce") are dropped rather than enumerated.
    let squashed: String = name.chars().filter(|c| !c.is_whitespace()).collect();
    let found = |name: &str| UNITS.iter().find(|unit| unit.names.contains(&name));
    if let Some(unit) = found(&squashed) {
        return Some(unit);
    }
    // A trailing plural is stripped rather than listed twice for every unit. Tried only after the exact match, so a unit whose own name ends in `s` — a second, an inch — is never mistaken for the plural of something else.
    squashed
        .strip_suffix('s')
        .filter(|singular| !singular.is_empty())
        .and_then(found)
}

#[cfg(test)]
mod tests {
    use super::super::format;
    use super::*;

    fn convert_to_string(input: &str) -> Option<String> {
        convert(input).map(|q| format!("{} {}", format(q.value), q.unit))
    }

    #[test]
    fn a_length_converts_both_ways_and_carries_its_unit() {
        assert_eq!(
            convert_to_string("3 km in mi").as_deref(),
            Some("1.8641135767 mi")
        );
        assert_eq!(
            convert_to_string("1 mi in km").as_deref(),
            Some("1.609344 km")
        );
        assert_eq!(convert_to_string("100 cm to m").as_deref(), Some("1 m"));
        assert_eq!(
            convert_to_string("2m in cm").as_deref(),
            Some("200 cm"),
            "no space needed"
        );
        assert_eq!(
            convert_to_string("6 feet in cm").as_deref(),
            Some("182.88 cm")
        );
    }

    /// `in` is both a unit and the keyword, so the split has to be the last one, not the first.
    #[test]
    fn inches_survive_being_spelled_like_the_keyword() {
        assert_eq!(
            convert_to_string("12 in in cm").as_deref(),
            Some("30.48 cm")
        );
        assert_eq!(convert_to_string("1 ft in in").as_deref(), Some("12 in"));
    }

    #[test]
    fn temperature_is_affine_not_a_ratio() {
        // The case a scale factor alone gets wrong: freezing water is not absolute zero.
        assert_eq!(convert_to_string("0 c in f").as_deref(), Some("32 °F"));
        assert_eq!(convert_to_string("100 c in f").as_deref(), Some("212 °F"));
        assert_eq!(convert_to_string("-40 c in f").as_deref(), Some("-40 °F"));
        assert_eq!(convert_to_string("32 f in c").as_deref(), Some("0 °C"));
        assert_eq!(convert_to_string("0 c in k").as_deref(), Some("273.15 K"));
        assert_eq!(
            convert_to_string("°C in °F").as_deref(),
            None,
            "a unit with no value is not a sum"
        );
    }

    #[test]
    fn the_decimal_and_binary_data_prefixes_are_different_questions() {
        assert_eq!(convert_to_string("1 gb in mb").as_deref(), Some("1000 MB"));
        assert_eq!(
            convert_to_string("1 gib in mib").as_deref(),
            Some("1024 MiB")
        );
        assert_eq!(
            convert_to_string("1 gib in gb").as_deref(),
            Some("1.073741824 GB")
        );
        assert_eq!(convert_to_string("8 bit in b").as_deref(), Some("1 B"));
    }

    #[test]
    fn the_left_side_is_a_whole_expression() {
        assert_eq!(convert_to_string("2*3 km in m").as_deref(), Some("6000 m"));
        assert_eq!(
            convert_to_string("(1+1) kg in g").as_deref(),
            Some("2000 g")
        );
        assert_eq!(
            convert_to_string("1_500 m in km").as_deref(),
            Some("1.5 km")
        );
    }

    #[test]
    fn a_conversion_between_two_different_things_is_refused() {
        // Refused rather than guessed: the launcher shows no answer and the app search still gets the query.
        assert!(convert("3 km in kg").is_none());
        assert!(convert("1 hour in litres").is_none());
        assert!(convert("5 c in mi").is_none());
    }

    #[test]
    fn a_sentence_that_merely_contains_a_keyword_is_not_a_conversion() {
        for query in [
            "photos in library",
            "log in",
            "settings",
            "5 in 10",
            "firefox to code",
            "in in in",
            "",
            "   in   ",
        ] {
            assert!(
                convert(query).is_none(),
                "'{query}' must fall through to the app search"
            );
        }
    }

    #[test]
    fn plurals_spellings_and_case_all_resolve() {
        assert_eq!(
            convert_to_string("1 Kilometre in Metres").as_deref(),
            Some("1000 m")
        );
        assert_eq!(convert_to_string("1 KM IN M").as_deref(), Some("1000 m"));
        assert_eq!(
            convert_to_string("2 square metres in cm2").as_deref(),
            Some("20000 cm²")
        );
        assert_eq!(convert_to_string("1 m3 in l").as_deref(), Some("1000 l"));
    }

    #[test]
    fn speed_angle_and_the_rest_of_the_dimensions_answer() {
        assert_eq!(
            convert_to_string("100 kmh in mph").as_deref(),
            Some("62.1371192237 mph")
        );
        assert_eq!(
            convert_to_string("180 deg in rad").as_deref(),
            Some("3.1415926536 rad")
        );
        assert_eq!(convert_to_string("1 h in min").as_deref(), Some("60 min"));
        assert_eq!(
            convert_to_string("1 stone in kg").as_deref(),
            Some("6.35029318 kg")
        );
        assert_eq!(
            convert_to_string("1 acre in m2").as_deref(),
            Some("4046.8564224 m²")
        );
    }

    /// Every name in the table has to be reachable, and no two units may claim the same spelling — the first would silently win and the second would be unreachable for ever.
    #[test]
    fn no_two_units_answer_to_the_same_name() {
        let mut seen: Vec<&str> = Vec::new();
        for unit in UNITS {
            for name in unit.names {
                assert!(
                    !seen.contains(name),
                    "'{name}' is claimed twice; the second unit ({}) would be unreachable",
                    unit.symbol
                );
                assert_eq!(
                    *name,
                    name.to_lowercase(),
                    "'{name}' is matched lowercase, so it must be written that way"
                );
                seen.push(name);
            }
            assert!(resolve(unit.names[0]).is_some(), "{} resolves", unit.symbol);
        }
    }
}
