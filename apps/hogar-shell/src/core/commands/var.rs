//! `hogar-shell var` — the typed variables layouts and rules read as `$name` (TA-6).
//!
//! A variable is machine state, so `set` recolours or rewrites every binding that reads it live, with no reload: the bindings follow the state feed. The first `set` of a name fixes its type, from `--type` or else as text; every later one is read as that type and refused when it does not fit.

use automation::vars;

use super::args::{Args, arg, flag};
use super::{Command, Target};

pub(crate) const VAR: Target = Target {
    name: "var",
    commands: &[
        Command {
            name: "get",
            args: "<name>",
            help: "a variable's value",
            run: |args| get(arg(args, 0, "name")?),
        },
        Command {
            name: "set",
            args: "<name> <value...> [--type <t>]",
            help: "set a variable to the rest of the line, keeping its type; a new one is text unless --type says number, bool, colour, image, font or list:<type>",
            run: set,
        },
        Command {
            name: "list",
            args: "",
            help: "every variable: name, type and value, tab-separated",
            run: |_| Ok(list()),
        },
        Command {
            name: "remove",
            args: "<name>",
            help: "forget a variable",
            run: |args| remove(arg(args, 0, "name")?),
        },
    ],
};

fn get(name: &str) -> Result<String, String> {
    vars::get(name)
        .map(|var| vars::show(&var))
        .ok_or_else(|| missing(name))
}

/// The value is the rest of the line as written, less `--type <t>` wherever it stands.
fn set(args: &Args<'_>) -> Result<String, String> {
    let name = arg(args, 0, "name")?;
    let ty = flag(&args[1..], "--type", "t")?;
    let value = match &ty {
        Some(ty) => {
            let at = 1 + ty.at;
            [args.text(1..at), args.rest(at + 2)]
                .into_iter()
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>()
                .join(" ")
        }
        None => args.rest(1).to_string(),
    };
    if value.is_empty() {
        return Err("missing argument <value>".to_string());
    }
    let asked = ty
        .map(|ty| vars::parse_type(ty.value))
        .transpose()
        .map_err(|why| why.english())?;
    let value = vars::set_text(name, &value, asked.as_ref()).map_err(|why| why.english())?;
    Ok(format!(
        "{name} = {} ({})",
        vars::show(&value),
        vars::type_name(&value.ty())
    ))
}

fn list() -> String {
    vars::list()
        .iter()
        .map(|(name, var)| {
            format!(
                "{name}\t{}\t{}",
                vars::type_name(&var.ty()),
                vars::show(var)
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn remove(name: &str) -> Result<String, String> {
    vars::remove(name)
        .map(|_| format!("removed {name}"))
        .ok_or_else(|| missing(name))
}

fn missing(name: &str) -> String {
    let known: Vec<String> = vars::list().into_keys().collect();
    match known.is_empty() {
        true => format!("there is no variable called `{name}`, and none are set"),
        false => format!(
            "there is no variable called `{name}` (try: {})",
            known.join(", ")
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use services::state::Var;

    fn reply(line: &str) -> String {
        super::super::dispatch(line)
    }

    #[test]
    fn a_new_variable_takes_the_type_asked_for_and_keeps_it() {
        assert_eq!(
            reply("var set cmd_test_count 3 --type number"),
            "ok cmd_test_count = 3 (number)"
        );
        assert_eq!(reply("var get cmd_test_count"), "ok 3");
        assert_eq!(
            reply("var set cmd_test_count 4.5"),
            "ok cmd_test_count = 4.5 (number)"
        );
        let refused = reply("var set cmd_test_count many");
        assert!(
            refused.starts_with("err `cmd_test_count` is a number"),
            "{refused}"
        );
        let retyped = reply("var set cmd_test_count 1 --type bool");
        assert!(retyped.contains("var remove cmd_test_count"), "{retyped}");
        assert_eq!(reply("var get cmd_test_count"), "ok 4.5");
        assert_eq!(
            reply("var remove cmd_test_count"),
            "ok removed cmd_test_count"
        );
        assert!(reply("var get cmd_test_count").starts_with("err there is no variable"));
        assert!(reply("var remove cmd_test_count").starts_with("err there is no variable"));
    }

    #[test]
    fn each_type_reads_its_own_text() {
        assert_eq!(
            reply("var set cmd_test_flag on --type bool"),
            "ok cmd_test_flag = true (bool)"
        );
        assert_eq!(
            reply("var set cmd_test_tint #ff8800 --type color"),
            "ok cmd_test_tint = #ff8800 (colour)"
        );
        assert!(
            reply("var set cmd_test_tint orange").starts_with("err `cmd_test_tint` is a colour")
        );
        assert_eq!(
            reply("var set cmd_test_sizes 1, 2, 3 --type list:number"),
            "ok cmd_test_sizes = 1, 2, 3 (list:number)"
        );
        assert!(
            reply("var set cmd_test_sizes 1, x")
                .starts_with("err `cmd_test_sizes` is a list:number")
        );
        assert_eq!(
            reply("var set cmd_test_words hello  wide world"),
            "ok cmd_test_words = hello  wide world (text)"
        );
        assert!(
            reply("var set cmd_test_bad 1 --type date").starts_with("err `date` is not a type")
        );
        assert_eq!(
            reply("var set cmd_test_bad"),
            "err missing argument <value>"
        );
        assert_eq!(
            reply("var set cmd_test_bad 1 --type"),
            "err missing argument <t> after --type"
        );
        let listed = reply("var list");
        assert!(
            listed.contains("cmd_test_sizes\tlist:number\t1, 2, 3"),
            "{listed}"
        );
        for name in ["flag", "tint", "sizes", "words"] {
            reply(&format!("var remove cmd_test_{name}"));
        }
        assert_eq!(vars::get("cmd_test_flag"), None::<Var>);
    }

    #[test]
    fn a_value_keeps_its_spacing_wherever_the_type_is_written() {
        assert_eq!(
            reply(r#"var set cmd_test_said "a   b"  --type text"#),
            r#"ok cmd_test_said = "a   b" (text)"#
        );
        assert_eq!(
            reply("var set cmd_test_said --type text two  spaces"),
            "ok cmd_test_said = two  spaces (text)"
        );
        assert_eq!(
            reply("var set cmd_test_said left  --type text  right"),
            "ok cmd_test_said = left right (text)",
            "only the spacing where the flag was cut out is one space"
        );
        assert_eq!(
            reply("var set cmd_test_said x --type text --type text"),
            "err --type is given twice"
        );
        assert_eq!(
            reply("var set cmd_test_nested 1 --type list:list:number"),
            "err a variable cannot hold a list of lists"
        );
        reply("var remove cmd_test_said");
    }
}
