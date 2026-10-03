//! Typed variables: values the user's layouts, rules and scripts set and read by name, kept in machine state (`state.json`) so they survive a restart.
//!
//! An expression reads one as `$name`. Its type is fixed by the value it was set to — text, number, bool, colour, image, font, or a list of one of them — and an image or a font reads as its text (a path, a family name), since the language has no such types.
//!
//! **Which exist.** [`declared`] answers a variable's type reactively, which is what an expression is checked against: whatever checked one that names a variable not set yet checks it again once it is, and comes alive without a reload.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};

use services::state::{self, ShellState, Var, VarList, VarType};
use telar::{Color, RwSignal};
use telar_expression::{Type, Value, format_number};
use util::report::Message;

use crate::scalar;

pub fn get(name: &str) -> Option<Var> {
    state::get().vars.get(name).cloned()
}

pub fn list() -> BTreeMap<String, Var> {
    state::get().vars
}

thread_local! {
    static TYPES: RefCell<Option<HashMap<String, RwSignal<Option<VarType>>>>> = const { RefCell::new(None) };
}

/// The type of the variable called `name`, or `None` while there is none, as a signal on this thread: what reads it in an effect or a memo runs again when that variable appears, goes or changes type, and not when only its value moves.
pub fn declared(name: &str) -> Option<VarType> {
    let cell = TYPES.with(|types| {
        let mut types = types.borrow_mut();
        let types = types.get_or_insert_with(|| {
            platform_wayland::app_watch(state::subscribe, |state: ShellState| {
                follow_types(&state.vars)
            });
            HashMap::new()
        });
        *types
            .entry(name.to_string())
            .or_insert_with(|| telar::detached(|| telar::signal(get(name).as_ref().map(Var::ty))))
    });
    cell.get()
}

/// Brings every type [`declared`] has answered on this thread up to `vars`: as each state is published, and at once after a write here, so the thread that set a variable reads it back without waiting for the publication.
fn follow_types(vars: &BTreeMap<String, Var>) {
    let cells: Vec<(RwSignal<Option<VarType>>, Option<VarType>)> = TYPES.with(|types| {
        types
            .borrow()
            .iter()
            .flatten()
            .map(|(name, cell)| (*cell, vars.get(name).map(Var::ty)))
            .collect()
    });
    telar::batch(|| {
        for (cell, ty) in cells {
            if cell.is_alive() && cell.peek() != ty {
                cell.set(ty);
            }
        }
    });
}

/// Sets `name` to `value`, persisting it. Refused for a name an expression could not read and for a value that is not a valid one of its own type: a number that is not finite, a colour that is not one, a list whose items are not all of its declared type.
pub fn set(name: &str, value: Var) -> Result<(), Message> {
    check_name(name)?;
    write(name, |_| Ok(value)).map(drop)
}

/// Forgets `name`, answering what it held.
pub fn remove(name: &str) -> Option<Var> {
    let held = state::try_update(|state| state.vars.remove(name).ok_or(())).ok()?;
    follow_types(&list());
    Some(held)
}

/// Sets `name` from text, as a command line hands it in: read as `asked` when one is named, else as the type the variable already has, else as text. A variable keeps its type: asking for another one is refused, as is text that does not read as the type it holds. The type is read and the value written in one step, so nothing set in between is overwritten unchecked.
pub fn set_text(name: &str, text: &str, asked: Option<&VarType>) -> Result<Var, Message> {
    check_name(name)?;
    write(name, |held| {
        if let (Some(asked), Some(held)) = (asked, held)
            && asked != held
        {
            return Err(util::message!(
                "finding.var_type_change",
                name = name,
                held = type_name(held),
                asked = type_name(asked)
            ));
        }
        let ty = asked.or(held).cloned().unwrap_or(VarType::Text);
        parse(&ty, text).map_err(|why| match held {
            Some(held) => util::message!(
                "finding.var_holds",
                name = name,
                held = type_name(held),
                why = why
            ),
            None => why,
        })
    })
}

/// Sets `name` to `value`, an expression's result of type `ty`, as a rule's `store` does: the variable takes the value's type, except that a text stays an image or a font when the variable already is one, since the language reads both as text.
pub fn store(name: &str, value: &Value, ty: &Type) -> Result<Var, Message> {
    check_name(name)?;
    write(name, |held| from_value(value, ty, held))
}

/// Writes what `make` answers from the type `name` holds now, under the state's one write lock. A refused value publishes and persists nothing.
fn write(
    name: &str,
    make: impl FnOnce(Option<&VarType>) -> Result<Var, Message>,
) -> Result<Var, Message> {
    let value = state::try_update(|state| {
        let held = state.vars.get(name).map(Var::ty);
        let value = make(held.as_ref())?;
        check(&value)?;
        state.vars.insert(name.to_string(), value.clone());
        Ok::<_, Message>(value)
    })?;
    follow_types(&list());
    Ok(value)
}

/// `text` read as a `ty` ([`scalar`]): `42`, `true`, `#88c0d0`, a path, a family name. A list is its items separated by commas.
pub fn parse(ty: &VarType, text: &str) -> Result<Var, Message> {
    let text = text.trim();
    Ok(match ty {
        VarType::Text => Var::Text(text.to_string()),
        VarType::Image => Var::Image(text.to_string()),
        VarType::Font => Var::Font(text.to_string()),
        VarType::Number => Var::Number(scalar::number(text)?),
        VarType::Bool => Var::Bool(scalar::truth(text)?),
        VarType::Color => {
            scalar::color(text)?;
            Var::Color(text.to_string())
        }
        VarType::List(element) => Var::List(VarList {
            of: (**element).clone(),
            items: match text.is_empty() {
                true => Vec::new(),
                false => text
                    .split(',')
                    .map(|item| parse(element, item))
                    .collect::<Result<_, _>>()?,
            },
        }),
    })
}

/// A type by the name `var set` takes: `text`, `number`, `bool`, `colour` (or `color`), `image`, `font`, or `list:<type>` of one of those.
pub fn parse_type(name: &str) -> Result<VarType, Message> {
    let name = name.trim();
    if let Some(element) = name.strip_prefix("list:") {
        return match scalar_type(element.trim()) {
            Some(element) => Ok(VarType::List(Box::new(element))),
            None if element.trim().starts_with("list:") => {
                Err(util::message!("finding.list_of_lists"))
            }
            None => Err(not_a_type(element.trim())),
        };
    }
    scalar_type(name).ok_or_else(|| not_a_type(name))
}

fn scalar_type(name: &str) -> Option<VarType> {
    Some(match name {
        "text" => VarType::Text,
        "number" => VarType::Number,
        "bool" => VarType::Bool,
        "colour" | "color" => VarType::Color,
        "image" => VarType::Image,
        "font" => VarType::Font,
        _ => return None,
    })
}

fn not_a_type(name: &str) -> Message {
    util::message!("finding.not_a_type", name = name)
}

/// A type by the name [`parse_type`] reads back, spelt as the expression language spells it in its own messages.
pub fn type_name(ty: &VarType) -> String {
    match ty {
        VarType::Text => "text".to_string(),
        VarType::Number => "number".to_string(),
        VarType::Bool => "bool".to_string(),
        VarType::Color => "colour".to_string(),
        VarType::Image => "image".to_string(),
        VarType::Font => "font".to_string(),
        VarType::List(element) => format!("list:{}", type_name(element)),
    }
}

/// A variable's value as [`parse`] reads it: what `var get` prints, a number as the expression language prints one.
pub fn show(var: &Var) -> String {
    match var {
        Var::Text(text) | Var::Image(text) | Var::Font(text) | Var::Color(text) => text.clone(),
        Var::Number(n) => format_number(*n),
        Var::Bool(b) => b.to_string(),
        Var::List(list) => list.items.iter().map(show).collect::<Vec<_>>().join(", "),
    }
}

/// A variable's type in the expression language.
pub fn type_of(ty: &VarType) -> Type {
    match ty {
        VarType::Text | VarType::Image | VarType::Font => Type::Text,
        VarType::Number => Type::Number,
        VarType::Bool => Type::Bool,
        VarType::Color => Type::Color,
        VarType::List(element) => Type::list(type_of(element)),
    }
}

/// What a variable holds once set to `value`, an expression's result of type `ty`, given the type it holds now ([`store`]).
fn from_value(value: &Value, ty: &Type, held: Option<&VarType>) -> Result<Var, Message> {
    let var_type = var_type_of(ty)?;
    let var_type = match (var_type, held) {
        (VarType::Text, Some(held @ (VarType::Image | VarType::Font))) => held.clone(),
        (var_type, _) => var_type,
    };
    stored(value, &var_type)
}

pub(crate) fn var_type_of(ty: &Type) -> Result<VarType, Message> {
    Ok(match ty {
        Type::Number => VarType::Number,
        Type::Text | Type::Never => VarType::Text,
        Type::Bool => VarType::Bool,
        Type::Color => VarType::Color,
        Type::List(element) => match **element {
            Type::List(_) => return Err(util::message!("finding.list_of_lists")),
            ref element => VarType::List(Box::new(var_type_of(element)?)),
        },
    })
}

fn stored(value: &Value, ty: &VarType) -> Result<Var, Message> {
    Ok(match (ty, value) {
        (VarType::Number, Value::Number(n)) => Var::Number(*n),
        (VarType::Bool, Value::Bool(b)) => Var::Bool(*b),
        (VarType::Color, Value::Color(_)) => Var::Color(value.to_string()),
        (VarType::Text, Value::Text(text)) => Var::Text(text.to_string()),
        (VarType::Image, Value::Text(text)) => Var::Image(text.to_string()),
        (VarType::Font, Value::Text(text)) => Var::Font(text.to_string()),
        (VarType::List(element), Value::List(items)) => Var::List(VarList {
            of: (**element).clone(),
            items: items
                .iter()
                .map(|item| stored(item, element))
                .collect::<Result<_, _>>()?,
        }),
        (ty, value) => {
            return Err(util::message!(
                "finding.var_value_type",
                value = value,
                held = type_name(ty)
            ));
        }
    })
}

/// A variable's value in the expression language. A stored colour that no longer reads as one is transparent rather than an error, since it was checked when it was set.
pub fn value_of(var: &Var) -> Value {
    match var {
        Var::Text(text) | Var::Image(text) | Var::Font(text) => Value::text(text.as_str()),
        Var::Number(n) => Value::Number(*n),
        Var::Bool(b) => Value::Bool(*b),
        Var::Color(text) => Value::Color(scalar::color(text).unwrap_or(Color::TRANSPARENT)),
        Var::List(list) => Value::list(list.items.iter().map(value_of)),
    }
}

/// Whether an expression could read a variable called `name` as `$name`.
pub fn check_name(name: &str) -> Result<(), Message> {
    match telar_expression::is_identifier(name) {
        true => Ok(()),
        false => Err(util::message!("finding.var_name", name = name)),
    }
}

fn check(var: &Var) -> Result<(), Message> {
    match var {
        Var::Number(n) if !n.is_finite() => {
            Err(util::message!("finding.var_not_finite", value = n))
        }
        Var::Color(text) => scalar::color(text).map(drop),
        Var::List(list) => {
            for item in &list.items {
                if item.ty() != list.of {
                    return Err(util::message!("finding.var_list_items"));
                }
                check(item)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_is_read_as_the_type_asked_for() {
        assert_eq!(parse(&VarType::Number, " 42 "), Ok(Var::Number(42.0)));
        assert!(parse(&VarType::Number, "forty").is_err());
        assert!(parse(&VarType::Number, "inf").is_err());
        assert_eq!(parse(&VarType::Bool, "yes"), Ok(Var::Bool(true)));
        assert_eq!(
            parse(&VarType::Color, "#88c0d0"),
            Ok(Var::Color("#88c0d0".to_string()))
        );
        assert!(parse(&VarType::Color, "teal").is_err());
        assert_eq!(
            parse(&VarType::List(Box::new(VarType::Number)), "1, 2,3"),
            Ok(Var::List(VarList {
                of: VarType::Number,
                items: vec![Var::Number(1.0), Var::Number(2.0), Var::Number(3.0)],
            }))
        );
        assert_eq!(
            parse_type("list:color"),
            Ok(VarType::List(Box::new(VarType::Color)))
        );
        assert!(parse_type("date").is_err());
    }

    #[test]
    fn a_value_and_its_type_print_as_they_are_read() {
        let ty = parse_type("list:number").unwrap();
        assert_eq!(type_name(&ty), "list:number");
        assert_eq!(show(&parse(&ty, "1,2.5").unwrap()), "1, 2.5");
        assert_eq!(show(&Var::Number(42.0)), "42");
        assert_eq!(show(&Var::Bool(false)), "false");
    }

    #[test]
    fn an_image_and_a_font_read_as_text() {
        assert_eq!(type_of(&VarType::Image), Type::Text);
        assert_eq!(type_of(&VarType::Font), Type::Text);
        assert_eq!(
            value_of(&Var::Font("Inter".to_string())),
            Value::text("Inter")
        );
        assert_eq!(
            type_of(&VarType::List(Box::new(VarType::Color))),
            Type::list(Type::Color)
        );
    }

    #[test]
    fn a_variable_is_set_kept_and_removed_through_machine_state() {
        set("vars_test_mood", Var::Text("calm".to_string())).unwrap();
        assert_eq!(get("vars_test_mood"), Some(Var::Text("calm".to_string())));
        assert_eq!(
            set_text("vars_test_mood", "busy", None),
            Ok(Var::Text("busy".to_string()))
        );
        set_text("vars_test_count", "3", Some(&VarType::Number)).unwrap();
        assert!(
            set_text("vars_test_count", "three", None).is_err(),
            "a variable keeps the type it was set as"
        );
        assert!(list().contains_key("vars_test_count"));
        assert_eq!(remove("vars_test_count"), Some(Var::Number(3.0)));
        assert_eq!(get("vars_test_count"), None);
    }

    #[test]
    fn what_an_expression_could_not_read_is_refused() {
        assert!(set("2fast", Var::Bool(true)).is_err());
        assert!(set("a-b", Var::Bool(true)).is_err());
        assert!(set("n", Var::Number(f64::NAN)).is_err());
        let mixed = Var::List(VarList {
            of: VarType::Number,
            items: vec![Var::Text("one".to_string())],
        });
        assert!(set("mixed", mixed).is_err());
    }

    #[test]
    fn a_list_of_lists_is_not_a_type() {
        assert_eq!(
            parse_type("list:list:number").map_err(|why| why.key().map(str::to_string)),
            Err(Some("finding.list_of_lists".to_string()))
        );
        assert!(
            parse_type("list:date")
                .unwrap_err()
                .english()
                .contains("`date` is not a type")
        );
        assert_eq!(
            parse_type(" list: colour "),
            Ok(VarType::List(Box::new(VarType::Color)))
        );
    }

    #[test]
    fn a_type_is_named_as_the_expression_language_names_it() {
        assert_eq!(type_name(&VarType::Color), Type::Color.to_string());
        assert_eq!(type_name(&VarType::Number), Type::Number.to_string());
        assert_eq!(type_name(&VarType::Text), Type::Text.to_string());
        assert_eq!(type_name(&VarType::Bool), Type::Bool.to_string());
        for ty in [VarType::Color, VarType::List(Box::new(VarType::Color))] {
            assert_eq!(parse_type(&type_name(&ty)), Ok(ty));
        }
    }

    #[test]
    fn a_number_prints_as_the_expression_language_prints_it() {
        assert_eq!(show(&Var::Number(0.1 + 0.2)), "0.3");
        assert_eq!(show(&Var::Number(-0.0)), "0");
        assert_eq!(show(&Var::Number(1e21)), format_number(1e21));
    }

    #[test]
    fn setting_from_text_checks_the_held_type_as_it_writes() {
        set_text("vars_test_kept", "2", Some(&VarType::Number)).unwrap();
        let refused = set_text("vars_test_kept", "on", Some(&VarType::Bool))
            .unwrap_err()
            .english();
        assert!(refused.contains("is a number, not a bool"), "{refused}");
        let refused = set_text("vars_test_kept", "many", None)
            .unwrap_err()
            .english();
        assert!(
            refused.starts_with("`vars_test_kept` is a number:"),
            "{refused}"
        );
        assert_eq!(get("vars_test_kept"), Some(Var::Number(2.0)));
        assert!(set_text("vars_test_kept", "1", Some(&VarType::Number)).is_ok());
        remove("vars_test_kept");
    }

    /// Every write is one publication of the whole state, persisted with it, so a refused one must publish nothing: a state that arrives equal to the one before it is a write that changed nothing being fanned out and written to disk.
    #[test]
    fn a_refused_write_publishes_and_persists_nothing() {
        set("vars_test_steady", Var::Number(1.0)).unwrap();
        let (tx, published) = platform_wayland::detached();
        state::subscribe(tx);

        assert!(set_text("vars_test_steady", "many", None).is_err());
        assert!(set_text("vars_test_steady", "on", Some(&VarType::Bool)).is_err());
        assert!(set("vars_test_steady", Var::Number(f64::INFINITY)).is_err());
        let nested = Value::list([Value::list(Vec::<Value>::new())]);
        assert!(
            store(
                "vars_test_steady",
                &nested,
                &Type::list(Type::list(Type::Number))
            )
            .is_err()
        );
        assert_eq!(remove("vars_test_never_set"), None);
        set("vars_test_steady_after", Var::Bool(true)).unwrap();

        let mut seen: Vec<ShellState> = Vec::new();
        while !seen
            .last()
            .is_some_and(|state| state.vars.contains_key("vars_test_steady_after"))
        {
            match published.try_recv() {
                Some(state) => seen.push(state),
                None => std::thread::yield_now(),
            }
        }
        for pair in seen.windows(2) {
            assert_ne!(
                pair[0], pair[1],
                "a write that changed nothing was published"
            );
        }
        assert_eq!(get("vars_test_steady"), Some(Var::Number(1.0)));
        remove("vars_test_steady");
        remove("vars_test_steady_after");
    }

    #[test]
    fn a_stored_text_stays_the_image_the_variable_holds() {
        set("vars_test_picture", Var::Image("/a.png".to_string())).unwrap();
        assert_eq!(
            store("vars_test_picture", &Value::text("/b.png"), &Type::Text),
            Ok(Var::Image("/b.png".to_string()))
        );
        assert_eq!(
            store("vars_test_picture", &Value::Number(3.0), &Type::Number),
            Ok(Var::Number(3.0)),
            "a store takes the value's type"
        );
        assert!(store("2fast", &Value::Bool(true), &Type::Bool).is_err());
        remove("vars_test_picture");
    }
}
