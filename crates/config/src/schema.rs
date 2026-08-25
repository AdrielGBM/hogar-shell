//! The annotated default config, generated rather than written.
//!
//! `hogar-shell config schema` prints every section with its defaults and the doc comment that explains it, so the "every option" reference is a view of the code instead of a second copy that drifts away from it. The comments come from `build.rs`, which lifts them off `config.rs`; the values come from serializing [`Config::starter`], so a key that exists is a key the shell reads.
//!
//! [`outline`] is that view as data, and [`render`] is one printing of it. `hogar-shell man 5` is the other: both walk the same tree, so a key cannot reach the TOML reference and go missing from the manual.

use std::collections::HashMap;
use std::fmt::Write;

use crate::Config;

include!(concat!(env!("OUT_DIR"), "/config_docs.rs"));

/// Which config struct backs each top-level section, so a section's keys can be annotated from the right struct. Kept honest by `every_section_maps_to_a_documented_struct`, which fails if a section is added to `Config` without an entry here.
fn section_structs() -> HashMap<&'static str, &'static str> {
    [
        ("general", "GeneralConfig"),
        ("bars", "BarsConfig"),
        ("theme", "ThemeConfig"),
        ("shape", "ShapeConfig"),
        ("corners", "CornersConfig"),
        ("panels", "PanelsConfig"),
        ("popouts", "PopoutsConfig"),
        ("icons", "IconsConfig"),
        ("stack", "StackConfig"),
        ("notifications", "NotificationsConfig"),
        ("toasts", "ToastsConfig"),
        ("screenshot", "ScreenshotConfig"),
        ("recorder", "RecorderConfig"),
        ("utilities", "UtilitiesConfig"),
        ("sidebar", "SidebarConfig"),
        ("background", "BackgroundConfig"),
        ("wallpaper", "WallpaperConfig"),
        ("widgets", "WidgetsConfig"),
        ("active_window", "ActiveWindowConfig"),
        ("clock", "ClockConfig"),
        ("media", "MediaConfig"),
        ("lyrics", "LyricsConfig"),
        ("workspaces", "WorkspacesConfig"),
        ("launcher", "LauncherConfig"),
        ("audio", "AudioConfig"),
        ("visualiser", "VisualiserConfig"),
        ("brightness", "BrightnessConfig"),
        ("temperature", "TemperatureConfig"),
        ("battery", "BatteryConfig"),
        ("lock_status", "LockStatusConfig"),
        ("lock", "LockConfig"),
        ("idle", "IdleConfig"),
        ("status_icons", "StatusIconsConfig"),
        ("network", "NetworkConfig"),
        ("bluetooth", "BluetoothConfig"),
        ("gpu", "GpuConfig"),
        ("weather", "WeatherConfig"),
        ("dashboard", "DashboardConfig"),
        ("paths", "PathsConfig"),
        ("tray", "TrayConfig"),
        ("animation", "AnimationConfig"),
        ("keynav", "KeyNavConfig"),
    ]
    .into_iter()
    .collect()
}

fn doc_for(structure: &str, field: &str) -> Option<&'static str> {
    CONFIG_DOCS
        .iter()
        .find(|(s, f, _)| *s == structure && *f == field)
        .map(|(_, _, doc)| *doc)
}

/// Every documented key, as `# ` comment lines wrapped to nothing — the source comments are already wrapped, so they are emitted verbatim rather than reflowed into a width this shell would have to guess.
fn comment(doc: &str) -> String {
    doc.lines()
        .map(|line| {
            if line.is_empty() {
                "#\n".to_string()
            } else {
                format!("# {line}\n")
            }
        })
        .collect()
}

/// One table in the reference: a `[section]`, a nested `[section.table]`, and the keys it holds.
pub struct Table {
    pub path: String,
    pub doc: Option<&'static str>,
    pub entries: Vec<Entry>,
}

/// What a table holds, in the order any rendering has to emit it — TOML puts every bare key before the first sub-table header, since a header printed first would swallow the keys that follow it.
pub enum Entry {
    /// A key and the value the shell uses when it is absent. `default` is `None` for an `Option` field: there is no value to print, and inventing one would document a default the shell does not use.
    Key {
        name: String,
        default: Option<toml::Value>,
        doc: Option<&'static str>,
    },
    Table(Table),
    /// A list of tables — `[[idle.stages]]`, `[[battery.warn_levels]]` — carrying the entries a fresh install starts with. Its shape is those entries; there is no struct behind the element type to annotate.
    List {
        path: String,
        doc: Option<&'static str>,
        elements: Vec<toml::Value>,
    },
}

/// Every section of the reference, or one of them, as data rather than as text. An unknown section name is an error listing the real ones, so a typo answers with the menu rather than with nothing.
pub fn outline(section: Option<&str>) -> Result<Vec<Table>, String> {
    let structs = section_structs();
    let defaults = toml::Value::try_from(Config::starter())
        .map_err(|e| format!("serializing the default config: {e}"))?;
    let table = defaults
        .as_table()
        .ok_or_else(|| "the default config is not a table".to_string())?;

    if let Some(name) = section
        && !structs.contains_key(name)
    {
        let mut known: Vec<&str> = structs.keys().copied().collect();
        known.sort_unstable();
        return Err(format!(
            "unknown section '{name}'; known sections: {}",
            known.join(", ")
        ));
    }

    let mut sections = Vec::new();
    for (name, structure) in ordered_sections(&structs) {
        if section.is_some_and(|wanted| wanted != name) {
            continue;
        }
        let Some(value) = table.get(name).and_then(toml::Value::as_table) else {
            continue;
        };
        sections.push(Table {
            path: name.to_string(),
            doc: doc_for(structure, ""),
            entries: walk(name, value, structure),
        });
    }
    Ok(sections)
}

/// One table's entries: its own keys, then the optional ones serde left out, then its sub-tables, then its lists of tables.
fn walk(path: &str, table: &toml::map::Map<String, toml::Value>, structure: &str) -> Vec<Entry> {
    let mut keys = Vec::new();
    let mut tables = Vec::new();
    let mut lists = Vec::new();
    for (key, entry) in table {
        let child = format!("{path}.{key}");
        if let Some(inner) = entry.as_table() {
            // A map-valued key (`[theme.colors]`, `[background.monitors]`) has no struct and no fixed keys, so it contributes a header and nothing else — still the one thing a reader cannot learn elsewhere: that the table exists and what to call it.
            let entries = match type_of(structure, key) {
                Some(nested) => walk(&child, inner, nested),
                None => Vec::new(),
            };
            tables.push(Entry::Table(Table {
                path: child,
                doc: doc_for(structure, key),
                entries,
            }));
            continue;
        }
        if let Some(elements) = table_array(entry) {
            // A list's explanation is usually on its element struct rather than on the field holding it: what an `[[idle.stages]]` table means is what an `IdleStage` is.
            let doc = doc_for(structure, key)
                .or_else(|| type_of(structure, key).and_then(|element| doc_for(element, "")));
            lists.push(Entry::List {
                path: child,
                doc,
                elements: elements.to_vec(),
            });
            continue;
        }
        keys.push(Entry::Key {
            name: key.clone(),
            default: Some(narrow_float(entry)),
            doc: doc_for(structure, key),
        });
    }
    keys.extend(unset(table, structure));
    keys.into_iter().chain(tables).chain(lists).collect()
}

/// The annotated default config, or one section of it: the outline printed as the TOML a user edits.
pub fn render(section: Option<&str>) -> Result<String, String> {
    let sections = outline(section)?;
    let mut out = String::new();
    if section.is_none() {
        out.push_str("# hogar-shell configuration reference\n");
        out.push_str(
            "#\n# Generated by `hogar-shell config schema` from this build's own defaults.\n",
        );
        out.push_str("# Every key below is the value the shell uses when the key is absent.\n\n");
    }
    for table in &sections {
        if let Some(doc) = table.doc {
            out.push_str(&comment(doc));
        }
        let _ = writeln!(out, "[{}]", table.path);
        out.push_str(&render_entries(&table.entries));
        out.push('\n');
    }
    Ok(out)
}

/// A table's entries as TOML. A list of tables has to carry its section in the header — serializing it as a bare one-key map yields a reference whose own text does not parse back into the section it documents.
fn render_entries(entries: &[Entry]) -> String {
    let mut out = String::new();
    for entry in entries {
        match entry {
            Entry::Key { name, default, doc } => {
                if let Some(doc) = doc {
                    out.push_str(&comment(doc));
                }
                match default {
                    Some(value) => out.push_str(
                        &toml::to_string(&toml::map::Map::from_iter([(
                            name.clone(),
                            value.clone(),
                        )]))
                        .unwrap_or_default(),
                    ),
                    None => {
                        let _ = writeln!(out, "# {name} =   # unset by default");
                    }
                }
            }
            Entry::Table(table) => {
                out.push('\n');
                if let Some(doc) = table.doc {
                    out.push_str(&comment(doc));
                }
                let _ = writeln!(out, "[{}]", table.path);
                out.push_str(&render_entries(&table.entries));
            }
            Entry::List {
                path,
                doc,
                elements,
            } => {
                out.push('\n');
                if let Some(doc) = doc {
                    out.push_str(&comment(doc));
                }
                for (index, element) in elements.iter().enumerate() {
                    if index > 0 {
                        out.push('\n');
                    }
                    let _ = writeln!(out, "[[{path}]]");
                    out.push_str(&toml::to_string(element).unwrap_or_default());
                }
            }
        }
    }
    out
}

/// The struct backing `structure::field`, when the field is a plain nested config struct.
fn type_of(structure: &str, field: &str) -> Option<&'static str> {
    CONFIG_FIELD_TYPES
        .iter()
        .find(|(owner, name, _)| *owner == structure && *name == field)
        .map(|(_, _, kind)| *kind)
}

/// The struct backing a section path — `background` or `widgets.clock` — by walking the field types.
fn struct_for(path: &str) -> Option<&'static str> {
    let mut parts = path.split('.');
    let mut current = *section_structs().get(parts.next()?)?;
    for part in parts {
        current = type_of(current, part)?;
    }
    Some(current)
}

/// Whether the config section at `path` has a key — or a key's explanation — containing `needle`, which the caller has already lowercased.
///
/// This is what the settings search matches against. Every form's fields are keys on a struct and every key's prose is already lifted off the source for the reference, so a search built on it finds a setting by the words that *explain* it without any form having to register its rows a second time — and cannot go stale when a form gains one.
pub fn section_mentions(path: &str, needle: &str) -> bool {
    let Some(structure) = struct_for(path) else {
        return false;
    };
    CONFIG_DOCS.iter().any(|(owner, field, doc)| {
        *owner == structure
            && (field.to_lowercase().contains(needle) || doc.to_lowercase().contains(needle))
    })
}

/// An `f32` config value printed the way it was written rather than the way it widens.
///
/// Serde has one float type and it is `f64`, so `0.35f32` reaches the serializer as `0.3499999940395355` and the generated reference documents a default nobody typed. The test for "this came from an `f32`" is that narrowing round-trips exactly — true for every value an `f32` field can hold and false for a `f64` carrying more precision than one, which is left alone.
fn narrow_float(entry: &toml::Value) -> toml::Value {
    match entry.as_float() {
        Some(value) if (value as f32) as f64 == value => {
            toml::Value::Float((value as f32).to_string().parse().unwrap_or(value))
        }
        _ => entry.clone(),
    }
}

/// `entry` as a non-empty array of tables, which is the one shape that needs its own header.
fn table_array(entry: &toml::Value) -> Option<&[toml::Value]> {
    let array = entry.as_array()?;
    (!array.is_empty() && array.iter().all(toml::Value::is_table)).then_some(array.as_slice())
}

/// The keys serde left out because they default to `None`.
///
/// An `Option` with no value serializes to nothing, so a reference built from the defaults alone would silently omit every optional key — `[clock] format` is a documented option a reader would never learn exists.
///
/// Walked over every field rather than over the documented ones: `[corners] top_left` and `[background] image` carry no doc comment and are still keys, and a reference that lists a key only when somebody remembered to explain it is one where forgetting a comment deletes the key.
fn unset(table: &toml::map::Map<String, toml::Value>, structure: &str) -> Vec<Entry> {
    CONFIG_FIELDS
        .iter()
        .filter(|(owner, field)| *owner == structure && !table.contains_key(*field))
        .map(|(_, field)| Entry::Key {
            name: (*field).to_string(),
            default: None,
            doc: doc_for(structure, field),
        })
        .collect()
}

fn ordered_sections(
    structs: &HashMap<&'static str, &'static str>,
) -> Vec<(&'static str, &'static str)> {
    let mut sections: Vec<(&'static str, &'static str)> =
        structs.iter().map(|(k, v)| (*k, *v)).collect();
    sections.sort_unstable_by_key(|(name, _)| *name);
    sections
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_section_on_config_has_a_struct_in_the_map() {
        // The one way this file goes stale: a section added to `Config` and never mapped here, which would silently drop it from the reference the README points at.
        let defaults = toml::Value::try_from(Config::starter()).unwrap();
        let table = defaults.as_table().unwrap();
        let structs = section_structs();
        let missing: Vec<&str> = table
            .keys()
            .map(String::as_str)
            .filter(|key| *key != "modules")
            .filter(|key| !structs.contains_key(key))
            .collect();
        assert!(
            missing.is_empty(),
            "sections with no struct mapped: {missing:?}"
        );
    }

    #[test]
    fn the_schema_carries_the_doc_comments_off_the_source() {
        let text = render(None).expect("renders");
        assert!(text.contains("[notifications]"));
        assert!(text.contains("[animation]"));
        assert!(
            text.contains("# "),
            "the schema is annotated, not a bare dump of defaults"
        );
        // A value, not a placeholder: the defaults come from the code, so they cannot disagree with it.
        assert!(text.contains("max_visible = 4"), "{text}");
        // And it round-trips: what the reference prints is a config the shell would accept.
        let parsed: Config = toml::from_str(&text).expect("the printed schema parses");
        assert_eq!(parsed.stack.max_visible, 4);
    }

    #[test]
    fn an_f32_default_is_printed_as_it_was_written() {
        // Serde has one float type, so every `f32` default reaches the printer widened: `0.35` came out as `0.3499999940395355`, and the reference documented a number nobody typed and no one would copy.
        let text = render(None).expect("renders");
        assert!(text.contains("background_opacity = 0.35"), "{text}");
        assert!(text.contains("beat_sensitivity = 1.35"), "{text}");
        assert!(
            !text.contains("0.3499999"),
            "a widened f32 is still being printed: {text}"
        );
        // And the shorter spelling still parses back to the same value the code holds.
        let parsed: Config = toml::from_str(&text).expect("the printed schema parses");
        assert_eq!(
            parsed.widgets.clock.background_opacity,
            Config::starter().widgets.clock.background_opacity
        );
    }

    #[test]
    fn a_list_of_tables_is_printed_under_the_section_that_owns_it() {
        // Round-tripping is not enough on its own to catch this: a bare `[[stages]]` parses as an unknown top-level key, which serde drops silently — so the reference read as valid while documenting a section the shell would never see.
        let text = render(Some("idle")).expect("renders");
        assert!(text.contains("[[idle.stages]]"), "{text}");
        assert!(!text.contains("\n[[stages]]"), "{text}");

        let parsed: Config = toml::from_str(&text).expect("the printed section parses");
        let printed = Config::starter().idle.stages;
        assert_eq!(parsed.idle.stages.len(), printed.len());
        assert_eq!(parsed.idle.stages[0].action, printed[0].action);

        // The same shape one section over, so the fix is not one special case.
        let battery = render(Some("battery")).expect("renders");
        assert!(battery.contains("[[battery.warn_levels]]"), "{battery}");
    }

    #[test]
    fn one_section_can_be_asked_for_and_a_typo_lists_the_rest() {
        let one = render(Some("clock")).expect("renders");
        assert!(one.contains("[clock]"));
        assert!(
            !one.contains("[notifications]"),
            "only the section asked for"
        );

        let err = render(Some("clok")).expect_err("a typo is an error");
        assert!(err.contains("unknown section 'clok'"), "{err}");
        assert!(err.contains("clock"), "and it lists the real ones: {err}");
    }
}
