[logic]
use crate::form::{parse_u32, persist_with, source};
use ::config::LauncherConfig;

let (config, path) = source();
let l = &config.launcher;
let base = l.clone();
let width = signal(l.width.to_string());
let height = signal(l.height.to_string());
let max_results = signal(l.max_results.to_string());
let fuzzy = signal(l.fuzzy);
let calculator = signal(l.calculator);
let qalc = signal(l.qalc);
let dangerous = signal(l.enable_dangerous_actions);

let save: Box<dyn Fn()> = Box::new({
    let (width, height, max_results) = (width.clone(), height.clone(), max_results.clone());
    let (fuzzy, calculator, qalc, dangerous) = (
        fuzzy.clone(),
        calculator.clone(),
        qalc.clone(),
        dangerous.clone(),
    );
    move || {
        // Merged into the file as it is now, because the applications page owns the other half of this same `[launcher]` table. A snapshot taken when the form was built would quietly revert a favourite marked since — see `persist_with`.
        persist_with(&path, "launcher", |current| LauncherConfig {
            width: parse_u32(&width.peek(), base.width),
            height: parse_u32(&height.peek(), base.height),
            max_results: parse_u32(&max_results.peek(), base.max_results),
            fuzzy: fuzzy.peek(),
            calculator: calculator.peek(),
            qalc: qalc.peek(),
            enable_dangerous_actions: dangerous.peek(),
            favourites: current.launcher.favourites.clone(),
            hidden: current.launcher.hidden.clone(),
            icons: current.launcher.icons.clone(),
            actions: current.launcher.actions.clone(),
        });
    }
});

[view]
form_section title(|| telar::t!("settings.section.launcher"))
    text_row label(|| telar::t!("settings.field.width")) value:$width placeholder:"640"
    text_row label(|| telar::t!("settings.field.height")) value:$height placeholder:"420"
    text_row label(|| telar::t!("settings.field.max_results")) value:$max_results placeholder:"12"
    toggle_row label(|| telar::t!("settings.field.fuzzy")) value:$fuzzy
    toggle_row label(|| telar::t!("settings.field.calculator")) value:$calculator
    toggle_row label(|| telar::t!("settings.field.qalc")) value:$qalc
    toggle_row label(|| telar::t!("settings.field.enable_dangerous_actions")) value:$dangerous
    save_row label(|| telar::t!("settings.save.launcher")) on_press(save)
