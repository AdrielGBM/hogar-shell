[logic]
use ::ui::chip_label::{chip_label, ChipLabelProps};
// No-op under a headless test (the clock shows its initial value there).
use ::config::ClockConfig;
use ::config::theme::{FontRole, NordTheme};
use ::services::clock;

// `strftime` patterns come from config, so a user can have seconds, a weekday or a 12-hour clock without the shell enumerating presets.
fn render(now: &chrono::DateTime<chrono::Local>, config: &ClockConfig) -> String {
    let time = now.format(config.time_format()).to_string();
    if config.show_date {
        format!("{} · {}", now.format(&config.date_format), time)
    } else {
        time
    }
}

let host = ui::host::Host::current()?;
let config = host.options::<::config::ClockConfig>().clone();
let for_tick = config.clone();

let now = signal(render(&chrono::Local::now(), &config));
let now_view = now.read_only();
// module_shell provides the box, hover/press feedback and drawer-opening click; this module supplies only content, painted with the container-chosen foreground.
let fg = host.foreground;
// One ticker for the whole shell, aligned to the second boundary; every clock surface reads the same broadcast.
platform_wayland::watch(clock::subscribe, move |t: clock::Now| {
    now.set(render(&t, &for_tick));
});

[view]
chip_label text:$now_view

[preview "Clock" fixture:ui::preview::bar_chip]
clock
