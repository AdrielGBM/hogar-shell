[logic]
use crate::form::{parse_u64, persist, source};
use crate::form_section::{FormSectionProps, form_section};
use crate::save_row::{SaveRowProps, save_row};
use ::ui::form::text_row::{TextRowProps, text_row};
use ::config::AutomationConfig;

let (config, path) = source();
let a = config.automation;
let timeout_seconds = signal(a.timeout_seconds.to_string());
let max_line_kib = signal(a.max_line_kib.to_string());
let max_run_kib = signal(a.max_run_kib.to_string());
let min_interval_seconds = signal(a.min_interval_seconds.to_string());
let backoff_seconds = signal(a.backoff_seconds.to_string());
let max_backoff_seconds = signal(a.max_backoff_seconds.to_string());
let shutdown_grace_seconds = signal(a.shutdown_grace_seconds.to_string());

let save: std::rc::Rc<dyn Fn()> = std::rc::Rc::new({
    let (timeout_seconds, max_line_kib, max_run_kib) = (
        timeout_seconds.clone(),
        max_line_kib.clone(),
        max_run_kib.clone(),
    );
    let (min_interval_seconds, backoff_seconds, max_backoff_seconds) = (
        min_interval_seconds.clone(),
        backoff_seconds.clone(),
        max_backoff_seconds.clone(),
    );
    let shutdown_grace_seconds = shutdown_grace_seconds.clone();
    move || {
        let value = AutomationConfig {
            timeout_seconds: parse_u64(&timeout_seconds.peek(), a.timeout_seconds),
            max_line_kib: parse_u64(&max_line_kib.peek(), a.max_line_kib),
            max_run_kib: parse_u64(&max_run_kib.peek(), a.max_run_kib),
            min_interval_seconds: parse_u64(&min_interval_seconds.peek(), a.min_interval_seconds),
            backoff_seconds: parse_u64(&backoff_seconds.peek(), a.backoff_seconds),
            max_backoff_seconds: parse_u64(&max_backoff_seconds.peek(), a.max_backoff_seconds),
            shutdown_grace_seconds: parse_u64(&shutdown_grace_seconds.peek(), a.shutdown_grace_seconds),
        };
        persist(&path, "automation", &value);
    }
});

[view]
form_section title:(Reactive::of(|| telar::t!("settings.section.automation")))
    text_row label:(Reactive::of(|| telar::t!("settings.field.source_timeout"))) value:$timeout_seconds placeholder:"5"
    text_row label:(Reactive::of(|| telar::t!("settings.field.source_max_line"))) value:$max_line_kib placeholder:"64"
    text_row label:(Reactive::of(|| telar::t!("settings.field.source_max_run"))) value:$max_run_kib placeholder:"1024"
    text_row label:(Reactive::of(|| telar::t!("settings.field.source_min_interval"))) value:$min_interval_seconds placeholder:"1"
    text_row label:(Reactive::of(|| telar::t!("settings.field.source_backoff"))) value:$backoff_seconds placeholder:"2"
    text_row label:(Reactive::of(|| telar::t!("settings.field.source_max_backoff"))) value:$max_backoff_seconds placeholder:"300"
    text_row label:(Reactive::of(|| telar::t!("settings.field.shutdown_grace"))) value:$shutdown_grace_seconds placeholder:"3"
    save_row label:(Reactive::of(|| telar::t!("settings.save.automation"))) on_press:save
