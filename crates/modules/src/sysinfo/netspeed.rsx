[logic]
use ::config::theme::{FontRole, NordTheme};
use ::services::netspeed::{self, NetSpeed, format_rate};

let initial = netspeed::current().unwrap_or_default();
let down = signal(format_rate(initial.down));
let up = signal(format_rate(initial.up));
let down_view = down.read_only();
let up_view = up.read_only();

platform_wayland::watch(netspeed::subscribe, move |speed: NetSpeed| {
    down.set(format_rate(speed.down));
    up.set(format_rate(speed.up));
});

let fg = ui::module::module_fg();
let fg_down = fg.clone();
let fg_up = fg.clone();
// Half-height arrows stacked in the chip: two rates need two lines to stay readable at bar size, and the direction glyph says which is which without a label.
let arrow_size = (ui::module::icon_px() * 0.55).round();

[view]
col justify:center gap(::ui::scale::space::xs())
    row align:center gap(::ui::scale::space::sm())
        icon_glyph name(|| "arrow-down".to_string()) tint(move || fg_down.get()) size:(arrow_size)
        text "{$down_view}" font_size:theme.font(FontRole::Caption) color:$fg
    row align:center gap(::ui::scale::space::sm())
        icon_glyph name(|| "arrow-up".to_string()) tint(move || fg_up.get()) size:(arrow_size)
        text "{$up_view}" font_size:theme.font(FontRole::Caption) color:$fg

[preview "Netspeed" fixture:ui::preview::bar_chip]
netspeed
