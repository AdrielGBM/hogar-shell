use std::rc::Rc;

use telar::{
    AlignItems, BorderRadius, Children, Color, Container, Input, LayoutStyle, Reactive, RectStyle,
    RwSignal, ShapeStyle, StyledContainer, TextStyle, box_item, children, effect, signal,
};

use config::theme::{FontRole, NordTheme, hex, parse_hex};

use crate::descriptor::Built;
use crate::form::labelled::labelled;
use crate::form::recorder::record_field;

const CHANNELS: usize = 3;

/// A labelled colour: a text field holding a theme token or a `#hex`, which is the value itself, the theme's colours as swatches with the one `value` names lit (none while it holds a hex), and red, green and blue fields that write `#rrggbb`. Text that is neither writes nothing.
#[derive(telar::Props)]
pub struct SwatchRowProps {
    #[props(into, default)]
    pub label: Reactive<String>,
    #[props(default = signal(String::new()))]
    pub value: RwSignal<String>,
    /// The token names offered as swatches, each drawn in the colour the theme gives it.
    #[props(into, default = Rc::from(config::theme::ACCENTS))]
    pub tokens: Rc<[&'static str]>,
    #[props(into, default)]
    pub placeholder: Reactive<String>,
    /// Whether typed text may be written; the default takes any theme token or hex.
    #[props(default = Rc::new(is_colour))]
    pub accepts: Rc<dyn Fn(&str) -> bool>,
}

/// Whether `text` is a theme token or a `#rrggbb`/`#rrggbbaa` hex.
pub fn is_colour(text: &str) -> bool {
    NordTheme::is_token(text) || parse_hex(text).is_some()
}

/// What `typed` writes: lowercased, or nothing for an empty field; `None` for text `accepts` turns down.
pub fn parse_colour(typed: &str, accepts: &dyn Fn(&str) -> bool) -> Option<String> {
    let typed = typed.trim().to_ascii_lowercase();
    (typed.is_empty() || accepts(&typed)).then_some(typed)
}

fn resolved(value: &str, theme: &NordTheme) -> [u8; CHANNELS] {
    let color = match value {
        "" => theme.accent,
        _ => theme.paint_of(value),
    };
    let [r, g, b, _] = color.to_rgba8();
    [r, g, b]
}

pub fn swatch_row(props: SwatchRowProps, _children: Children) -> Built {
    let SwatchRowProps {
        label,
        value,
        tokens,
        placeholder,
        accepts,
    } = props;
    record_field(&value);
    let theme = telar::use_theme::<NordTheme>();
    let live = telar::Theme::<NordTheme>::default();
    let ColourBinding {
        typed,
        channels,
        selected,
    } = bind_colour(value, &tokens, accepts);

    let field = {
        let input = Input::declaring(
            typed,
            LayoutStyle::new().height(live.get().font(FontRole::Body) * 1.4),
            move |inherited: TextStyle| {
                inherited
                    .with_font_size(live.get().font(FontRole::Body))
                    .with_color(live.get().text)
            },
        )?
        .placeholder(placeholder.get());
        let radius = crate::scale::corner::md();
        StyledContainer::new(
            LayoutStyle::new()
                .flex_column()
                .padding_horizontal(crate::scale::space::md())
                .padding_vertical(crate::scale::space::sm()),
            move |_| {
                RectStyle::default()
                    .with_fill(live.get().base)
                    .with_radius(BorderRadius::all(radius))
            },
            children![input],
        )?
    };

    let picking = Rc::clone(&tokens);
    let swatches = telar::swatches(
        telar::SwatchesProps::props()
            .colors(tokens.iter().map(|token| theme.token(token)).collect())
            .names(tokens.iter().map(|token| token.to_string()).collect())
            .selected(selected)
            .on_select(Rc::new(move |at: u32| {
                if let Some(name) = picking.get(at as usize)
                    && value.peek() != *name
                {
                    value.set(name.to_string());
                }
            }))
            .build(),
        Children::default(),
    )?;

    let mut picker = Vec::new();
    for channel in channels {
        picker.push(telar::scrub_field(
            telar::ScrubFieldProps::props()
                .value(channel)
                .min(0.0)
                .max(255.0)
                .step(1.0)
                .format(Rc::new(|level: f32| format!("{}", level.round() as u8)))
                .parse(Rc::new(|typed: &str| {
                    typed
                        .trim()
                        .parse::<f32>()
                        .ok()
                        .filter(|level| level.is_finite())
                        .map(f32::round)
                }))
                .build(),
            Children::default(),
        )?);
    }
    let picker = Container::new(
        LayoutStyle::new()
            .flex_row()
            .align_items(AlignItems::CENTER)
            .gap(crate::scale::space::sm()),
        picker,
    )?;

    let rows = vec![box_item(field), box_item(swatches), box_item(picker)];
    let column = Container::new(
        LayoutStyle::new()
            .flex_column()
            .flex_grow(1.0)
            .gap(crate::scale::space::sm()),
        rows,
    )?;
    labelled(label, box_item(column))
}

/// The signals one colour row keeps in step with its `value`: the text typed, the lit swatch, and the three channels.
pub struct ColourBinding {
    pub typed: RwSignal<String>,
    pub channels: [RwSignal<f32>; CHANNELS],
    pub selected: RwSignal<Option<u32>>,
}

pub fn bind_colour(
    value: RwSignal<String>,
    tokens: &Rc<[&'static str]>,
    accepts: Rc<dyn Fn(&str) -> bool>,
) -> ColourBinding {
    let live = telar::Theme::<NordTheme>::default();
    let index_of = {
        let tokens = Rc::clone(tokens);
        move |name: &str| {
            tokens
                .iter()
                .position(|token| *token == name)
                .map(|at| at as u32)
        }
    };
    let selected = signal(index_of(&value.peek()));
    let followed = value.read_only();
    effect(move || {
        let at = index_of(&followed.get());
        if selected.peek() != at {
            selected.set(at);
        }
    });

    let typed = signal(value.peek());
    let judge = Rc::clone(&accepts);
    effect(move || {
        let now = followed.get();
        if typed.peek_with(|typed| parse_colour(typed, &*judge).as_deref() != Some(now.as_str())) {
            typed.set(now);
        }
    });
    effect(move || {
        if let Some(parsed) = parse_colour(&typed.get(), &*accepts)
            && value.peek() != parsed
        {
            value.set(parsed);
        }
    });

    let channels: [RwSignal<f32>; CHANNELS] = std::array::from_fn(|_| signal(0.0));
    effect(move || {
        let now = resolved(&followed.get(), &live.get());
        for (channel, level) in channels.iter().zip(now) {
            if channel.peek() != f32::from(level) {
                channel.set(f32::from(level));
            }
        }
    });
    effect(move || {
        let picked: [u8; CHANNELS] =
            std::array::from_fn(|at| channels[at].get().round().clamp(0.0, 255.0) as u8);
        if picked != resolved(&value.peek(), &live.get()) {
            value.set(hex(Color::rgb(
                f32::from(picked[0]) / 255.0,
                f32::from(picked[1]) / 255.0,
                f32::from(picked[2]) / 255.0,
            )));
        }
    });
    ColourBinding {
        typed,
        channels,
        selected,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bound(start: &str) -> (RwSignal<String>, ColourBinding) {
        bound_by(start, Rc::new(is_colour))
    }

    fn bound_by(
        start: &str,
        accepts: Rc<dyn Fn(&str) -> bool>,
    ) -> (RwSignal<String>, ColourBinding) {
        telar::reset_runtime();
        telar::set_theme(NordTheme::new());
        let value = signal(start.to_string());
        let tokens: Rc<[&'static str]> = Rc::from(["surface", "accent", "blue"]);
        let binding = bind_colour(value, &tokens, accepts);
        (value, binding)
    }

    #[test]
    fn typing_a_token_writes_it_and_lights_its_swatch() {
        let (value, binding) = bound("surface");
        binding.typed.set("Accent".to_string());
        assert_eq!(value.peek(), "accent");
        assert_eq!(binding.selected.peek(), Some(1));
    }

    #[test]
    fn a_written_hex_lights_no_swatch() {
        let (value, binding) = bound("blue");
        binding.typed.set("#12ab34".to_string());
        assert_eq!(value.peek(), "#12ab34");
        assert_eq!(binding.selected.peek(), None);
    }

    #[test]
    fn picking_a_hex_writes_rrggbb() {
        let (value, binding) = bound("");
        for (channel, level) in binding.channels.iter().zip([255.0, 136.0, 0.0]) {
            channel.set(level);
        }
        assert_eq!(value.peek(), "#ff8800");
        assert_eq!(binding.typed.peek(), "#ff8800");
        assert_eq!(binding.selected.peek(), None);
    }

    #[test]
    fn invalid_text_writes_nothing() {
        let (value, binding) = bound("blue");
        for text in [
            "#12",
            "#zzzzzz",
            "bluee",
            "#12345",
            "bad",
            "add",
            "decade",
            "not a colour",
        ] {
            binding.typed.set(text.to_string());
            assert_eq!(value.peek(), "blue", "{text:?} wrote");
            assert_eq!(binding.selected.peek(), Some(2));
        }
    }

    #[test]
    fn a_hex_needs_its_hash_and_six_or_eight_digits() {
        assert!(is_colour("#ff8800"));
        assert!(is_colour("#ff880080"));
        for text in ["add", "decade", "ff8800", "#12345", "#ff88000"] {
            assert!(!is_colour(text), "{text:?} is a colour");
        }
    }

    #[test]
    fn a_row_writes_only_what_it_accepts() {
        let (value, binding) = bound_by("blue", Rc::new(NordTheme::has_accent));
        binding.typed.set("surface".to_string());
        assert_eq!(value.peek(), "blue");
        binding.typed.set("#ff880080".to_string());
        assert_eq!(value.peek(), "blue");
        binding.typed.set("#ff8800".to_string());
        assert_eq!(value.peek(), "#ff8800");
    }

    #[test]
    fn an_outside_change_replaces_the_text() {
        let (value, binding) = bound("blue");
        value.set("#000000".to_string());
        assert_eq!(binding.typed.peek(), "#000000");
        assert_eq!(binding.channels[0].peek(), 0.0);
    }
}
