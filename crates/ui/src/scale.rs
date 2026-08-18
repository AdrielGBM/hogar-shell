//! The sizes the shell draws itself at, named by size rather than by use.
//!
//! A token named for where it is used is a lie the first time someone needs it somewhere else: `CARD_RADIUS`
//! on a row invites either a wrong name or a fresh literal, and the shell collected sixty of the second kind.
//! A T-shirt size says only how big it is, which is the only thing a step on a scale knows about itself.
//!
//! **Both scales have a source, and it is the surface's own.** A corner radius and a base spacing are both
//! configured (`[shape] radius` / `[shape] spacing`, per bar, each falling back to the palette's), so each
//! scale is *derived* from whatever this surface resolved — set the radius to `0` and the whole shell squares
//! off together instead of only its panel corners; raise the spacing and every inset and gap opens with it,
//! not only the distance between two modules on a bar. That is why both are functions and neither is a
//! constant.
//!
//! **Every inset and every gap is on the scale, including inside a widget.** They ran 1, 2, 3, 5, 7 as often
//! as 4, 8, 16 across 84 padding literals and 110 gap literals, which looks at first like per-widget tuning
//! and is not: nothing here was ever measured against anything else, so the spread is what a number picked
//! afresh each time looks like. Six steps, and a value that is not one of them is a bug.

/// The corner radii, as fractions of the one this surface resolved.
///
/// Three steps because the shell has three: a panel and its peers, the cards and rows inside them, and the
/// small hover pills inside those. At the default palette's radius of 10 they come out 10 / 8 / 6, within a
/// pixel of the literals they replace — the point is not a new look, it is that `[shape] radius` and a
/// palette's own radius finally reach past the outermost corner.
///
/// The ratios are `telar::ThemeTokens`' own (`radius_lg` / `radius_md` / `radius_sm`), so a catalogue widget
/// dropped next to a card rounds by the same rule rather than by a second scale invented here. What stays the
/// shell's is the *base*: these resolve against the surface's configured radius (per-bar override → `[shape]
/// radius` → the theme), which a theme token cannot see.
///
/// **Resolve these once per build and capture the number.** A style closure runs on every paint, and the
/// lookup behind [`content_radius`](crate::panel::content_radius) is a context read, not a constant.
pub mod corner {
    /// A surface's own corner: a panel, a card that *is* the panel, a window.
    pub fn xl() -> f32 {
        crate::panel::content_radius()
    }

    /// What sits inside a panel — a card, a row, a button.
    pub fn md() -> f32 {
        xl() * 0.8
    }

    /// The small pressable things inside those: a hover pill on a menu row, an accent stripe.
    pub fn xs() -> f32 {
        xl() * 0.6
    }
}

/// A box's paint at a step of the [`corner`] scale.
///
/// These exist so the radius is resolved *once*, when the box is built, and captured. A style closure runs on
/// every paint and the lookup behind [`corner`] walks the surface context and can rebuild the palette — so
/// `move |_| RectStyle::filled(c, corner::md())` is a correctness trap that only shows up as a slow frame.
/// Taking the colour and handing back the closure makes the resolved-once version the easy one to write.
pub mod paint {
    use telar::{Color, Rect, RectStyle};

    /// A surface's own corner: a panel, a card that *is* the panel, a window.
    pub fn xl(color: Color) -> impl Fn(Rect) -> RectStyle + 'static {
        at(color, super::corner::xl())
    }

    /// What sits inside a panel — a card, a row, a button.
    pub fn md(color: Color) -> impl Fn(Rect) -> RectStyle + 'static {
        at(color, super::corner::md())
    }

    /// The small pressable things inside those: a hover pill on a menu row, an accent stripe.
    pub fn xs(color: Color) -> impl Fn(Rect) -> RectStyle + 'static {
        at(color, super::corner::xs())
    }

    fn at(color: Color, radius: f32) -> impl Fn(Rect) -> RectStyle + 'static {
        move |_| RectStyle::filled(color, radius)
    }
}

/// Every distance the shell puts between two things: an inset from an edge, a gap between siblings — as
/// fractions of the base spacing this surface resolved.
///
/// Six steps, doubling from [`md`] in both directions, with [`xs`] below for the hairline gaps a dense list
/// wants. Few enough that picking one is a decision and not a guess, and wide enough to cover the whole shell —
/// which the twelve distinct values it replaced did not do any better.
///
/// [`md`] is the base on purpose, where the radius base is the *largest* step: "how round is the biggest thing"
/// and "what is the default gap" are different questions. The four middle ratios are `telar::ThemeTokens`' own
/// (`spacing_sm` / `spacing_md` / `spacing_lg` / `spacing_xl`), so a catalogue widget dropped next to a shell
/// row is spaced by the same rule rather than by a second scale invented here; [`xs`] and [`xxl`] extend it
/// past what the theme names. What stays the shell's is the *base*: these resolve against the surface's
/// configured spacing (per-bar override → `[shape] spacing` → the theme), which a theme token cannot see.
///
/// **Resolve these once per build and capture the number**, the same rule the [`corner`] scale carries: the
/// lookup behind [`content_spacing`](crate::panel::content_spacing) is a context read, not a constant.
pub mod space {
    use crate::panel::content_spacing;

    /// Hairline. Between rows of a dense list, where the point is separation rather than air.
    pub fn xs() -> f32 {
        content_spacing() * 0.25
    }

    /// Tight. Inside a control — the space around a glyph in a small button.
    pub fn sm() -> f32 {
        content_spacing() * 0.5
    }

    /// The default. Between two things that belong together; a panel's rows, a chip's icon and its label.
    pub fn md() -> f32 {
        content_spacing()
    }

    /// Between two groups of things rather than two things.
    pub fn lg() -> f32 {
        content_spacing() * 1.5
    }

    /// A panel's own inset, and the space between its major sections.
    pub fn xl() -> f32 {
        content_spacing() * 2.0
    }

    /// The widest the shell goes: a card that is the only thing on its surface.
    pub fn xxl() -> f32 {
        content_spacing() * 3.0
    }
}

#[cfg(test)]
mod tests {
    /// The check that keeps this a scale rather than a one-off tidy-up.
    ///
    /// Nothing about writing `.gap(6.0)` looks wrong — it is how every one of the two hundred literals this
    /// replaced got written, one at a time, each perfectly reasonable on its own. Only the histogram showed it,
    /// and a histogram is not something anybody runs. So the rule is checked instead: a distance is a step on
    /// the scale, or it is zero, and there is no third option to drift into.
    #[test]
    fn every_distance_in_the_shell_is_a_step_on_the_scale() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(std::path::Path::parent)
            .expect("the workspace root is two levels above this crate")
            .to_path_buf();
        let mut offenders = Vec::new();
        let mut stack = vec![root.join("crates"), root.join("apps")];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = dir.read_dir() else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                // `.telar/build` is the transpiler's own output, not source.
                if path.is_dir() {
                    if path.file_name().and_then(|n| n.to_str()) != Some(".telar") {
                        stack.push(path);
                    }
                    continue;
                }
                let relative = path
                    .strip_prefix(&root)
                    .unwrap_or(&path)
                    .display()
                    .to_string();
                let is_source = path
                    .extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| e == "rs" || e == "rsx");
                if !is_source || relative == "crates/ui/src/scale.rs" {
                    continue;
                }
                let Ok(text) = std::fs::read_to_string(&path) else {
                    continue;
                };
                for (line, found) in bare_distances(&text) {
                    offenders.push(format!("{relative}:{line}: {found}"));
                }
            }
        }
        offenders.sort();
        assert!(
            offenders.is_empty(),
            "these name a distance the scale does not have — use `space::{{XS,SM,MD,LG,XL,XXL}}`, or `0.0` for \
             none at all:\n{}",
            offenders.join("\n")
        );
    }

    /// Every `.padding_*(N)` / `.gap(N)` / rsx `pad:N` carrying a bare number other than zero.
    ///
    /// Hand-rolled rather than a regex crate: this is the only place in the workspace that would need one, and
    /// a dependency for a single test is a worse trade than twenty lines of scanning.
    fn bare_distances(text: &str) -> Vec<(usize, String)> {
        const CALLS: [&str; 8] = [
            ".padding_all(",
            ".padding_top(",
            ".padding_right(",
            ".padding_bottom(",
            ".padding_left(",
            ".padding_horizontal(",
            ".padding_vertical(",
            ".gap(",
        ];
        const ATTRS: [&str; 4] = ["pad:", "pad_x:", "pad_y:", "gap:"];
        let mut found = Vec::new();
        for (index, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.starts_with("//") {
                continue;
            }
            for call in CALLS {
                let mut from = 0;
                while let Some(at) = line[from..].find(call) {
                    let start = from + at + call.len();
                    let arg: String = line[start..]
                        .chars()
                        .take_while(|c| c.is_ascii_digit() || *c == '.')
                        .collect();
                    if !arg.is_empty()
                        && arg.parse::<f32>().is_ok_and(|value| value != 0.0)
                        // A dimension, not a distance: `.gap(1.0)` is spacing, `.height(1.0)` is a hairline.
                        && line[start + arg.len()..].starts_with(')')
                    {
                        found.push((index + 1, format!("{call}{arg})")));
                    }
                    from = start;
                }
            }
            for attr in ATTRS {
                // Only at a word boundary: `pad:` also ends `keypad:`.
                let mut from = 0;
                while let Some(at) = line[from..].find(attr) {
                    let head = from + at;
                    let boundary = head == 0
                        || !line[..head]
                            .chars()
                            .next_back()
                            .is_some_and(|c| c.is_alphanumeric() || c == '_');
                    let start = head + attr.len();
                    let arg: String = line[start..]
                        .chars()
                        .take_while(|c| c.is_ascii_digit() || *c == '.')
                        .collect();
                    if boundary && !arg.is_empty() && arg.parse::<f32>().is_ok_and(|v| v != 0.0) {
                        found.push((index + 1, format!("{attr}{arg}")));
                    }
                    from = start.max(head + 1);
                }
            }
        }
        found
    }
}
