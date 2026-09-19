[logic]
use crate::lockstatus::shown;
use crate::lockstatus::{IndicatorProps, indicator};
use ::config::theme::NordTheme;
use ::services::lockkeys::{self, LockKeys};

let host = ui::host::Host::current()?;
let config = *host.options::<::config::LockStatusConfig>();

let keys = signal(lockkeys::current().unwrap_or_else(lockkeys::read));
let listed = keys.read_only();
let tint = keys.read_only();

platform_wayland::watch(lockkeys::subscribe, move |state: LockKeys| keys.set(state));

// Which indicators exist is itself reactive under `hide_inactive`, so the row's children are a keyed list rather than a fixed pair — an indicator appearing or leaving never rebuilds the other one.
let indicators = memo(move || shown(listed.get(), config));

let fg = host.foreground;
let idle = use_theme::<NordTheme>().muted;
let size = host.icon_size();
let pad = host.inset();

[view]
row align:center
    for lock in $indicators key *lock
        indicator lock:lock keys:tint.clone() fg:fg idle:idle size:size pad:pad

[preview "Lockstatus" fixture:ui::preview::bar_chip]
lockstatus
