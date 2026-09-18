//! What a run measured in-process, one line per fact, and the reader the report uses to take it back.
//!
//! Every line starts with the wall-clock time it describes, in microseconds since the Unix epoch, because the other half of the evidence — the protocol log — is stamped by libwayland from the same `CLOCK_REALTIME`, and lining the two up is how a commit is attributed to the scenario and the event that caused it. The rest of the line is tab-separated: a kind, then the kind's fields. Free text has its tabs and newlines flattened so a line is always one record.

use std::fs::File;
use std::io::{LineWriter, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

/// A rect in a surface's logical coordinates, as the layout placed it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LogicalRect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl LogicalRect {
    pub fn new(x: f64, y: f64, w: f64, h: f64) -> Self {
        Self { x, y, w, h }
    }

    pub fn contains(&self, x: f64, y: f64) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Record {
    /// A fact about the run as a whole: its mode, backend, output, versions.
    Meta { key: String, value: String },
    /// A scenario beginning or ending.
    Scenario { name: String, start: bool },
    /// One discrete change the report checks the damage of: what changed, on which surface, and the rects the layout said it should repaint.
    Event {
        scenario: String,
        index: u32,
        role: String,
        expected: Vec<LogicalRect>,
    },
    /// A `TELAR_PERF` window summary, verbatim.
    Perf { summary: String },
    /// Memory figures from `/proc/self/status`, in kB.
    Rss {
        label: String,
        fields: Vec<(String, u64)>,
    },
    /// CPU time consumed so far: this process (all threads), and the compositor when it could be read.
    Cpu {
        label: String,
        process_us: u64,
        compositor_us: Option<u64>,
    },
    /// A pointer press as a surface's tree received it, in that surface's logical coordinates.
    Press { role: String, x: f64, y: f64 },
    /// A click target: where the user is asked to click, and which answer is expected there.
    Target {
        class: String,
        id: String,
        rect: LogicalRect,
    },
    /// A log event telar or the platform emitted at info level or above.
    Log {
        level: String,
        target: String,
        message: String,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub at_us: u64,
    pub record: Record,
}

pub fn now_us() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_micros() as u64)
        .unwrap_or(0)
}

fn flat(text: &str) -> String {
    text.replace(['\t', '\n', '\r'], " ")
}

fn rects_text(rects: &[LogicalRect]) -> String {
    rects
        .iter()
        .map(|r| format!("{:.2},{:.2},{:.2},{:.2}", r.x, r.y, r.w, r.h))
        .collect::<Vec<_>>()
        .join(";")
}

fn parse_rects(text: &str) -> Option<Vec<LogicalRect>> {
    if text.is_empty() {
        return Some(Vec::new());
    }
    text.split(';').map(parse_rect).collect()
}

fn parse_rect(text: &str) -> Option<LogicalRect> {
    let parts: Vec<f64> = text
        .split(',')
        .map(|p| p.parse().ok())
        .collect::<Option<_>>()?;
    let [x, y, w, h] = parts[..] else {
        return None;
    };
    Some(LogicalRect::new(x, y, w, h))
}

impl Record {
    fn to_line(&self) -> String {
        match self {
            Record::Meta { key, value } => format!("meta\t{}\t{}", flat(key), flat(value)),
            Record::Scenario { name, start } => {
                format!(
                    "scenario\t{}\t{}",
                    flat(name),
                    if *start { "start" } else { "end" }
                )
            }
            Record::Event {
                scenario,
                index,
                role,
                expected,
            } => format!(
                "event\t{}\t{index}\t{}\t{}",
                flat(scenario),
                flat(role),
                rects_text(expected)
            ),
            Record::Perf { summary } => format!("perf\t{}", flat(summary)),
            Record::Rss { label, fields } => {
                let fields: Vec<String> = fields.iter().map(|(k, v)| format!("{k}={v}")).collect();
                format!("rss\t{}\t{}", flat(label), fields.join("\t"))
            }
            Record::Cpu {
                label,
                process_us,
                compositor_us,
            } => format!(
                "cpu\t{}\t{process_us}\t{}",
                flat(label),
                compositor_us.map_or("-".to_owned(), |v| v.to_string())
            ),
            Record::Press { role, x, y } => format!("press\t{}\t{x:.2}\t{y:.2}", flat(role)),
            Record::Target { class, id, rect } => {
                format!(
                    "target\t{}\t{}\t{}",
                    flat(class),
                    flat(id),
                    rects_text(std::slice::from_ref(rect))
                )
            }
            Record::Log {
                level,
                target,
                message,
            } => {
                format!("log\t{}\t{}\t{}", flat(level), flat(target), flat(message))
            }
        }
    }

    fn from_fields(fields: &[&str]) -> Option<Record> {
        let text = |i: usize| fields.get(i).map(|s| s.to_string());
        Some(match *fields.first()? {
            "meta" => Record::Meta {
                key: text(1)?,
                value: text(2).unwrap_or_default(),
            },
            "scenario" => Record::Scenario {
                name: text(1)?,
                start: *fields.get(2)? == "start",
            },
            "event" => Record::Event {
                scenario: text(1)?,
                index: fields.get(2)?.parse().ok()?,
                role: text(3)?,
                expected: parse_rects(fields.get(4).copied().unwrap_or(""))?,
            },
            "perf" => Record::Perf { summary: text(1)? },
            "rss" => Record::Rss {
                label: text(1)?,
                fields: fields[2..]
                    .iter()
                    .filter_map(|f| {
                        let (k, v) = f.split_once('=')?;
                        Some((k.to_owned(), v.parse().ok()?))
                    })
                    .collect(),
            },
            "cpu" => Record::Cpu {
                label: text(1)?,
                process_us: fields.get(2)?.parse().ok()?,
                compositor_us: fields.get(3).and_then(|v| v.parse().ok()),
            },
            "press" => Record::Press {
                role: text(1)?,
                x: fields.get(2)?.parse().ok()?,
                y: fields.get(3)?.parse().ok()?,
            },
            "target" => Record::Target {
                class: text(1)?,
                id: text(2)?,
                rect: parse_rect(fields.get(3)?)?,
            },
            "log" => Record::Log {
                level: text(1)?,
                target: text(2)?,
                message: text(3).unwrap_or_default(),
            },
            _ => return None,
        })
    }
}

/// Reads a timeline back, skipping lines that do not parse — a run killed mid-write leaves at most one.
pub fn read(text: &str) -> Vec<Entry> {
    text.lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line.split('\t').collect();
            let at_us = fields.first()?.parse().ok()?;
            let record = Record::from_fields(&fields[1..])?;
            Some(Entry { at_us, record })
        })
        .collect()
}

/// The writing end, shared by the director on the driver thread and the tracing layer on whichever thread logs.
///
/// Line-buffered so a run the user interrupts keeps everything up to its last complete line.
#[derive(Clone)]
pub struct Recorder {
    out: Arc<Mutex<LineWriter<File>>>,
}

impl Recorder {
    pub fn create(path: &Path) -> std::io::Result<Self> {
        Ok(Self {
            out: Arc::new(Mutex::new(LineWriter::new(File::create(path)?))),
        })
    }

    pub fn record(&self, record: Record) {
        self.record_at(now_us(), record);
    }

    /// Records a fact about a moment that has already passed — an event whose expected rects are only known once the layout has run.
    pub fn record_at(&self, at_us: u64, record: Record) {
        if let Ok(mut out) = self.out.lock() {
            let _ = writeln!(out, "{at_us}\t{}", record.to_line());
        }
    }

    pub fn meta(&self, key: &str, value: impl std::fmt::Display) {
        self.record(Record::Meta {
            key: key.to_owned(),
            value: value.to_string(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(record: Record) {
        let line = format!("1789733135000000\t{}", record.to_line());
        let back = read(&line);
        assert_eq!(
            back,
            vec![Entry {
                at_us: 1_789_733_135_000_000,
                record
            }]
        );
    }

    #[test]
    fn every_record_survives_a_round_trip() {
        round_trip(Record::Meta {
            key: "mode".into(),
            value: "merged".into(),
        });
        round_trip(Record::Scenario {
            name: "clock".into(),
            start: true,
        });
        round_trip(Record::Event {
            scenario: "notify".into(),
            index: 3,
            role: "top".into(),
            expected: vec![
                LogicalRect::new(1532.0, 44.0, 380.0, 64.5),
                LogicalRect::new(1532.0, 116.5, 380.0, 64.5),
            ],
        });
        round_trip(Record::Event {
            scenario: "clock".into(),
            index: 0,
            role: "top".into(),
            expected: Vec::new(),
        });
        round_trip(Record::Perf {
            summary: "build=41/95us(n60) damage=59".into(),
        });
        round_trip(Record::Rss {
            label: "steady".into(),
            fields: vec![("VmRSS".into(), 51234), ("VmHWM".into(), 60000)],
        });
        round_trip(Record::Cpu {
            label: "clock:end".into(),
            process_us: 1_250_000,
            compositor_us: None,
        });
        round_trip(Record::Press {
            role: "catcher".into(),
            x: 812.5,
            y: 486.0,
        });
        round_trip(Record::Target {
            class: "bar".into(),
            id: "B1".into(),
            rect: LogicalRect::new(700.0, 4.0, 96.0, 28.0),
        });
    }

    #[test]
    fn free_text_cannot_split_a_record() {
        let record = Record::Log {
            level: "WARN".into(),
            target: "telar".into(),
            message: "HW renderer unavailable\t(no adapter)\nfalling back".into(),
        };
        let back = read(&format!("1\t{}", record.to_line()));
        assert_eq!(back.len(), 1);
        let Record::Log { message, .. } = &back[0].record else {
            panic!("expected a log record");
        };
        assert_eq!(message, "HW renderer unavailable (no adapter) falling back");
    }

    #[test]
    fn a_torn_last_line_is_dropped() {
        let text = "1\tscenario\tidle\tstart\n2\tcpu\tidle:en";
        let back = read(text);
        assert_eq!(back.len(), 1);
    }
}
