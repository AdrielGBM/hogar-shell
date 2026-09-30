//! Reading and writing one value of an instance's options by where it sits: a dotted key, then an index into a list or a name in a table below it.

use toml::{Table, Value};

/// One step down into an options table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    Key(String),
    Index(usize),
}

pub type Path = Vec<Step>;

/// The path a field's dotted key names: `face.scale` is `face`, then `scale`.
pub fn path_of(key: &str) -> Path {
    key.split('.')
        .map(|part| Step::Key(part.to_string()))
        .collect()
}

pub fn get<'a>(table: &'a Table, path: &[Step]) -> Option<&'a Value> {
    let (first, rest) = path.split_first()?;
    let Step::Key(key) = first else {
        return None;
    };
    let mut at = table.get(key)?;
    for step in rest {
        at = match (step, at) {
            (Step::Key(key), Value::Table(inner)) => inner.get(key)?,
            (Step::Index(index), Value::Array(items)) => items.get(*index)?,
            _ => return None,
        };
    }
    Some(at)
}

/// Writes `value` at `path` in `own`, the options an instance sets itself. What `path` passes through and `own` does not have yet is taken from `shown`, the options as the screen has them: a list whole, since a list is one value and setting one element of it sets all of it, and a table empty, since a table is merged key by key and copying it would pin every key in it.
pub fn set(own: &mut Table, shown: &Table, path: &[Step], value: Value) {
    let Some((last, parents)) = path.split_last() else {
        return;
    };
    let mut container = OwnContainer::Table(own);
    for (depth, step) in parents.iter().enumerate() {
        let inherited = get(shown, &path[..=depth]);
        let blank = match inherited {
            Some(Value::Array(items)) => Value::Array(items.clone()),
            _ => Value::Table(Table::new()),
        };
        container = match container.child(step, blank) {
            Some(child) => child,
            None => return,
        };
    }
    container.put(last, value);
}

/// Takes the value at `path` out of `own`, so the instance goes back to what it inherits there.
pub fn unset(own: &mut Table, path: &[Step]) {
    let Some((last, parents)) = path.split_last() else {
        return;
    };
    let mut at: &mut Value = match parents.first() {
        None => {
            if let Step::Key(key) = last {
                own.remove(key);
            }
            return;
        }
        Some(Step::Key(key)) => match own.get_mut(key) {
            Some(value) => value,
            None => return,
        },
        Some(Step::Index(_)) => return,
    };
    for step in &parents[1..] {
        at = match (step, at) {
            (Step::Key(key), Value::Table(inner)) => match inner.get_mut(key) {
                Some(value) => value,
                None => return,
            },
            (Step::Index(index), Value::Array(items)) => match items.get_mut(*index) {
                Some(value) => value,
                None => return,
            },
            _ => return,
        };
    }
    match (last, at) {
        (Step::Key(key), Value::Table(inner)) => {
            inner.remove(key);
        }
        (Step::Index(index), Value::Array(items)) if *index < items.len() => {
            items.remove(*index);
        }
        _ => {}
    }
}

enum OwnContainer<'a> {
    Table(&'a mut Table),
    Value(&'a mut Value),
}

impl<'a> OwnContainer<'a> {
    fn child(self, step: &Step, blank: Value) -> Option<OwnContainer<'a>> {
        let value = match (self, step) {
            (OwnContainer::Table(table), Step::Key(key)) => {
                table.entry(key.clone()).or_insert(blank)
            }
            (OwnContainer::Value(Value::Table(table)), Step::Key(key)) => {
                table.entry(key.clone()).or_insert(blank)
            }
            (OwnContainer::Value(Value::Array(items)), Step::Index(index)) => {
                items.get_mut(*index)?
            }
            _ => return None,
        };
        Some(OwnContainer::Value(value))
    }

    fn put(self, step: &Step, value: Value) {
        match (self, step) {
            (OwnContainer::Table(table), Step::Key(key))
            | (OwnContainer::Value(Value::Table(table)), Step::Key(key)) => {
                table.insert(key.clone(), value);
            }
            (OwnContainer::Value(Value::Array(items)), Step::Index(index)) => {
                if let Some(slot) = items.get_mut(*index) {
                    *slot = value;
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(text: &str) -> Table {
        toml::from_str(text).expect("a table")
    }

    #[test]
    fn a_nested_key_is_written_into_a_table_of_its_own_and_nothing_else_is_pinned() {
        let shown = table("[face]\nscale = 1.0\nhands = true\n");
        let mut own = Table::new();
        set(&mut own, &shown, &path_of("face.scale"), Value::Float(2.0));
        assert_eq!(own, table("[face]\nscale = 2.0\n"));
        assert_eq!(get(&own, &path_of("face.scale")), Some(&Value::Float(2.0)));
    }

    #[test]
    fn an_element_of_an_inherited_list_takes_the_whole_list_with_it() {
        let shown = table("hidden = [\"a\", \"b\", \"c\"]\n");
        let mut own = Table::new();
        let path = vec![Step::Key("hidden".into()), Step::Index(1)];
        set(&mut own, &shown, &path, Value::String("x".into()));
        assert_eq!(own, table("hidden = [\"a\", \"x\", \"c\"]\n"));
        unset(&mut own, &path);
        assert_eq!(own, table("hidden = [\"a\", \"c\"]\n"));
        unset(&mut own, &path_of("hidden"));
        assert!(own.is_empty());
    }
}
