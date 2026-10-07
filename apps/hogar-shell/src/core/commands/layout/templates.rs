use layout::templates;
use surfaces::layouts;

use super::super::args::{Args, arg};

pub(super) fn template(args: &Args<'_>) -> Result<String, String> {
    match args.first().copied() {
        Some("list") => match args.get(1) {
            None => Ok(list()),
            Some(extra) => Err(format!(
                "'{extra}' is not something `layout template list` takes"
            )),
        },
        Some("use") => use_template(&args[1..]),
        Some(other) => Err(format!(
            "'{other}' is not something `layout template` does (try: list, use)"
        )),
        None => Err("say `list` or `use <name> [as <layout-name>]`".to_string()),
    }
}

fn list() -> String {
    templates::all()
        .iter()
        .map(|template| format!("{}\t{}", template.name(), template.description.english()))
        .collect::<Vec<_>>()
        .join("\n")
}

fn use_template(words: &[&str]) -> Result<String, String> {
    let name = arg(words, 0, "name")?;
    let called = match &words[1..] {
        [] => None,
        ["as", called] => Some(*called),
        ["as"] => return Err("missing argument <layout-name> after `as`".to_string()),
        _ => {
            return Err(format!(
                "expected `as <layout-name>` after the template's name, got `{}`",
                words[1..].join(" ")
            ));
        }
    };
    let id =
        layouts::start(|store| templates::put(store, name, called).map_err(|why| why.message()))
            .map_err(|why| why.english())?;
    Ok(format!(
        "made the layout `{id}` from the template `{name}` and drew it; the layout drawn before is as it was"
    ))
}

#[cfg(test)]
mod tests {
    use layout::{LayoutId, LayoutStore};

    use super::*;
    use crate::test_support::shell_holding;

    fn run(line: &str) -> Result<String, String> {
        crate::core::commands::dispatch_locally(
            &line.split(' ').map(String::from).collect::<Vec<_>>(),
        )
    }

    #[test]
    fn template_list_names_each_template_with_what_it_holds() {
        let listed = run("layout template list").expect("it lists");
        assert_eq!(
            listed,
            "showcase\tA desktop of weather, media, processor, clock and visualiser widgets, and a full top bar"
        );
        assert!(run("layout template list extra").is_err());
        assert!(run("layout template").is_err());
        assert!(run("layout template remove showcase").is_err());
    }

    #[test]
    fn template_use_makes_and_draws_a_new_layout_and_leaves_the_one_drawn_before() {
        let mut mine = layout::built_in();
        mine.id = LayoutId::new("mine");
        let store = shell_holding("templates-use", "mine", &mine, &[]);
        let mine = store.borrow().get(&LayoutId::new("mine")).cloned();

        let said = run("layout template use showcase").expect("it is made");
        assert!(said.contains("`showcase`"), "{said}");
        assert_eq!(store.borrow().active_id().as_str(), "showcase");
        assert_eq!(
            services::state::get().layout.as_deref(),
            Some("showcase"),
            "the new layout is what this installation draws"
        );
        assert_eq!(store.borrow().get(&LayoutId::new("mine")).cloned(), mine);
        assert_eq!(
            store.borrow().active().outputs,
            layout::templates::named("showcase")
                .expect("the showcase ships")
                .layout()
                .expect("its layout")
                .outputs
        );

        run("layout template use showcase as work").expect("named");
        assert_eq!(store.borrow().active_id().as_str(), "work");
        run("layout template use showcase").expect("numbered");
        assert_eq!(store.borrow().active_id().as_str(), "showcase-2");

        for (line, says) in [
            (
                "layout template use showcase as mine",
                "a layout called `mine` exists already",
            ),
            ("layout template use showcase as default", "`default`"),
            (
                "layout template use showcase as Work!",
                "cannot name a layout",
            ),
            ("layout template use showcase as", "missing argument"),
            ("layout template use showcase like work", "expected `as"),
            ("layout template use nothing", "there is no template called"),
            ("layout template use", "missing argument <name>"),
        ] {
            let refused = run(line).expect_err(line);
            assert!(refused.contains(says), "{line}: {refused}");
        }
        assert_eq!(store.borrow().active_id().as_str(), "showcase-2");
        assert_eq!(
            store.borrow().names().count(),
            5,
            "default, mine and three copies"
        );
    }

    #[test]
    fn the_showcase_template_passes_layout_check() {
        assert!(
            ui::descriptor::installed().is_empty(),
            "checked as the command line checks it, with no module table installed"
        );
        let dir = util::paths::isolated_root()
            .expect("a test process resolves under its scratch root")
            .join("layout-templates-check");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a layouts directory");
        let (mut store, report) = LayoutStore::load(&dir);
        templates::put(&mut store, "showcase", None).expect("it is made");
        let verdict = super::super::check_in(store, report, Some("showcase"));
        assert!(verdict.is_ok(), "{}", verdict.unwrap_err());
    }
}
