use std::rc::Rc;

use telar::{Container, LayoutItem, LayoutStyle, MenuEntry, Rect, SizeDimension, Text, use_theme};

use config::Edge;
use config::theme::{FontRole, NordTheme};
use layout::templates;
use layout::{LayerKind, LayoutId};
use surfaces::reconcile;
use ui::descriptor::Built;

use crate::host;
use crate::mode;
use crate::session::EditError;

pub(crate) fn install() {
    host::add_strip_action((|| telar::t!("editor.template.new"), || mode::said(open())));
}

pub(crate) fn open() -> Result<(), EditError> {
    let mode = mode::required()?;
    let desktop = reconcile::desktop_now(Some(&mode.output)).ok_or_else(EditError::no_output)?;
    let at = surfaces::menu::pointer().unwrap_or((desktop.size.0 / 2.0, desktop.size.1 / 2.0));
    crate::context::show(
        &desktop,
        (LayerKind::Overlay, Edge::Top),
        Rect::new(at.0, at.1, 0.0, 0.0),
        at,
        gallery(),
    );
    Ok(())
}

pub fn gallery() -> Vec<MenuEntry> {
    templates::all()
        .iter()
        .map(|template| {
            let name = template.name().to_string();
            let (title, description) = (template.title.render(), template.description.render());
            MenuEntry::Custom {
                widget: Rc::new(move || face(title.clone(), description.clone())),
                act: Some(Rc::new(move || mode::said(use_template(&name).map(drop)))),
            }
        })
        .collect()
}

pub fn use_template(name: &str) -> Result<LayoutId, EditError> {
    let template = templates::named(name).map_err(|why| EditError::refused(why.message()))?;
    let made = surfaces::layouts::start(|store| {
        templates::put(store, name, None).map_err(|why| why.message())
    })
    .map_err(EditError::refused)?;
    mode::confirm(telar::t!(
        "editor.template.made",
        layout = made.as_str(),
        template = template.title.render()
    ));
    Ok(made)
}

fn face(title: String, description: String) -> Built {
    let theme = use_theme::<NordTheme>();
    let line = |said: String, role: FontRole, muted: bool| {
        Text::declaring(
            move || said.clone(),
            LayoutStyle::new().width(SizeDimension::Percent(1.0)),
            move |inherited| {
                let ink = match muted {
                    true => theme.muted,
                    false => theme.text,
                };
                theme.text_over(inherited, role, ink)
            },
        )
    };
    let lines: Vec<Box<dyn LayoutItem>> = vec![
        Box::new(line(title, FontRole::Body, false)?),
        Box::new(line(description, FontRole::Caption, true)?),
    ];
    Ok(Box::new(Container::new(
        LayoutStyle::new()
            .flex_column()
            .width(SizeDimension::Percent(1.0))
            .padding_horizontal(ui::scale::space::sm())
            .padding_vertical(ui::scale::space::xs()),
        lines,
    )?))
}
