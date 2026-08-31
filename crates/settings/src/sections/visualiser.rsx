[logic]
use crate::form_section::{form_section, FormSectionProps};
use crate::text_row::{text_row, TextRowProps};
use crate::save_row::{save_row, SaveRowProps};
use crate::form::{parse_f32, parse_u32, persist, source};
use ::config::VisualiserConfig;

let (config, path) = source();
let v = config.visualiser;
let bars = signal(v.bars.to_string());
let smoothing = signal(v.smoothing.to_string());
let floor_db = signal(v.floor_db.to_string());
let gain = signal(v.gain.to_string());
let beat = signal(v.beat_sensitivity.to_string());
let frame_rate = signal(v.frame_rate.to_string());

let save: std::rc::Rc<dyn Fn()> = std::rc::Rc::new({
    let (bars, smoothing, floor_db) = (bars.clone(), smoothing.clone(), floor_db.clone());
    let (gain, beat, frame_rate) = (gain.clone(), beat.clone(), frame_rate.clone());
    move || {
        let value = VisualiserConfig {
            bars: parse_u32(&bars.peek(), v.bars),
            smoothing: parse_f32(&smoothing.peek(), v.smoothing),
            floor_db: parse_f32(&floor_db.peek(), v.floor_db),
            gain: parse_f32(&gain.peek(), v.gain),
            beat_sensitivity: parse_f32(&beat.peek(), v.beat_sensitivity),
            frame_rate: parse_u32(&frame_rate.peek(), v.frame_rate),
        };
        persist(&path, "visualiser", &value);
    }
});

[view]
form_section title:(Reactive::of(|| telar::t!("settings.section.visualiser")))
    text_row label:(Reactive::of(|| telar::t!("settings.field.visualiser_bars"))) value:$bars placeholder:"48"
    text_row label:(Reactive::of(|| telar::t!("settings.field.smoothing"))) value:$smoothing placeholder:"0.6"
    text_row label:(Reactive::of(|| telar::t!("settings.field.floor_db"))) value:$floor_db placeholder:"-60"
    text_row label:(Reactive::of(|| telar::t!("settings.field.gain"))) value:$gain placeholder:"1"
    text_row label:(Reactive::of(|| telar::t!("settings.field.beat_sensitivity"))) value:$beat placeholder:"1.35"
    text_row label:(Reactive::of(|| telar::t!("settings.field.frame_rate"))) value:$frame_rate placeholder:"60"
    save_row label:(Reactive::of(|| telar::t!("settings.save.visualiser"))) on_press:save
