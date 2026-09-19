//! Who is signed in: the user's picture and name, drawn the same wherever they appear — this module's widget, the dashboard's user card and the lock screen.

use std::path::{Path, PathBuf};

use telar::{
    AlignItems, Container, JustifyContent, LayoutError, LayoutItem, LayoutStyle, SizeDimension,
    Text, box_item, use_theme,
};

use config::DashboardConfig;
use config::theme::{FontRole, NordTheme};
use ui::card::{Card, Density};
use ui::host::Host;
use ui::icon::icon_view;
use ui::scale::space;
use util::paths;

const SMALL_PICTURE: f32 = 64.0;
const PICTURE: f32 = 96.0;
const ACCOUNTS_ICONS: &str = "/var/lib/AccountsService/icons";

/// The signed-in user's login name, from their passwd entry.
pub fn name() -> String {
    let name = services::pam::current_user();
    if name.trim().is_empty() {
        telar::t!("sysinfo.no_reading")
    } else {
        name
    }
}

/// Where the user's picture is: `[dashboard] avatar` when set, else the first of the places a desktop conventionally keeps one — `~/.face`, `~/.face.icon`, then the AccountsService icon a display manager writes.
pub fn avatar_path(config: &DashboardConfig) -> Option<PathBuf> {
    let configured = config.avatar.trim();
    if !configured.is_empty() {
        let path = paths::expand_tilde(Path::new(configured));
        return path.is_file().then_some(path);
    }
    let home = paths::home_dir()?;
    let mut candidates = vec![home.join(".face"), home.join(".face.icon")];
    let user = services::pam::current_user();
    if !user.trim().is_empty() {
        candidates.push(Path::new(ACCOUNTS_ICONS).join(user));
    }
    candidates.into_iter().find(|path| path.is_file())
}

/// The picture and the name, built but not yet arranged: each place that shows them lays them out its own way.
pub struct Identity {
    pub face: Box<dyn LayoutItem>,
    pub name: Box<dyn LayoutItem>,
}

/// The user's picture, round and `picture` across, or a silhouette when there is none; and their name.
pub fn identity(
    config: &DashboardConfig,
    picture: f32,
    theme: NordTheme,
) -> Result<Identity, LayoutError> {
    let face = match avatar_path(config).and_then(|path| util::picture::circle(&path, picture)) {
        Some(face) => face,
        None => icon_view(
            || "circle-user-round".to_string(),
            move || theme.subtle,
            picture,
        )?,
    };
    let name = name();
    let name = box_item(Text::new(
        move || name.clone(),
        LayoutStyle::new(),
        move || {
            theme
                .text_style(FontRole::Title, theme.text)
                .with_font_weight(700)
        },
    )?);
    Ok(Identity { face, name })
}

pub fn widget(host: &Host) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let small = host.is_small();
    let picture = if small { SMALL_PICTURE } else { PICTURE };
    let Identity { face, name } = identity(
        host.options::<DashboardConfig>(),
        picture,
        use_theme::<NordTheme>(),
    )?;

    let style = LayoutStyle::new()
        .align_items(AlignItems::CENTER)
        .justify_content(JustifyContent::CENTER)
        .width(SizeDimension::Percent(1.0))
        .height(SizeDimension::Percent(1.0));
    let style = if small {
        style.flex_column().gap(space::md())
    } else {
        style.flex_row().gap(space::xl())
    };
    let identity = Container::new(style, vec![face, name])?;
    Card::bare()
        .child(move || Ok(Box::new(identity) as Box<dyn LayoutItem>))
        .build(Density::Widget)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_configured_avatar_that_is_not_a_file_is_no_avatar() {
        let dir = std::env::temp_dir().join(format!("hogar-shell-user-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a directory");
        let at = |path: &Path| DashboardConfig {
            avatar: path.display().to_string(),
            ..DashboardConfig::default()
        };
        assert_eq!(avatar_path(&at(&dir)), None, "a directory is not a picture");
        assert_eq!(avatar_path(&at(&dir.join("missing.png"))), None);

        let face = dir.join("face.png");
        std::fs::write(&face, b"x").expect("a file");
        assert_eq!(avatar_path(&at(&face)), Some(face));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
