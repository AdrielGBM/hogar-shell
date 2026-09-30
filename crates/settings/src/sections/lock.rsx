[logic]
use ::ui::form::enum_row::{EnumRowProps, enum_row};
use crate::form::{
    MEDIA_DETAILS, NOTIFICATION_DETAILS, media_detail_str, notification_detail_str, parse_i32,
    parse_media_detail, parse_notification_detail, persist, source,
};
use crate::form_section::{FormSectionProps, form_section};
use crate::save_row::{SaveRowProps, save_row};
use ::ui::form::text_row::{TextRowProps, text_row};
use ::ui::form::toggle_row::{ToggleRowProps, toggle_row};
use ::config::LockConfig;

let (config, path) = source();
let l = &config.lock;
// The keys not on the form — the library path and the biometric budgets — are carried through unchanged, so saving here never quietly drops a setting the panel has no row for. What the lock screen *shows* is not here at all any more: that is the layout's `lock` layer, and these two rows are only how much each reading there may reveal.
let base = l.clone();
let pam_service = signal(l.pam_service.clone());
let max_tries = signal(l.max_tries.to_string());
let lockout_seconds = signal(l.lockout_seconds.to_string());
let lock_before_sleep = signal(l.lock_before_sleep);
let fingerprint = signal(l.fingerprint);
let howdy_command = signal(l.howdy_command.clone());
let notification_detail = signal(notification_detail_str(l.notification_detail).to_string());
let media_detail = signal(media_detail_str(l.media_detail).to_string());

let save: std::rc::Rc<dyn Fn()> = std::rc::Rc::new({
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
    let (notification_detail, media_detail) =
        (notification_detail.clone(), media_detail.clone());
    move || {
        let value = LockConfig {
            pam_service: pam_service.peek().trim().to_string(),
            max_tries: parse_i32(&max_tries.peek(), base.max_tries as i32).max(0) as u32,
            lockout_seconds: parse_i32(&lockout_seconds.peek(), base.lockout_seconds as i32)
                .max(0) as u64,
            lock_before_sleep: lock_before_sleep.peek(),
            fingerprint: fingerprint.peek(),
            howdy_command: howdy_command.peek().trim().to_string(),
            notification_detail: parse_notification_detail(&notification_detail.peek()),
            media_detail: parse_media_detail(&media_detail.peek()),
            ..base.clone()
        };
        persist(&path, "lock", &value);
    }
});

[view]
form_section title:(Reactive::of(|| telar::t!("settings.section.lock")))
    text_row label:(Reactive::of(|| telar::t!("settings.field.pam_service"))) value:$pam_service placeholder:"login"
    text_row label:(Reactive::of(|| telar::t!("settings.field.max_tries"))) value:$max_tries placeholder:"5"
    text_row label:(Reactive::of(|| telar::t!("settings.field.lockout_seconds"))) value:$lockout_seconds placeholder:"30"
    toggle_row label:(Reactive::of(|| telar::t!("settings.field.lock_before_sleep"))) value:$lock_before_sleep
    toggle_row label:(Reactive::of(|| telar::t!("settings.field.fingerprint"))) value:$fingerprint
    text_row label:(Reactive::of(|| telar::t!("settings.field.howdy_command"))) value:$howdy_command placeholder:"howdy compare"
    enum_row label:(Reactive::of(|| telar::t!("settings.field.notification_detail"))) value:$notification_detail options:NOTIFICATION_DETAILS
    enum_row label:(Reactive::of(|| telar::t!("settings.field.media_detail"))) value:$media_detail options:MEDIA_DETAILS
    save_row label:(Reactive::of(|| telar::t!("settings.save.lock"))) on_press:save
