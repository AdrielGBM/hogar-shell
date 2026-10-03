//! `hogar-shell rule` — the `[[rules]]` the running shell has loaded from `config.toml` (TA-6).

use automation::rules;

use super::args::arg;
use super::{Command, Target};

pub(crate) const RULE: Target = Target {
    name: "rule",
    commands: &[
        Command {
            name: "list",
            args: "",
            help: "every rule: id, trigger, state (on, off, invalid or suspended) and when it last fired, tab-separated",
            run: |_| Ok(list()),
        },
        Command {
            name: "run",
            args: "<id>",
            help: "run a rule's commands now, ignoring its trigger, `when` and `enabled`, and print each with its reply; its `store` is not written",
            run: |args| rules::shell().run_now(arg(args, 0, "id")?),
        },
    ],
};

fn list() -> String {
    rules::shell()
        .list()
        .iter()
        .map(|rule| {
            format!(
                "{}\t{}\t{}\t{}",
                rule.id,
                rule.trigger,
                rule.state.as_str(),
                rule.fired_at().as_deref().unwrap_or("never")
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::super::dispatch;

    #[test]
    fn a_rule_nobody_wrote_is_named_as_missing() {
        let reply = dispatch("rule run nobody");
        assert!(
            reply.starts_with("err there is no rule called `nobody`"),
            "{reply}"
        );
        assert_eq!(dispatch("rule run"), "err missing argument <id>");
        assert_eq!(dispatch("rule list"), "ok");
    }
}
