//! The distribution mark that opens the shell's own menu.

use telar::{LayoutError, LayoutItem};

const OS_RELEASE: &str = "/etc/os-release";

/// The Iconify glyph for a distribution `ID` from `os-release`. Only the families with a recognisable mark in the default set are listed; anything else falls back to a generic penguin rather than a wrong logo.
fn glyph_for(id: &str) -> &'static str {
    match id {
        "nixos" => "simple-icons:nixos",
        "arch" | "archarm" => "simple-icons:archlinux",
        "debian" => "simple-icons:debian",
        "ubuntu" => "simple-icons:ubuntu",
        "fedora" => "simple-icons:fedora",
        "gentoo" => "simple-icons:gentoo",
        "opensuse" | "opensuse-tumbleweed" | "opensuse-leap" => "simple-icons:opensuse",
        "alpine" => "simple-icons:alpinelinux",
        "manjaro" => "simple-icons:manjaro",
        "pop" => "simple-icons:popos",
        "linuxmint" => "simple-icons:linuxmint",
        "endeavouros" => "simple-icons:endeavouros",
        _ => "simple-icons:linux",
    }
}

/// The `ID=` field of an `os-release` file, unquoted. The spec allows the value to be quoted or bare, and `ID_LIKE` must not be mistaken for it — hence matching the key exactly rather than by prefix.
fn parse_id(text: &str) -> Option<String> {
    text.lines()
        .filter_map(|line| line.split_once('='))
        .find(|(key, _)| key.trim() == "ID")
        .map(|(_, value)| {
            value
                .trim()
                .trim_matches('"')
                .trim_matches('\'')
                .to_string()
        })
        .filter(|id| !id.is_empty())
}

/// The icon the logo chip shows: the configured `[general] logo` when set, else the running distribution's mark detected from `/etc/os-release`.
pub fn logo_icon() -> String {
    let configured = ui::module::surface_env()
        .map(|env| env.config.general.logo.clone())
        .unwrap_or_default();
    if !configured.trim().is_empty() {
        return configured;
    }
    let id = std::fs::read_to_string(OS_RELEASE)
        .ok()
        .and_then(|text| parse_id(&text))
        .unwrap_or_default();
    glyph_for(&id).to_string()
}

pub fn logo_chip() -> Result<Box<dyn LayoutItem>, LayoutError> {
    let fg = ui::module::module_fg();
    let icon = logo_icon();
    ui::icon::icon_view(
        move || icon.clone(),
        move || fg.get(),
        ui::module::icon_px(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn os_release_id_is_read_unquoted_and_not_confused_with_id_like() {
        let text = "\
NAME=\"NixOS\"
ID=nixos
ID_LIKE=\"debian arch\"
VERSION_ID=\"25.05\"
";
        assert_eq!(parse_id(text), Some("nixos".to_string()));

        let quoted = "PRETTY_NAME=\"Ubuntu\"\nID=\"ubuntu\"\n";
        assert_eq!(parse_id(quoted), Some("ubuntu".to_string()));
    }

    #[test]
    fn a_file_without_an_id_yields_none() {
        assert_eq!(parse_id("NAME=\"Something\"\n"), None);
        assert_eq!(parse_id("ID=\n"), None, "an empty ID is not an id");
        assert_eq!(parse_id(""), None);
    }

    #[test]
    fn an_unknown_distribution_falls_back_to_a_generic_mark() {
        assert_eq!(glyph_for("nixos"), "simple-icons:nixos");
        assert_eq!(
            glyph_for("some-new-distro"),
            "simple-icons:linux",
            "better a generic penguin than another distribution's logo"
        );
    }
}
