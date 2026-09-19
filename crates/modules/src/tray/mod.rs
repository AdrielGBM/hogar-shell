use std::path::Path;
use std::sync::Arc;

use telar::{
    AlignItems, Color, ImageData, JustifyContent, LayoutError, LayoutItem, LayoutStyle, ObjectFit,
    PointerButton, Raster, ReadSignal, Rect, RectStyle, StyledContainer, track_layout,
};

mod menu;

use config::theme::NordTheme;
use config::{SurfaceEnv, TrayConfig};
use services::tray::{self as tray_service, Pixmap, TrayItem};
use ui::host::Host;
use ui::icon::{app_icon_view_tinted, icon_view};

/// Drawn for an application that names an icon nobody can resolve and ships no pixels either — rare, but a blank gap that still takes clicks is worse than an obvious placeholder.
const FALLBACK_GLYPH: &str = "mdi:application-outline";

/// The items the bar draws: the ones the user hasn't hidden, minus those asking to be `Passive`, which is the spec's way for an application to say "I am running but have nothing to report".
pub fn visible(items: &[TrayItem], config: &TrayConfig) -> Vec<TrayItem> {
    if !config.enabled {
        return Vec::new();
    }
    items
        .iter()
        .filter(|item| item.status != tray_service::Status::Passive)
        .filter(|item| !config.is_hidden(&item.id))
        .cloned()
        .collect()
}

/// A file inside the application's own `IconThemePath`, which is where several applications put an icon that exists in no installed theme. Checked before the theme lookup precisely because the theme has no answer.
fn private_icon(item: &TrayItem) -> Option<String> {
    let dir = item.icon_theme_path.trim();
    let name = item.icon_reference().trim();
    if dir.is_empty() || name.is_empty() {
        return None;
    }
    ["svg", "png"]
        .iter()
        .map(|ext| Path::new(dir).join(format!("{name}.{ext}")))
        .find(|path| path.is_file())
        .map(|path| path.to_string_lossy().into_owned())
}

fn pixmap_widget(pixmap: &Arc<Pixmap>, size: f32) -> Result<Box<dyn LayoutItem>, LayoutError> {
    // The service guarantees `rgba` is exactly `width * height * 4`, which is what `ImageData::new` asserts.
    let data = Arc::new(ImageData::new(
        pixmap.rgba.clone(),
        pixmap.width,
        pixmap.height,
    ));
    Ok(Box::new(telar::Image::new(
        LayoutStyle::new().width(size).height(size).flex_shrink(0.0),
        move || data.clone(),
        || Raster::Smooth,
        || ObjectFit::Contain,
    )?))
}

/// One application's icon, resolved through everything the spec and the desktop offer, most specific first: the user's own substitution, the application's private icon directory, the icon theme, the raw pixels it handed over, and finally a placeholder.
fn icon_widget(
    item: &TrayItem,
    config: &TrayConfig,
    tint: Color,
    size: f32,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    if let Some(glyph) = config.icon_sub_for(&item.id) {
        let glyph = glyph.to_string();
        return icon_view(move || glyph.clone(), move || tint, size);
    }
    let recolour = config.recolour.then_some(tint);
    if let Some(path) = private_icon(item)
        && let Some(widget) = app_icon_view_tinted(&path, size, recolour)?
    {
        return Ok(widget);
    }
    if let Some(widget) = app_icon_view_tinted(item.icon_reference(), size, recolour)? {
        return Ok(widget);
    }
    if let Some(pixmap) = item.icon_pixmap() {
        return pixmap_widget(pixmap, size);
    }
    icon_view(|| FALLBACK_GLYPH.to_string(), move || tint, size)
}

/// Where the chip sits inside its bar, and which bar that is — everything the menu needs to anchor itself. `None` for a host that runs along no bar, where there is no edge to hang a menu off.
fn anchor_for(host: &Host, rect: ReadSignal<Rect>) -> Option<(Rect, SurfaceEnv)> {
    let edge = host.axis?;
    let env = SurfaceEnv::for_edge(
        std::sync::Arc::clone(host.config()),
        edge,
        host.output.clone(),
    );
    Some((rect.get(), env))
}

/// A primary click. An item that says it is a menu, or that implements no `Activate`, gets its menu opened — which for everything built on libappindicator is the only interaction it has.
fn primary(item: &TrayItem, host: &Host, rect: ReadSignal<Rect>) {
    if item.item_is_menu || !item.has_activate {
        open_menu(item, host, rect);
        return;
    }
    tray_service::activate(item, 0, 0);
}

/// A right click always means "show me the menu". Only when the item exposes none does this fall back to asking the application to pop its own.
fn open_menu(item: &TrayItem, host: &Host, rect: ReadSignal<Rect>) {
    if item.menu.trim().is_empty() {
        tray_service::context_menu(item, 0, 0);
        return;
    }
    let Some((chip, env)) = anchor_for(host, rect) else {
        return;
    };
    menu::toggle(item, chip, env);
}

fn secondary(item: &TrayItem) {
    tray_service::secondary_activate(item, 0, 0);
}

/// One tray icon, wrapped in its own pressable box.
///
/// Built here rather than in the view because the view's `for` is reactive: it constructs each item afresh whenever that application comes back, so its content has to be an expression (`build`) rather than a widget bound once in `[logic]`.
///
/// Right-click opens the item's menu and middle-click is `SecondaryActivate`; what a primary click does depends on the item, since `Activate` is far from universal — see [`primary`].
#[derive(telar::Props)]
pub struct TrayIconProps {
    pub item: TrayItem,
    pub config: TrayConfig,
    pub host: Host,
    pub theme: NordTheme,
}

pub fn tray_icon(
    props: TrayIconProps,
    _children: telar::Children,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let TrayIconProps {
        item,
        config,
        host,
        theme,
    } = props;
    let size = host.icon_size();
    let radius = host.corner_radius();
    let icon = icon_widget(&item, &config, host.foreground, size)?;
    let pad = if config.compact {
        (size * 0.08).round().max(1.0)
    } else {
        (size * 0.2).round().max(2.0)
    };
    let rest = if config.background {
        theme.surface
    } else {
        Color::TRANSPARENT
    };
    let hover = theme.overlay;

    let style = LayoutStyle::new()
        .flex_row()
        .align_items(AlignItems::CENTER)
        .justify_content(JustifyContent::CENTER)
        .padding_all(pad)
        .flex_shrink(0.0);

    let press_item = item.clone();
    let alt_host = host.clone();
    let alt_item = item.clone();
    let scroll_item = item;
    let container =
        StyledContainer::new(style, move |_r| RectStyle::filled(rest, radius), vec![icon])?;
    // The chip's own laid-out rect is what the menu anchors to, so it has to be tracked before the handlers that read it are attached.
    let rect = track_layout(container.layout_node())
        .expect("a container registers its rect")
        .read_only();
    let alt_rect = rect;
    let container = container
        .hover_style(move |_r| RectStyle::filled(hover, radius))
        .active_style(move |_r| RectStyle::filled(hover.darken(0.14), radius))
        .on_press(move || primary(&press_item, &host, rect))
        .on_alt_press(move |button| match button {
            PointerButton::Secondary => open_menu(&alt_item, &alt_host, alt_rect),
            _ => secondary(&alt_item),
        })
        .on_scroll(move |dx, dy| {
            // The wheel reports pixels; the spec wants a step count, and an applet that maps it to volume expects a small number rather than the ~60 one notch produces.
            let (delta, horizontal) = if dx.abs() > dy.abs() {
                (dx, true)
            } else {
                (dy, false)
            };
            if delta != 0.0 {
                tray_service::scroll(
                    &scroll_item,
                    (delta / 60.0).round().clamp(-3.0, 3.0) as i32,
                    horizontal,
                );
            }
        });
    Ok(Box::new(container))
}

#[cfg(test)]
mod tests {
    use super::*;
    use services::tray::Status;

    fn item(id: &str, status: Status) -> TrayItem {
        TrayItem {
            key: format!(":1.1/{id}"),
            id: id.to_string(),
            status,
            ..TrayItem::default()
        }
    }

    #[test]
    fn a_passive_item_is_running_but_not_drawn() {
        let items = [
            item("nm-applet", Status::Active),
            item("quiet", Status::Passive),
        ];
        let shown = visible(&items, &TrayConfig::default());
        assert_eq!(shown.len(), 1);
        assert_eq!(shown[0].id, "nm-applet");
    }

    #[test]
    fn an_item_asking_for_attention_is_still_drawn() {
        let items = [item("backup", Status::NeedsAttention)];
        assert_eq!(visible(&items, &TrayConfig::default()).len(), 1);
    }

    #[test]
    fn hidden_ids_are_dropped_by_pattern() {
        let config = TrayConfig {
            hidden: vec!["steam_app_*".to_string()],
            ..TrayConfig::default()
        };
        let items = [
            item("steam_app_440", Status::Active),
            item("nm-applet", Status::Active),
        ];
        let shown = visible(&items, &config);
        assert_eq!(shown.len(), 1);
        assert_eq!(shown[0].id, "nm-applet");
    }

    #[test]
    fn a_disabled_tray_draws_nothing_at_all() {
        let config = TrayConfig {
            enabled: false,
            ..TrayConfig::default()
        };
        assert!(visible(&[item("nm-applet", Status::Active)], &config).is_empty());
    }

    #[test]
    fn a_private_icon_directory_is_only_used_when_it_holds_the_file() {
        let mut item = item("dropbox", Status::Active);
        item.icon_name = "dropboxstatus-idle".to_string();
        assert_eq!(
            private_icon(&item),
            None,
            "no directory named, nothing to find"
        );

        let dir = std::env::temp_dir().join(format!("hogar-shell-tray-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        item.icon_theme_path = dir.to_string_lossy().into_owned();
        assert_eq!(
            private_icon(&item),
            None,
            "a directory without the file is not a match"
        );

        let png = dir.join("dropboxstatus-idle.png");
        std::fs::write(&png, b"not really a png").unwrap();
        assert_eq!(
            private_icon(&item),
            Some(png.to_string_lossy().into_owned())
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
