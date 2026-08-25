[logic]
use crate::form::{parse_i32, persist, source};
use ::config::LockConfig;

let (config, path) = source();
let l = &config.lock;
// The keys not on the form — the library path, the biometric budgets, the weather and resource rows — are carried through unchanged, so saving here never quietly drops a setting the panel has no row for.
let base = l.clone();
let pam_service = signal(l.pam_service.clone());
let max_tries = signal(l.max_tries.to_string());
let lockout_seconds = signal(l.lockout_seconds.to_string());
let lock_before_sleep = signal(l.lock_before_sleep);
let fingerprint = signal(l.fingerprint);
let howdy_command = signal(l.howdy_command.clone());
let show_avatar = signal(l.show_avatar);
let show_media = signal(l.show_media);
let show_notifications = signal(l.show_notifications);
let hide_notifs = signal(l.hide_notifs);

let save: Box<dyn Fn()> = Box::new({
    let (pam_service, max_tries, lockout_seconds) = (
        pam_service.clone(),
        max_tries.clone(),
        lockout_seconds.clone(),
    );
    let (lock_before_sleep, fingerprint, howdy_command) = (
        lock_before_sleep.clone(),
        fingerprint.clone(),
        howdy_command.clone(),
    );
    let (show_avatar, show_media, show_notifications, hide_notifs) = (
        show_avatar.clone(),
        show_media.clone(),
        show_notifications.clone(),
        hide_notifs.clone(),
    );
    move || {
        let value = LockConfig {
            pam_service: pam_service.peek().trim().to_string(),
            max_tries: parse_i32(&max_tries.peek(), base.max_tries as i32).max(0) as u32,
            lockout_seconds: parse_i32(&lockout_seconds.peek(), base.lockout_seconds as i32)
                .max(0) as u64,
            lock_before_sleep: lock_before_sleep.peek(),
            fingerprint: fingerprint.peek(),
            howdy_command: howdy_command.peek().trim().to_string(),
            show_avatar: show_avatar.peek(),
            show_media: show_media.peek(),
            show_notifications: show_notifications.peek(),
            hide_notifs: hide_notifs.peek(),
            ..base.clone()
        };
        persist(&path, "lock", &value);
    }
});

[view]
form_section title(|| telar::t!("settings.section.lock"))
    text_row label(|| telar::t!("settings.field.pam_service")) value:$pam_service placeholder:"login"
    text_row label(|| telar::t!("settings.field.max_tries")) value:$max_tries placeholder:"5"
    text_row label(|| telar::t!("settings.field.lockout_seconds")) value:$lockout_seconds placeholder:"30"
    toggle_row label(|| telar::t!("settings.field.lock_before_sleep")) value:$lock_before_sleep
    toggle_row label(|| telar::t!("settings.field.fingerprint")) value:$fingerprint
    text_row label(|| telar::t!("settings.field.howdy_command")) value:$howdy_command placeholder:"howdy compare"
    toggle_row label(|| telar::t!("settings.field.show_avatar")) value:$show_avatar
    toggle_row label(|| telar::t!("settings.field.show_media")) value:$show_media
    toggle_row label(|| telar::t!("settings.field.show_notifications")) value:$show_notifications
    toggle_row label(|| telar::t!("settings.field.hide_notifs")) value:$hide_notifs
    save_row label(|| telar::t!("settings.save.lock")) on_press(save)
