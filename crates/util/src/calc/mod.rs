//! The launcher calculator's answers: arithmetic from `telar-expression`, conversions from [`units`], and [`qalc`] as the fallback.
//!
//! [`units`] answers `3 km in mi` from a static table, with no subprocess. [`qalc`] is the fallback for everything neither of them does, and it *is* a subprocess, so it runs on a worker thread and its answer arrives when it arrives.

use telar_expression::{calculate, format_number};

pub mod qalc;
pub mod units;

/// An answer the calculator can give: a plain number, or a number in a unit.
#[derive(Clone, Debug, PartialEq)]
pub enum Answer {
    Number(f64),
    Quantity { value: f64, unit: &'static str },
}

impl Answer {
    /// What to show, and what selecting the row copies. The unit travels with the number: an answer of `1.86` pasted somewhere else is a different claim from `1.86 mi`.
    pub fn text(&self) -> String {
        match self {
            Answer::Number(value) => format_number(*value),
            Answer::Quantity { value, unit } => format!("{} {unit}", format_number(*value)),
        }
    }
}

/// Solves `input`, whatever kind of question it is: a conversion first, then arithmetic.
///
/// Conversions are tried first because `3 km in mi` is not arithmetic at all — the evaluator rejects it on the first unit name — and because a conversion that parses is unambiguous about what was meant.
pub fn solve(input: &str) -> Option<Answer> {
    if let Some(quantity) = units::convert(input) {
        return Some(Answer::Quantity {
            value: quantity.value,
            unit: quantity.unit,
        });
    }
    calculate(input).ok().map(Answer::Number)
}

/// Whether `query` reads as a calculation worth showing a result for.
///
/// A bare number is deliberately excluded: typing `2` is far more likely the start of an app name than a sum the user wants echoed back at them.
pub fn looks_like_math(query: &str) -> bool {
    let trimmed = query.trim();
    if trimmed.is_empty() || trimmed.parse::<f64>().is_ok() {
        return false;
    }
    solve(trimmed).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn solve_answers_arithmetic_and_conversions_through_one_call() {
        assert_eq!(solve("2+2").map(|a| a.text()).as_deref(), Some("4"));
        assert_eq!(
            solve("3 km in mi").map(|a| a.text()).as_deref(),
            Some("1.8641135767 mi"),
            "the unit travels with the number, since 1.86 alone is a different claim"
        );
        assert_eq!(solve("firefox").map(|a| a.text()), None);

        // A conversion is tried first because the evaluator cannot see one at all: it stops at the unit's name.
        assert!(calculate("3 km in mi").is_err());
        assert!(looks_like_math("3 km in mi"));
        assert!(looks_like_math("100 c in f"));
        assert!(!looks_like_math("photos in library"));
    }

    #[test]
    fn looks_like_math_ignores_a_bare_number() {
        assert!(
            !looks_like_math("2"),
            "typing '2' is the start of a name, not a sum"
        );
        assert!(!looks_like_math("42"));
        assert!(!looks_like_math("firefox"));
        assert!(looks_like_math("2+2"));
        assert!(looks_like_math("sqrt(2)"));
        assert!(looks_like_math(" 8 * 7 "));
    }
}
