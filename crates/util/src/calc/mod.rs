//! Expression evaluation for the launcher's calculator mode.
//!
//! A recursive-descent evaluator over a hand-rolled tokenizer: no dependency, no subprocess, and fast enough to run on every keystroke on the UI thread — which is the requirement that rules out shelling out to `qalc` for the *live* result.
//!
//! Percentages are context-sensitive, because that is what people mean by them: `200 + 10%` is 220, not 200.1. A percentage on the right of `+`/`-` is taken *of the left operand*; anywhere else it is simply a hundredth.
//!
//! Two things sit on top of the evaluator. [`units`] answers `3 km in mi` from a static table, still with no dependency and no subprocess. [`qalc`] is the fallback for everything neither of them does — and it *is* a subprocess, so it runs on a worker thread and its answer arrives when it arrives.

pub mod qalc;
pub mod units;

/// A value plus whether it was written as a percentage, which the `+`/`-` rule needs.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Value {
    number: f64,
    percent: bool,
}

impl Value {
    fn plain(number: f64) -> Self {
        Self {
            number,
            percent: false,
        }
    }

    /// The number this stands for on its own: a percentage is a hundredth, so a bare `10%` is `0.1`.
    fn resolve(self) -> f64 {
        if self.percent {
            self.number / 100.0
        } else {
            self.number
        }
    }

    /// What to add to (or subtract from) `left`: `200 + 10%` moves by a tenth *of 200*.
    fn relative_to(self, left: f64) -> f64 {
        if self.percent {
            left * self.number / 100.0
        } else {
            self.number
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Token {
    Number(f64),
    Ident(String),
    Plus,
    Minus,
    Star,
    Slash,
    Caret,
    Percent,
    Open,
    Close,
}

/// Splits `input` into tokens, or `None` on a character that has no meaning here — which is how a query that is plainly not an expression (an app name) is rejected before any evaluation happens.
fn tokenize(input: &str) -> Option<Vec<Token>> {
    let mut tokens = Vec::new();
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if c.is_ascii_digit() || (c == '.' && chars.get(i + 1).is_some_and(char::is_ascii_digit)) {
            let start = i;
            // `_` groups digits (`1_000_000`); the float parser never sees it.
            while chars
                .get(i)
                .is_some_and(|c| c.is_ascii_digit() || *c == '.' || *c == '_')
            {
                i += 1;
            }
            // An exponent only counts when a digit (or a sign then a digit) follows, so `2e` is not a number and `2 e` keeps `e` as the constant.
            if let Some('e' | 'E') = chars.get(i).copied() {
                let mut lookahead = i + 1;
                if let Some('+' | '-') = chars.get(lookahead).copied() {
                    lookahead += 1;
                }
                if chars.get(lookahead).is_some_and(char::is_ascii_digit) {
                    i = lookahead;
                    while chars.get(i).is_some_and(char::is_ascii_digit) {
                        i += 1;
                    }
                }
            }
            let text: String = chars[start..i].iter().filter(|c| **c != '_').collect();
            tokens.push(Token::Number(text.parse().ok()?));
            continue;
        }
        if c.is_alphabetic() {
            let start = i;
            while chars.get(i).is_some_and(|c| c.is_alphanumeric()) {
                i += 1;
            }
            tokens.push(Token::Ident(chars[start..i].iter().collect()));
            continue;
        }
        let token = match c {
            '+' => Token::Plus,
            '-' | '−' => Token::Minus,
            '*' | '×' | '·' => Token::Star,
            '/' | '÷' => Token::Slash,
            '^' => Token::Caret,
            '%' => Token::Percent,
            '(' | '[' => Token::Open,
            ')' | ']' => Token::Close,
            _ => return None,
        };
        tokens.push(token);
        i += 1;
    }
    (!tokens.is_empty()).then_some(tokens)
}

struct Parser {
    tokens: Vec<Token>,
    at: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.at)
    }

    fn eat(&mut self, token: &Token) -> bool {
        if self.peek() == Some(token) {
            self.at += 1;
            return true;
        }
        false
    }

    fn expression(&mut self) -> Option<Value> {
        let mut left = self.term()?.resolve();
        loop {
            if self.eat(&Token::Plus) {
                left += self.term()?.relative_to(left);
            } else if self.eat(&Token::Minus) {
                left -= self.term()?.relative_to(left);
            } else {
                return Some(Value::plain(left));
            }
        }
    }

    fn term(&mut self) -> Option<Value> {
        let mut left = self.power()?;
        loop {
            if self.eat(&Token::Star) {
                left = Value::plain(left.resolve() * self.power()?.resolve());
            } else if self.eat(&Token::Slash) {
                let divisor = self.power()?.resolve();
                // Division by zero yields infinity in IEEE terms, but "∞" is not an answer a calculator should offer for `1/0`; refusing leaves the query looking like what it is — not a valid sum.
                if divisor == 0.0 {
                    return None;
                }
                left = Value::plain(left.resolve() / divisor);
            } else {
                return Some(left);
            }
        }
    }

    /// Right-associative, so `2^3^2` is `2^9`, matching every calculator and maths convention.
    fn power(&mut self) -> Option<Value> {
        let base = self.unary()?;
        if self.eat(&Token::Caret) {
            let exponent = self.power()?.resolve();
            return Some(Value::plain(base.resolve().powf(exponent)));
        }
        Some(base)
    }

    fn unary(&mut self) -> Option<Value> {
        if self.eat(&Token::Minus) {
            let inner = self.unary()?;
            // Negation keeps the percent flag, so `200 - -10%` is still a tenth of 200.
            return Some(Value {
                number: -inner.number,
                percent: inner.percent,
            });
        }
        self.eat(&Token::Plus);
        self.postfix()
    }

    fn postfix(&mut self) -> Option<Value> {
        let mut value = self.primary()?;
        if self.eat(&Token::Percent) {
            value.percent = true;
        }
        Some(value)
    }

    fn primary(&mut self) -> Option<Value> {
        match self.peek().cloned()? {
            Token::Number(n) => {
                self.at += 1;
                Some(Value::plain(n))
            }
            Token::Open => {
                self.at += 1;
                let inner = self.expression()?;
                self.eat(&Token::Close).then_some(inner)
            }
            Token::Ident(name) => {
                self.at += 1;
                let lowered = name.to_ascii_lowercase();
                if let Some(constant) = constant(&lowered) {
                    return Some(Value::plain(constant));
                }
                // A function must be called: `sqrt` alone is a word, not a value, which is what keeps an app search for "sqrt" from rendering as a calculation.
                if !self.eat(&Token::Open) {
                    return None;
                }
                let argument = self.expression()?.resolve();
                self.eat(&Token::Close).then_some(())?;
                apply(&lowered, argument).map(Value::plain)
            }
            _ => None,
        }
    }
}

fn constant(name: &str) -> Option<f64> {
    match name {
        "pi" | "π" => Some(std::f64::consts::PI),
        "e" => Some(std::f64::consts::E),
        "tau" => Some(std::f64::consts::TAU),
        _ => None,
    }
}

fn apply(name: &str, x: f64) -> Option<f64> {
    let value = match name {
        "sqrt" => x.sqrt(),
        "cbrt" => x.cbrt(),
        "abs" => x.abs(),
        "floor" => x.floor(),
        "ceil" => x.ceil(),
        "round" => x.round(),
        "ln" => x.ln(),
        "log" | "log10" => x.log10(),
        "log2" => x.log2(),
        "exp" => x.exp(),
        "sin" => x.sin(),
        "cos" => x.cos(),
        "tan" => x.tan(),
        "asin" => x.asin(),
        "acos" => x.acos(),
        "atan" => x.atan(),
        _ => return None,
    };
    value.is_finite().then_some(value)
}

/// Evaluates `input`, or `None` when it is not a complete arithmetic expression.
///
/// Being strict is the point: this runs on every launcher keystroke, and a query that is really an app name must fall through to the app search rather than showing a spurious number. So trailing tokens, unbalanced brackets, unknown words and non-finite results all fail rather than being salvaged.
pub fn evaluate(input: &str) -> Option<f64> {
    let tokens = tokenize(input)?;
    let mut parser = Parser { tokens, at: 0 };
    let value = parser.expression()?;
    // Anything left over means the input was not one expression — `2 + 3 foo` must not quietly evaluate to 5.
    if parser.at != parser.tokens.len() {
        return None;
    }
    let number = value.resolve();
    number.is_finite().then_some(number)
}

/// Renders a result the way a calculator does: no trailing `.0` on whole numbers, and rounded far enough to hide binary-float noise (`0.1 + 0.2` reads as `0.3`, not `0.30000000000000004`).
pub fn format(value: f64) -> String {
    let rounded = (value * 1e10).round() / 1e10;
    if rounded == rounded.trunc() && rounded.abs() < 1e15 {
        return format!("{}", rounded as i64);
    }
    let text = format!("{rounded}");
    if text.contains('.') {
        text.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        text
    }
}

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
            Answer::Number(value) => format(*value),
            Answer::Quantity { value, unit } => format!("{} {unit}", format(*value)),
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
    evaluate(input).map(Answer::Number)
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

    fn eval(input: &str) -> Option<String> {
        evaluate(input).map(format)
    }

    #[test]
    fn arithmetic_follows_precedence_and_brackets() {
        assert_eq!(eval("2+3*4").as_deref(), Some("14"));
        assert_eq!(eval("(2+3)*4").as_deref(), Some("20"));
        assert_eq!(eval("10 / 4").as_deref(), Some("2.5"));
        assert_eq!(eval("-3 + 5").as_deref(), Some("2"));
        assert_eq!(eval("2 * -3").as_deref(), Some("-6"));
        assert_eq!(eval("[1 + 2] * 3").as_deref(), Some("9"));
    }

    #[test]
    fn exponentiation_is_right_associative() {
        assert_eq!(eval("2^10").as_deref(), Some("1024"));
        assert_eq!(
            eval("2^3^2").as_deref(),
            Some("512"),
            "2^(3^2), not (2^3)^2 = 64"
        );
    }

    #[test]
    fn percentages_are_relative_to_what_they_are_added_to() {
        assert_eq!(eval("200 + 10%").as_deref(), Some("220"));
        assert_eq!(eval("200 - 10%").as_deref(), Some("180"));
        // Anywhere else a percentage is just a hundredth.
        assert_eq!(eval("50 * 10%").as_deref(), Some("5"));
        assert_eq!(eval("10%").as_deref(), Some("0.1"));
        assert_eq!(eval("200 * (1 + 10%)").as_deref(), Some("220"));
    }

    #[test]
    fn functions_and_constants_resolve() {
        assert_eq!(eval("sqrt(16)").as_deref(), Some("4"));
        assert_eq!(eval("round(2.6)").as_deref(), Some("3"));
        assert_eq!(eval("log2(1024)").as_deref(), Some("10"));
        assert_eq!(eval("2 * pi").as_deref(), eval("tau").as_deref());
        assert_eq!(eval("ln(e)").as_deref(), Some("1"));
        assert_eq!(eval("SQRT(9)").as_deref(), Some("3"), "case-insensitive");
    }

    #[test]
    fn results_hide_binary_float_noise() {
        assert_eq!(eval("0.1 + 0.2").as_deref(), Some("0.3"));
        assert_eq!(eval("1/3").as_deref(), Some("0.3333333333"));
        assert_eq!(format(4.0), "4", "a whole result has no trailing .0");
        assert_eq!(format(-0.5), "-0.5");
    }

    #[test]
    fn digit_grouping_and_exponents_parse() {
        assert_eq!(eval("1_000_000 / 4").as_deref(), Some("250000"));
        assert_eq!(eval("2e3 + 1").as_deref(), Some("2001"));
        assert_eq!(eval("1.5e-2").as_deref(), Some("0.015"));
    }

    #[test]
    fn an_app_name_is_not_a_calculation() {
        // The whole point: these run on every keystroke and must fall through to the app search.
        for query in [
            "firefox",
            "code",
            "2 + 3 firefox",
            "sqrt",
            "((1+2)",
            "1 +",
            "+",
            "",
            "   ",
            "log(",
        ] {
            assert!(
                evaluate(query).is_none(),
                "'{query}' must not evaluate to a number"
            );
        }
    }

    #[test]
    fn division_by_zero_is_not_an_answer() {
        assert!(evaluate("1/0").is_none());
        assert!(evaluate("5 / (3 - 3)").is_none());
    }

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
        assert!(evaluate("3 km in mi").is_none());
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
