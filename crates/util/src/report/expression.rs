//! What an expression's [`ErrorCode`] says, as a [`Message`] of this crate's `expression.*` entries, keyed by [`ErrorCode::key`]: one wording for every place the shell shows an expression's mistake, in the language it is shown in.
//!
//! The types, the things found and the brackets an error names are messages of their own, so a Spanish sentence says "un número" where the English one says "a number". A host's own failure ([`ErrorCode::Host`]) is in the host's catalogue, which this crate cannot see: here it is the host's English, and a host that translates its failures reads them itself before asking for the rest.

use telar_expression::{Closing, Context, ErrorCode, Found, Pattern, Type, ValueKind};

use super::Message;

impl Message {
    /// What `code` says.
    pub fn expression(code: &ErrorCode) -> Message {
        match code {
            ErrorCode::InvalidNumber { written } => {
                message!("expression.invalid_number", written = written)
            }
            ErrorCode::NumberTooLarge { written } => {
                message!("expression.number_too_large", written = written)
            }
            ErrorCode::ReferenceNeedsName => message!("expression.reference_needs_name"),
            ErrorCode::UnclosedText { quote } => {
                message!("expression.unclosed_text", quote = quote)
            }
            ErrorCode::UnknownEscape => message!("expression.unknown_escape"),
            ErrorCode::InvalidColor { written } => {
                message!("expression.invalid_color", written = written)
            }
            ErrorCode::LoneOperator { found, suggestion } => {
                let advice = match found {
                    '=' => message!("expression.advice.compare", suggestion = suggestion),
                    _ => message!("expression.advice.write", suggestion = suggestion),
                };
                message!("expression.lone_operator", found = found, advice = advice)
            }
            ErrorCode::UnexpectedCharacter { found } => {
                message!("expression.unexpected_character", found = found)
            }
            ErrorCode::EmptyExpression => message!("expression.empty_expression"),
            ErrorCode::TrailingToken { found } => {
                message!("expression.trailing_token", found = thing(found))
            }
            ErrorCode::ExpectedClosing {
                expected,
                closing,
                found,
            } => message!(
                "expression.expected_closing",
                expected = expected,
                closing = closing_of(*closing),
                found = found_clause(found)
            ),
            ErrorCode::ExpectedValue { found } => {
                message!("expression.expected_value", found = found_clause(found))
            }
            ErrorCode::TooDeep => message!("expression.too_deep"),
            ErrorCode::IfNeedsCall => message!("expression.if_needs_call"),
            ErrorCode::IfArity => message!("expression.if_arity"),
            ErrorCode::ExpectedType { expected, found } => message!(
                "expression.expected_type",
                expected = Message::type_name(expected),
                found = Message::type_name(found)
            ),
            ErrorCode::OperandType {
                context,
                expected,
                found,
            } => message!(
                "expression.operand_type",
                context = context_of(context),
                expected = Message::type_name(expected),
                found = Message::type_name(found)
            ),
            ErrorCode::BranchMismatch { first, second } => message!(
                "expression.branch_mismatch",
                first = Message::type_name(first),
                second = Message::type_name(second)
            ),
            ErrorCode::ListItemMismatch { item, list } => message!(
                "expression.list_item_mismatch",
                item = Message::type_name(item),
                list = Message::type_name(list)
            ),
            ErrorCode::NameIsFunction { name } => {
                message!("expression.name_is_function", name = name)
            }
            ErrorCode::UnknownName { name } => message!("expression.unknown_name", name = name),
            ErrorCode::NameIsConstant { name } => {
                message!("expression.name_is_constant", name = name)
            }
            ErrorCode::UnknownFunction { name } => {
                message!("expression.unknown_function", name = name)
            }
            ErrorCode::CompareMismatch {
                operator,
                left,
                right,
            } => message!(
                "expression.compare_mismatch",
                operator = operator,
                left = Message::type_name(left),
                right = Message::type_name(right)
            ),
            ErrorCode::NotOrderable { operator, found } => message!(
                "expression.not_orderable",
                operator = operator,
                found = Message::type_name(found)
            ),
            ErrorCode::NoMatchingForm { name, given, forms } => message!(
                "expression.no_matching_form",
                name = name,
                given = joined(given.iter().map(Message::type_name)),
                forms = either(
                    forms
                        .iter()
                        .map(|form| Message::verbatim(format!("{name}{form}")))
                )
            ),
            ErrorCode::ArgumentType {
                name,
                index,
                expected,
                found,
            } => message!(
                "expression.argument_type",
                number = index + 1,
                name = name,
                expected = pattern(expected),
                found = Message::type_name(found)
            ),
            ErrorCode::ArgumentCount {
                name,
                takes,
                at_least,
                given,
            } => {
                let takes = match at_least {
                    true => message!("expression.takes_at_least", count = takes),
                    false => message!("expression.takes", count = takes),
                };
                message!(
                    "expression.argument_count",
                    name = name,
                    takes = takes,
                    given = message!("expression.given", count = given)
                )
            }
            ErrorCode::ReferenceType {
                reference,
                declared,
                found,
            } => message!(
                "expression.reference_type",
                reference = reference,
                declared = Message::type_name(declared),
                found = Message::type_name(found)
            ),
            ErrorCode::ReferenceNotFinite {
                reference,
                declared,
            } => message!(
                "expression.reference_not_finite",
                reference = reference,
                declared = Message::type_name(declared)
            ),
            ErrorCode::ResultType {
                name,
                declared,
                found,
            } => message!(
                "expression.result_type",
                name = name,
                declared = Message::type_name(declared),
                found = Message::type_name(found)
            ),
            ErrorCode::NoFiniteResult { name } => {
                message!("expression.no_finite_result", name = name)
            }
            ErrorCode::DivisionByZero => message!("expression.division_by_zero"),
            ErrorCode::NoRealResult => message!("expression.no_real_result"),
            ErrorCode::ResultTooLarge => message!("expression.result_too_large"),
            ErrorCode::TextTooLong { limit } => {
                message!("expression.text_too_long", limit = limit)
            }
            ErrorCode::MistypedValue => message!("expression.mistyped_value"),
            ErrorCode::MissingArgument { index } => {
                message!("expression.missing_argument", number = index + 1)
            }
            ErrorCode::ExpectedArgument { expected } => {
                message!("expression.expected_argument", expected = kind(*expected))
            }
            ErrorCode::MalformedFormat => message!("expression.malformed_format"),
            ErrorCode::FormatMissingValue { index, given } => message!(
                "expression.format_missing_value",
                index = index,
                given = message!("expression.given", count = given)
            ),
            ErrorCode::DecimalsNotWhole => message!("expression.decimals_not_whole"),
            ErrorCode::PositionNotWhole => message!("expression.position_not_whole"),
            ErrorCode::PositionOutside { position, len } => message!(
                "expression.position_outside",
                position = position,
                len = len
            ),
            ErrorCode::Host(error) => Message::verbatim(error.message.clone()),
            other => Message::verbatim(other.to_string()),
        }
    }

    /// A type, as a sentence names it: "number", "list of text".
    pub fn type_name(ty: &Type) -> Message {
        match ty {
            Type::Number => message!("expression.type.number"),
            Type::Text => message!("expression.type.text"),
            Type::Bool => message!("expression.type.bool"),
            Type::Color => message!("expression.type.colour"),
            Type::List(item) if **item == Type::Never => message!("expression.type.empty_list"),
            Type::List(item) => message!("expression.type.list", item = Message::type_name(item)),
            Type::Never => message!("expression.type.nothing"),
        }
    }
}

fn pattern(pattern: &Pattern) -> Message {
    match pattern {
        Pattern::Exact(ty) => Message::type_name(ty),
        Pattern::Any => message!("expression.type.any"),
        Pattern::Var(_) => Message::verbatim(pattern.to_string()),
        Pattern::List(item) => message!("expression.type.list", item = self::pattern(item)),
    }
}

/// What was found, as the subject of a sentence: "a number", "`foo`".
fn thing(found: &Found) -> Message {
    match found {
        Found::Number => message!("expression.found.number"),
        Found::Text => message!("expression.found.text"),
        Found::Color => message!("expression.found.colour"),
        Found::Name(name) => message!("expression.found.name", name = name),
        Found::Reference(dotted) => message!("expression.found.reference", name = dotted),
        Found::Symbol(symbol) => message!("expression.found.symbol", symbol = symbol),
        Found::End => message!("expression.found.end"),
    }
}

/// What was found, as the clause after "but": "found a number", "the expression ended".
fn found_clause(found: &Found) -> Message {
    match found {
        Found::End => message!("expression.ended"),
        other => message!("expression.found_clause", found = thing(other)),
    }
}

fn closing_of(closing: Closing) -> Message {
    match closing {
        Closing::List => message!("expression.closing.list"),
        Closing::Group => message!("expression.closing.group"),
        Closing::Call => message!("expression.closing.call"),
    }
}

fn context_of(context: &Context) -> Message {
    match context {
        Context::Operator(operator) => {
            message!("expression.context.operator", operator = operator)
        }
        Context::AddAfterText => message!("expression.context.add_after_text"),
        Context::IfCondition => message!("expression.context.if_condition"),
    }
}

fn kind(kind: ValueKind) -> Message {
    match kind {
        ValueKind::Number => message!("expression.kind.number"),
        ValueKind::Text => message!("expression.kind.text"),
        ValueKind::Bool => message!("expression.kind.bool"),
        ValueKind::Color => message!("expression.kind.colour"),
        ValueKind::List => message!("expression.kind.list"),
    }
}

/// `a, b, c`.
fn joined(items: impl DoubleEndedIterator<Item = Message>) -> Message {
    fold(items, |first, rest| {
        message!("expression.series", first = first, rest = rest)
    })
}

/// `a or b or c`.
fn either(items: impl DoubleEndedIterator<Item = Message>) -> Message {
    fold(items, |first, rest| {
        message!("expression.either", first = first, rest = rest)
    })
}

fn fold(
    items: impl DoubleEndedIterator<Item = Message>,
    pair: impl Fn(Message, Message) -> Message,
) -> Message {
    let mut items = items.rev();
    let Some(last) = items.next() else {
        return Message::verbatim("");
    };
    items.fold(last, |rest, first| pair(first, rest))
}

#[cfg(test)]
mod tests {
    use telar_expression::{HostError, Reference, Signature};

    use super::*;
    use crate::report::untranslated;

    /// One of every failure the language reports today: the enum is `non_exhaustive`, so this list is what says a new one has been added without words of its own.
    fn every_code() -> Vec<ErrorCode> {
        let number = || Type::Number;
        vec![
            ErrorCode::InvalidNumber {
                written: "1e".into(),
            },
            ErrorCode::NumberTooLarge {
                written: "1e999".into(),
            },
            ErrorCode::ReferenceNeedsName,
            ErrorCode::UnclosedText { quote: '"' },
            ErrorCode::UnknownEscape,
            ErrorCode::InvalidColor {
                written: "#zz".into(),
            },
            ErrorCode::LoneOperator {
                found: '=',
                suggestion: "==",
            },
            ErrorCode::LoneOperator {
                found: '&',
                suggestion: "&&",
            },
            ErrorCode::UnexpectedCharacter { found: '@' },
            ErrorCode::EmptyExpression,
            ErrorCode::TrailingToken {
                found: Found::Number,
            },
            ErrorCode::ExpectedClosing {
                expected: ")",
                closing: Closing::Call,
                found: Found::End,
            },
            ErrorCode::ExpectedClosing {
                expected: "}",
                closing: Closing::List,
                found: Found::Symbol("]"),
            },
            ErrorCode::ExpectedClosing {
                expected: ")",
                closing: Closing::Group,
                found: Found::Text,
            },
            ErrorCode::ExpectedValue {
                found: Found::Color,
            },
            ErrorCode::ExpectedValue {
                found: Found::Name("then".into()),
            },
            ErrorCode::ExpectedValue {
                found: Found::Reference("battery.level".into()),
            },
            ErrorCode::TooDeep,
            ErrorCode::IfNeedsCall,
            ErrorCode::IfArity,
            ErrorCode::ExpectedType {
                expected: Type::Bool,
                found: Type::list(Type::Text),
            },
            ErrorCode::OperandType {
                context: Context::Operator("+"),
                expected: number(),
                found: Type::Color,
            },
            ErrorCode::OperandType {
                context: Context::AddAfterText,
                expected: Type::Text,
                found: Type::list(Type::Never),
            },
            ErrorCode::OperandType {
                context: Context::IfCondition,
                expected: Type::Bool,
                found: Type::Never,
            },
            ErrorCode::BranchMismatch {
                first: number(),
                second: Type::Text,
            },
            ErrorCode::ListItemMismatch {
                item: Type::Text,
                list: Type::list(number()),
            },
            ErrorCode::NameIsFunction { name: "len".into() },
            ErrorCode::UnknownName {
                name: "firefox".into(),
            },
            ErrorCode::NameIsConstant { name: "pi".into() },
            ErrorCode::UnknownFunction { name: "lne".into() },
            ErrorCode::CompareMismatch {
                operator: "==",
                left: number(),
                right: Type::Text,
            },
            ErrorCode::NotOrderable {
                operator: "<",
                found: Type::Color,
            },
            ErrorCode::NoMatchingForm {
                name: "mix".into(),
                given: vec![number(), Type::Text],
                forms: vec![
                    Signature::new([Pattern::Exact(Type::Color), Pattern::Any], Type::Color),
                    Signature::new([Pattern::list(Pattern::Var(0))], Pattern::Var(0)),
                ],
            },
            ErrorCode::ArgumentType {
                name: "alpha".into(),
                index: 0,
                expected: Pattern::Exact(Type::Color),
                found: number(),
            },
            ErrorCode::ArgumentCount {
                name: "len".into(),
                takes: 1,
                at_least: false,
                given: 2,
            },
            ErrorCode::ArgumentCount {
                name: "fmt".into(),
                takes: 2,
                at_least: true,
                given: 1,
            },
            ErrorCode::ReferenceType {
                reference: Reference::new("battery", ["level"]),
                declared: number(),
                found: Type::Text,
            },
            ErrorCode::ReferenceNotFinite {
                reference: Reference::new("cpu", ["usage"]),
                declared: number(),
            },
            ErrorCode::ResultType {
                name: "df".into(),
                declared: Type::Text,
                found: number(),
            },
            ErrorCode::NoFiniteResult {
                name: "sqrt".into(),
            },
            ErrorCode::DivisionByZero,
            ErrorCode::NoRealResult,
            ErrorCode::ResultTooLarge,
            ErrorCode::TextTooLong { limit: 65536 },
            ErrorCode::MistypedValue,
            ErrorCode::MissingArgument { index: 1 },
            ErrorCode::ExpectedArgument {
                expected: ValueKind::Number,
            },
            ErrorCode::ExpectedArgument {
                expected: ValueKind::Text,
            },
            ErrorCode::ExpectedArgument {
                expected: ValueKind::Bool,
            },
            ErrorCode::ExpectedArgument {
                expected: ValueKind::Color,
            },
            ErrorCode::ExpectedArgument {
                expected: ValueKind::List,
            },
            ErrorCode::MalformedFormat,
            ErrorCode::FormatMissingValue { index: 2, given: 1 },
            ErrorCode::DecimalsNotWhole,
            ErrorCode::PositionNotWhole,
            ErrorCode::PositionOutside {
                position: 4,
                len: 2,
            },
        ]
    }

    #[test]
    fn every_failure_of_the_language_has_words_in_every_language_the_shell_speaks() {
        for code in every_code() {
            let message = Message::expression(&code);
            for locale in ["en", "es"] {
                assert!(
                    message.is_translated_in(locale),
                    "`{}` has no words of its own in {locale}: {message:?}",
                    code.key()
                );
            }
        }
        assert_eq!(
            untranslated(&crate::__rsx_i18n::CATALOG, &["expression."]),
            Vec::<String>::new()
        );
    }

    /// The catalogue's English is the language's own, word for word, so the command line says what it always said.
    #[test]
    fn the_english_is_what_the_language_itself_says() {
        for code in every_code() {
            assert_eq!(
                Message::expression(&code).english(),
                code.to_string(),
                "`{}`",
                code.key()
            );
        }
    }

    #[test]
    fn a_failure_reads_in_the_language_it_is_shown_in_down_to_the_types_it_names() {
        let code = ErrorCode::ExpectedType {
            expected: Type::Bool,
            found: Type::list(Type::Number),
        };
        let message = Message::expression(&code);
        assert_eq!(
            message.english(),
            "expected bool, but this gives list of number"
        );
        assert_eq!(
            message.render_in("es"),
            "se esperaba booleano, pero esto da lista de número"
        );
        assert_eq!(message.key(), Some("expression.expected_type"));
    }

    #[test]
    fn a_host_s_failure_is_its_english_until_the_host_reads_it() {
        let host = HostError::new(
            "expression.no_reading",
            "`$battery.level` has no reading yet",
        );
        assert_eq!(
            Message::expression(&ErrorCode::Host(host)).render_in("es"),
            "`$battery.level` has no reading yet"
        );
    }

    #[test]
    fn a_message_travels_through_a_host_error_and_back() {
        let catalog = &crate::__rsx_i18n::CATALOG;
        let message = message!("expression.position_outside", position = 4, len = 2);
        let error = message.host_error();
        assert_eq!(error.key, "expression.position_outside");
        assert_eq!(error.args, ["4", "2"]);
        assert_eq!(error.message, "position 4 is outside a list of 2");
        let back = Message::from_host_error(catalog, &error);
        assert_eq!(back, message);
        assert_eq!(
            back.render_in("es"),
            "la posición 4 está fuera de una lista de 2"
        );
        let foreign = HostError::new("elsewhere.unknown", "said elsewhere");
        assert_eq!(
            Message::from_host_error(catalog, &foreign).render_in("es"),
            "said elsewhere"
        );
    }
}
