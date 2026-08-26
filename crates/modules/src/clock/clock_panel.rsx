[logic]
use ::config::theme::{FontRole, NordTheme};
use ::services::clock;

fn now_time() -> String {
    chrono::Local::now().format("%H:%M:%S").to_string()
}
fn now_date() -> String {
    chrono::Local::now().format("%A, %d %B %Y").to_string()
}

let time = signal(now_time());
let date = signal(now_date());
let time_view = time.read_only();
let date_view = date.read_only();
let theme = use_theme::<NordTheme>();
let display = theme.font(FontRole::Display);
let body = theme.font(FontRole::Body);
// The shared ticker, so reopening this panel never stacks a second timer on the first.
platform_wayland::watch(clock::subscribe, move |t: clock::Now| {
    time.set(t.format("%H:%M:%S").to_string());
    date.set(t.format("%A, %d %B %Y").to_string());
});

[view]
col align:center gap(::ui::scale::space::md()) text_align:center
    text "{$time_view}" font_size:display color:theme.text
    text "{$date_view}" font_size:body color:theme.subtle
