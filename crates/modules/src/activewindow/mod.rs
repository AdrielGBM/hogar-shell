//! What the focused window is called, and how much of that fits on a bar.

use services::hyprland::{self, ActiveWindow};

/// The text the chip shows: the window's title, or its class when it has no title.
///
/// Whole, however long it is. What fits is a question about pixels, and it is answered where the pixels are: the
/// chip yields width when its side of the bar is short of it and the label elides to what is left. Cutting to a
/// character count here answered a different question — it took the same 60 characters off a title with a whole
/// empty bar beside it as off one with none, and 60 "W" are twice the width of 60 "i".
pub fn label(window: &ActiveWindow) -> String {
    if window.title.is_empty() {
        window.class.clone()
    } else {
        window.title.clone()
    }
}

/// The label in `compact` mode: the app's class rather than the document title, which is what fits on a narrow
/// bar and stays stable while the user moves around inside one app.
pub fn compact_label(window: &ActiveWindow) -> String {
    window.class.clone()
}

/// The chip's leading (or trailing) visual: the focused application's own artwork at `size`, or an empty box
/// when its class has no installed icon.
///
/// A function rather than a bound widget because the view places it on one of two sides depending on
/// `inverted`, and a `widget` binding is a *value* — placeable once. Each `build` site calls this and gets its
/// own node, which is the rule the view DSL documents.
///
/// The air between icon and title is this slot's own margin rather than the row's gap: the slot is rebuilt for
/// every focused class, and a class that resolves to nothing has to cost nothing — a row gap would still be
/// spent on the empty box, indenting the title of every app with no installed icon.
pub fn icon_slot(
    class: &str,
    size: f32,
    inverted: bool,
) -> Result<Box<dyn telar::LayoutItem>, telar::LayoutError> {
    let Some(icon) = ui::icon::app_icon_view(class, size)? else {
        return Ok(telar::box_item(telar::Container::new(
            telar::LayoutStyle::new(),
            vec![],
        )?));
    };
    let style = telar::LayoutStyle::new().flex_shrink(0.0);
    let style = if inverted {
        style.margin_inline_start(ui::scale::space::MD)
    } else {
        style.margin_inline_end(ui::scale::space::MD)
    };
    Ok(telar::box_item(telar::Container::new(style, vec![icon])?))
}

/// Focuses the window the chip is showing — clicking the title takes you back to it, which is what the chip
/// looks like it should do.
pub fn focus_active() {
    hyprland::focus_active_window();
}

#[cfg(test)]
mod tests {
    use super::*;
    use telar::{
        AvailableSpace, Container, LayoutItem, LayoutStyle, compute_layout, reset_layout_runtime,
        track_layout,
    };

    /// The width a slot claims on a bar, margin and all.
    fn slot_width(class: &str, inverted: bool) -> f32 {
        reset_layout_runtime();
        let row = Container::new(
            LayoutStyle::new().flex_row(),
            vec![icon_slot(class, 16.0, inverted).expect("the slot builds")],
        )
        .expect("the row builds");
        let rect = track_layout(row.layout_node()).expect("a container registers its rect");
        compute_layout(
            row.layout_node(),
            AvailableSpace::MaxContent,
            AvailableSpace::Definite(32.0),
        )
        .expect("the row lays out");
        rect.get().width
    }

    /// An icon-bearing app is spaced off its title; an app with no installed icon costs nothing at all.
    ///
    /// The spacing is the slot's own margin rather than a gap on the row, and this is why: the slot is rebuilt
    /// for every focused class, so on the class that resolves to nothing it is an empty box — and a row gap
    /// would still be spent on it, indenting the title of every app the icon theme does not know.
    #[test]
    fn the_icon_carries_the_air_between_it_and_the_title_and_a_missing_one_carries_none() {
        // An absolute path is a reference `resolve_app_icon` takes as-is, so this needs no installed theme.
        let path = std::env::temp_dir().join("hyprshell-activewindow-icon-slot.svg");
        std::fs::write(
            &path,
            r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><rect width="16" height="16"/></svg>"#,
        )
        .expect("the fixture icon is written");
        let installed = path.to_str().expect("a utf-8 temp path");

        assert_eq!(
            slot_width(installed, false),
            16.0 + ui::scale::space::MD,
            "a leading icon has to hold the air between itself and the title"
        );
        assert_eq!(
            slot_width(installed, true),
            16.0 + ui::scale::space::MD,
            "and so does a trailing one, on its other side"
        );
        assert_eq!(
            slot_width("hyprshell-no-such-application", false),
            0.0,
            "a class the icon theme has never heard of must not indent the title it sits next to"
        );

        std::fs::remove_file(&path).ok();
    }

    fn window(title: &str, class: &str) -> ActiveWindow {
        ActiveWindow {
            title: title.to_string(),
            class: class.to_string(),
            address: "0x1".to_string(),
            handle: None,
        }
    }

    #[test]
    fn the_label_prefers_the_title_and_falls_back_to_the_class() {
        assert_eq!(label(&window("Inbox", "thunderbird")), "Inbox");
        assert_eq!(
            label(&window("", "thunderbird")),
            "thunderbird",
            "a window with no title still names itself"
        );
    }

    /// However long a title runs, it reaches the chip whole — the elide is what shortens it, at the width it
    /// actually has, and a count of characters cutting it first would take the room away before then.
    #[test]
    fn a_long_title_is_handed_over_untouched() {
        let long = "A very long window title that no bar has room for — hyprshell — Visual Studio Code";
        assert_eq!(label(&window(long, "code")), long);
        assert_eq!(
            label(&window("日本語のタイトル", "x")),
            "日本語のタイトル",
            "and a title is never cut mid-codepoint, because it is never cut here at all"
        );
    }

    #[test]
    fn compact_mode_shows_the_app_not_the_document() {
        assert_eq!(compact_label(&window("Docs — Firefox", "firefox")), "firefox");
    }
}
