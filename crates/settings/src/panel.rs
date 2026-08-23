use std::path::PathBuf;
use std::sync::Arc;
use ui::scale::{corner, paint, space};

use telar::{
    AlignItems, Container, Input, JustifyContent, LayoutError, LayoutItem, LayoutStyle,
    ReactiveList, RectStyle, RwSignal, SizeDimension, StyledContainer, Text, box_item, signal,
    use_theme,
};

use crate::form::*;
use config::Config;
use config::theme::{FontRole, NordTheme};
use ui::icon::icon_view;
use ui::module::{icon_px, module_fg};
use util::state::kept;

/// This application's module id, which is also the id its surface is registered under — what a reload needs to
/// know to leave the window that caused it alone (see [`surfaces::shell::authored_change`]).
pub(crate) const MODULE: &str = "settings";

/// The nav pane's width, the gap to the forms beside it, and how wide the search box is. Wide enough for the
/// longest page label in either catalogue without wrapping, which is what stops the nav reflowing as the
/// language changes under it.
const NAV_WIDTH: f32 = 190.0;
const NAV_GAP: f32 = 24.0;
const SEARCH_WIDTH: f32 = 220.0;

/// The bar chip: a gear that opens the settings panel.
pub fn settings_chip() -> Result<Box<dyn LayoutItem>, LayoutError> {
    let fg = module_fg();
    icon_view(|| "settings".to_string(), move || fg.get(), icon_px())
}

/// The settings panel: an in-shell editor for `config.toml`. Each section's fields are seeded from the current
/// file, and a form applies itself a moment after the last edit — its Save button is the same write without the
/// wait (see [`live_apply`]). Both go through [`Config::save_section`] (format-preserving), which the running
/// shell hot-reloads and applies live; Revert (in the header) puts the file back to how it was when the window
/// opened.
pub fn settings_panel() -> Result<Box<dyn LayoutItem>, LayoutError> {
    let theme = use_theme::<NordTheme>();
    let path = Arc::new(Config::default_path());
    let config = Arc::new(Config::load_or_default(&path));
    services::locale::attach(config.language());
    // Which file the forms on this window edit, said once here rather than handed to each of them — see
    // `form::source`.
    crate::form::set_source((*path).clone());

    // The selection and the query are the whole state of the application, and they belong to the *surface*
    // rather than to this build of it: an edit made from another window rebuilds this one, and a settings
    // application that jumped back to its first page every time the config changed would be unusable.
    let selected = kept("settings.page", || signal(0usize));
    let query = kept("settings.query", || signal(String::new()));
    // Bumped when the file stops being what the forms are showing — which is Revert, and only Revert. A form
    // applying itself writes what it already holds, and re-seeding *that* is how the field being typed into
    // loses its caret.
    let reseed = kept("settings.reseed", || signal(0u64));
    // What Revert restores: the file as it was when the *user* opened this window, not as it was a reload ago.
    remember_opened(path.as_path());

    let body = Container::new(
        LayoutStyle::new()
            .flex_row()
            .gap(NAV_GAP)
            .width(SizeDimension::Percent(1.0)),
        vec![
            nav_pane(selected.clone(), query.read_only(), theme)?,
            page_stack(
                selected.read_only(),
                query.read_only(),
                reseed.read_only(),
                config,
                Arc::clone(&path),
                theme,
            )?,
        ],
    )?;

    let panel = Container::new(
        LayoutStyle::new()
            .flex_column()
            .gap(space::xl())
            .width(SizeDimension::Percent(1.0)),
        vec![header(query, reseed, path, theme)?, Box::new(body)],
    )?;
    Ok(Box::new(panel))
}

/// Forgets the Revert snapshot. Called when the panel is closed for real, so the next window reverts to the
/// file as *it* found it rather than to something a previous session opened against.
pub fn forget_panel_state() {
    forget_opened();
}

/// The title, the search box that reaches every page, and Revert.
fn header(
    query: RwSignal<String>,
    reseed: RwSignal<u64>,
    path: Arc<PathBuf>,
    theme: NordTheme,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let title = Text::auto(
        || telar::t!("settings.title"),
        LayoutStyle::new().flex_grow(1.0),
        move || {
            theme
                .text_style(FontRole::Title, theme.text)
                .with_weight(700)
        },
    )?;

    let input = Input::new(
        query,
        LayoutStyle::new()
            .flex_grow(1.0)
            .height(theme.font(FontRole::Body) * 1.6),
        move || theme.text_style(FontRole::Body, theme.text),
    )?
    .placeholder(telar::t!("settings.search"));
    let boxed = StyledContainer::new(
        LayoutStyle::new()
            .width(SEARCH_WIDTH)
            .padding_horizontal(space::md())
            .padding_vertical(space::sm()),
        paint::md(theme.base),
        vec![box_item(input)],
    )?;

    // Not a `save_button`: that one now records the form it belongs to, and Revert belongs to no form.
    let revert_ink = theme.red;
    let revert = StyledContainer::new(
        LayoutStyle::new()
            .padding_horizontal(space::lg())
            .padding_vertical(space::md())
            .flex_shrink(0.0)
            .justify_content(JustifyContent::CENTER),
        paint::md(theme.base),
        vec![box_item(Text::auto(
            || telar::t!("settings.revert"),
            LayoutStyle::new(),
            move || {
                theme
                    .text_style(FontRole::Caption, revert_ink)
                    .with_weight(700)
            },
        )?)],
    )?
    .hover_style(paint::md(theme.overlay))
    .on_press(move || {
        revert_to_opened(path.as_path());
        // Straight away rather than waiting for the reload the write triggers: Revert is the one moment the
        // forms on screen are known to be wrong, and it is the user asking to see the file instead.
        reseed.set(reseed.peek() + 1);
    });

    Ok(Box::new(Container::new(
        LayoutStyle::new()
            .flex_row()
            .align_items(AlignItems::CENTER)
            .gap(space::lg())
            .width(SizeDimension::Percent(1.0)),
        vec![Box::new(title), Box::new(boxed), Box::new(revert)],
    )?))
}

/// The nav: one row per page, the selected one filled, the ones a search excludes dimmed.
fn nav_pane(
    selected: RwSignal<usize>,
    query: telar::ReadSignal<String>,
    theme: NordTheme,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let mut rows: Vec<Box<dyn LayoutItem>> = Vec::with_capacity(crate::pages::PAGES.len());
    for (index, page) in crate::pages::PAGES.iter().enumerate() {
        rows.push(nav_row(
            index,
            page,
            selected.clone(),
            query.clone(),
            theme,
        )?);
    }
    Ok(Box::new(Container::new(
        LayoutStyle::new()
            .flex_column()
            .gap(space::xs())
            .width(NAV_WIDTH)
            .flex_shrink(0.0),
        rows,
    )?))
}

fn nav_row(
    index: usize,
    page: &'static crate::pages::Page,
    selected: RwSignal<usize>,
    query: telar::ReadSignal<String>,
    theme: NordTheme,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let on_fg = theme.accent.most_readable(&[theme.text, theme.base]);
    // Read out of the two signals in one place: a row's colour depends on both, and the ink has to match
    // whatever fill the same frame drew.
    let ink = {
        let (selected, query) = (selected.read_only(), query.clone());
        move || {
            if selected.get() == index {
                on_fg
            } else if page.matches(&query.get()) {
                theme.text
            } else {
                theme.muted
            }
        }
    };
    let label_ink = ink.clone();
    let label = Text::auto(
        move || crate::pages::label("settings.page", page.label),
        LayoutStyle::new().flex_grow(1.0),
        move || theme.text_style(FontRole::Body, label_ink()),
    )?;
    let glyph = icon_view(
        move || page.icon.to_string(),
        ink,
        theme.font(FontRole::Body) * 1.15,
    )?;

    let fill = selected.read_only();
    let press = selected;
    let rounded = corner::md();
    let row = StyledContainer::new(
        LayoutStyle::new()
            .flex_row()
            .align_items(AlignItems::CENTER)
            .gap(space::lg())
            .padding_horizontal(space::lg())
            .padding_vertical(space::md())
            .width(SizeDimension::Percent(1.0)),
        move |_| {
            if fill.get() == index {
                RectStyle::filled(theme.accent, rounded)
            } else {
                RectStyle::default()
            }
        },
        vec![glyph, Box::new(label)],
    )?
    .hover_style(paint::md(theme.surface))
    .on_press(move || press.set(index));
    Ok(Box::new(row))
}

/// The forms for the selected page, narrowed by the search.
///
/// A keyed list rather than a rebuilt column: the key is the page *and* the query, because narrowing a page
/// changes which forms are on it, and a list keyed on the page alone would keep showing the ones it had.
fn page_stack(
    selected: telar::ReadSignal<usize>,
    query: telar::ReadSignal<String>,
    reseed: telar::ReadSignal<u64>,
    config: Arc<Config>,
    path: Arc<PathBuf>,
    theme: NordTheme,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let height = config.settings_page_height();
    // The nav is outside this scroll area on purpose: a nav pane that scrolls away with the page it selects is
    // a list of links you have to scroll back up to use.
    let scroll = telar::LayoutScrollArea::new_kept(
        "settings.scroll",
        LayoutStyle::new()
            .flex_column()
            .flex_grow(1.0)
            // `min_width(0)` against flexbox's `auto` default: a form's rows are `width: 100%` of whatever they
            // are given, and a flex item that may not shrink below its content asks for the widest row it has,
            // which is how the page area ends up wider than the surface it is in.
            .min_width(0.0)
            .height(height),
        move |viewport| {
            crate::form::set_viewport(viewport.clone());

            // A page is *replaced*, not resized: three screens down the Appearance page is not a place to be
            // dropped into Network, and neither is three screens down the forms a search has just narrowed
            // away. The scroll area puts a too-short page back in range on its own; only this knows that what
            // is in the viewport is now a different thing rather than the same thing resized.
            //
            // Not on the first run, which is the effect being seeded rather than the user choosing a page —
            // and on a rebuild that seeding run is exactly what would throw away the position being kept.
            let (page, search) = (selected.clone(), query.clone());
            let seeded = std::cell::Cell::new(false);
            let follow_page = telar::effect(move || {
                page.get();
                search.get();
                if seeded.replace(true) {
                    viewport.scroll_to_top();
                }
            });
            let page_area = build_page_area(selected, query, reseed, config, path, theme)?;
            Ok(Box::new(telar::Holding::new(page_area, vec![follow_page])))
        },
    )?;
    Ok(Box::new(scroll))
}

/// The forms themselves: the sections the current page and search leave visible, each seeded from the file.
fn build_page_area(
    selected: telar::ReadSignal<usize>,
    query: telar::ReadSignal<String>,
    reseed: telar::ReadSignal<u64>,
    config: Arc<Config>,
    path: Arc<PathBuf>,
    _theme: NordTheme,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    // The snapshot the window opened with decides how tall the page area is, and nothing else: every form is
    // seeded from the file at the moment it is *built*, so a form rebuilt is a form re-seeded.
    let (_opened_with, _path) = (config, path);
    let source = move || {
        // All read out first: `visible` translates labels, which reads the locale signal, and a nested read
        // inside another signal's borrow is the re-entrant panic that only fires when the widget is built.
        let index = selected.get();
        let text = query.get();
        let at = reseed.get();
        crate::pages::visible(index, &text)
            .into_iter()
            .map(|section| (text.clone(), at, section))
            .collect()
    };
    // Each form re-reads the file for itself (`form::source`), which is what makes a form rebuilt a form
    // re-seeded — the panel only says which file that is, once, before any of them is built.
    let build =
        move |(_, _, section): (String, u64, &'static crate::pages::Section)| (section.build)();
    Ok(Box::new(ReactiveList::with_style(
        LayoutStyle::new()
            .flex_column()
            .gap(space::xxl())
            .width(SizeDimension::Percent(1.0)),
        source,
        // Keyed on the query and the re-seed as well as the form: narrowing changes which forms are here, and
        // Revert changes what they should be showing. Anything not in the key is a form the user may be
        // typing into, which must survive its own applied changes.
        |(query, at, section): &(String, u64, &'static crate::pages::Section)| {
            (query.clone(), *at, section.label)
        },
        build,
    )?) as Box<dyn LayoutItem>)
}

#[cfg(test)]
mod tests {
    use super::*;
    use telar::WindowRoot;
    use telar::{reset_layout_runtime, set_theme};

    // Switching the locale after the panel is built re-renders its labels live: the section titles are
    // reactive `t!` closures, so the rendered text changes from English to Spanish without a rebuild.
    #[test]
    fn labels_live_switch_locale() {
        use telar::{ComponentList, DrawCommand, Event};

        fn has_text(tree: &ComponentList, needle: &str) -> bool {
            tree.commands()
                .iter()
                .any(|c| matches!(c, DrawCommand::Text { text, .. } if text.contains(needle)))
        }

        reset_layout_runtime();
        set_theme(NordTheme::new());
        let panel = settings_panel().expect("settings panel");
        let mut tree = ComponentList::new(WindowRoot::wrapping(panel).expect("root"));
        tree.on_event(&Event::WindowResized {
            width: 380,
            height: 1200,
        });

        // Force the locale after building so the assertion is independent of the machine's system locale; the
        // labels are reactive `t!` closures, so `commands()` re-renders in whatever locale is active now.
        telar::set_locale("en");
        assert!(has_text(&tree, "Settings"), "English title before switch");
        assert!(!has_text(&tree, "Ajustes"));

        telar::set_locale("es");
        assert!(
            has_text(&tree, "Ajustes"),
            "Spanish title after live switch"
        );
        assert!(
            !has_text(&tree, "Settings"),
            "English title gone after switch"
        );
    }

    /// Every form on every page, built. `labels_live_switch_locale` only ever reaches the first page — the
    /// page area is a keyed list over the *selected* page — so until this existed, a section that panicked on
    /// a nested signal read shipped as long as it was not on Appearance. Which is most of them.
    #[test]
    fn every_section_on_every_page_builds() {
        // A path that does not exist, so every form seeds from `Config::default` rather than from whatever
        // config the machine running the test happens to have.
        crate::form::set_source(std::path::PathBuf::from(
            "/nonexistent/hogar-shell-test.toml",
        ));
        for page in crate::pages::PAGES {
            for section in page.sections {
                reset_layout_runtime();
                set_theme(NordTheme::new());
                assert!(
                    (section.build)().is_ok(),
                    "{}/{} does not build",
                    page.label,
                    section.label
                );
            }
        }
    }

    /// Switching pages puts the view back at the top — and leaves it free to move afterwards.
    ///
    /// The second half is the one that has to be asserted: "scroll back to the top when the page changes" is
    /// an effect, and an effect that reads the offset it writes re-runs on every wheel tick and puts the view
    /// straight back, which is a page that cannot be scrolled at all rather than one that starts at its top.
    #[test]
    fn a_page_switch_returns_to_the_top_and_the_page_still_scrolls() {
        use telar::{ComponentList, Event, PointerSource, ScrollDelta};

        telar::Scope::with(|| {
            reset_layout_runtime();
            set_theme(NordTheme::new());
            let panel = settings_panel().expect("settings panel");
            let mut tree = ComponentList::new(WindowRoot::wrapping(panel).expect("root"));
            tree.on_event(&Event::WindowResized {
                width: 900,
                height: 600,
            });

            // The panel's own state, reached the way the panel reaches it: `kept` is scoped to the surface,
            // and this test is that surface.
            let page = kept("settings.page", || signal(0usize));
            let (_, offset_y) = kept("settings.scroll", || (signal(0.0f32), signal(0.0f32)));

            // Over the page area — right of the nav pane, below the header — and then a wheel down.
            let wheel = |tree: &mut ComponentList| {
                tree.on_event(&Event::PointerMoved {
                    x: 600.0,
                    y: 300.0,
                    source: PointerSource::Mouse,
                });
                tree.on_event(&Event::Scrolled {
                    delta: ScrollDelta::Pixels { x: 0.0, y: -120.0 },
                    x: 600.0,
                    y: 300.0,
                });
                telar::batch(|| {});
                telar::relayout_if_dirty();
            };

            wheel(&mut tree);
            assert!(
                offset_y.peek() > 0.0,
                "a page taller than its viewport scrolls, or the rest of this test proves nothing"
            );

            page.set(1);
            telar::batch(|| {});
            telar::relayout_if_dirty();
            assert_eq!(
                offset_y.peek(),
                0.0,
                "changing page is a different thing in the viewport, not the same thing resized"
            );

            wheel(&mut tree);
            assert!(
                offset_y.peek() > 0.0,
                "and the new page scrolls like any other — the effect that put the view back at the top must \
                 not have subscribed itself to the offset it wrote"
            );
        });
    }

    /// The same, in the tree the shell actually mounts: the window chrome around the panel, and the panel
    /// reached the way a float reaches it. The plain-panel test above misses whatever the frame contributes.
    #[test]
    fn a_page_switch_still_scrolls_inside_the_window_frame() {
        use telar::{ComponentList, Event, PointerSource, ScrollDelta, SurfaceFrameStyle};

        telar::Scope::with(|| {
            reset_layout_runtime();
            let theme = NordTheme::new();
            set_theme(theme);
            let body = settings_panel().expect("the settings panel builds");
            let frame = telar::window_frame(
                MODULE.to_string(),
                None,
                SurfaceFrameStyle {
                    background: theme.surface,
                    title_bar: theme.overlay,
                    title_text: theme.text,
                    close: theme.muted,
                    radius: 12.0,
                    font_size: theme.font(FontRole::Title),
                    controls: Default::default(),
                    body_inset: 12.0,
                    control_hover: telar::Color::TRANSPARENT,
                    close_hover: telar::Color::TRANSPARENT,
                },
                std::rc::Rc::new(|| {}),
                body,
                None,
            )
            .expect("surface frame");
            let mut tree = ComponentList::new(WindowRoot::wrapping(frame).expect("root"));
            tree.on_event(&Event::WindowResized {
                width: 920,
                height: 680,
            });

            let page = kept("settings.page", || signal(0usize));
            let (_, offset_y) = kept("settings.scroll", || (signal(0.0f32), signal(0.0f32)));
            let wheel = |tree: &mut ComponentList| {
                tree.on_event(&Event::PointerMoved {
                    x: 600.0,
                    y: 400.0,
                    source: PointerSource::Mouse,
                });
                tree.on_event(&Event::Scrolled {
                    delta: ScrollDelta::Pixels { x: 0.0, y: -120.0 },
                    x: 600.0,
                    y: 300.0,
                });
                telar::batch(|| {});
                telar::relayout_if_dirty();
            };

            wheel(&mut tree);
            assert!(offset_y.peek() > 0.0, "the first page scrolls");

            page.set(1);
            telar::batch(|| {});
            telar::relayout_if_dirty();
            assert_eq!(offset_y.peek(), 0.0, "a page switch starts at the top");

            wheel(&mut tree);
            assert!(
                offset_y.peek() > 0.0,
                "and the page under the frame still scrolls afterwards"
            );
        });
    }
}
