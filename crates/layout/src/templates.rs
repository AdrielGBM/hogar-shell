//! Starting layouts the shell ships as bundles. Using one copies its layout into the store under a name of the user's own, as a file nobody imported: what a shipped template runs is trusted like the user's own.

use std::path::Path;

use util::report::{Message, Report};

use crate::bundle::{self, Bundle};
use crate::library::is_komponent_name;
use crate::model::*;
use crate::store::{LayoutStore, StoreError};

struct Shipped {
    name: &'static str,
    manifest: &'static str,
    layout: &'static str,
    title: fn() -> Message,
    description: fn() -> Message,
}

const SHIPPED: &[Shipped] = &[Shipped {
    name: "showcase",
    manifest: include_str!("../templates/showcase/manifest.toml"),
    layout: include_str!("../templates/showcase/layouts/showcase.toml"),
    title: || util::message!("template.showcase.title"),
    description: || util::message!("template.showcase.description"),
}];

#[derive(Clone, Debug)]
pub struct Template {
    pub bundle: Bundle,
    pub title: Message,
    pub description: Message,
}

impl Template {
    pub fn name(&self) -> &str {
        &self.bundle.manifest.name
    }

    pub fn layout(&self) -> Option<&Layout> {
        self.bundle.layouts.get(&LayoutId::new(self.name()))
    }
}

pub fn shipped() -> Vec<(&'static str, Result<Template, Report>)> {
    SHIPPED
        .iter()
        .map(|shipped| (shipped.name, read(shipped)))
        .collect()
}

pub fn all() -> Vec<Template> {
    shipped()
        .into_iter()
        .filter_map(|(_, template)| template.ok())
        .collect()
}

pub fn named(name: &str) -> Result<Template, TemplateError> {
    all()
        .into_iter()
        .find(|template| template.name() == name)
        .ok_or_else(|| TemplateError::Unknown(name.to_string()))
}

fn read(shipped: &Shipped) -> Result<Template, Report> {
    let bundle = bundle::of_texts(
        &Path::new("templates").join(shipped.name),
        shipped.manifest,
        &[(shipped.name, shipped.layout)],
        &[],
    )?;
    Ok(Template {
        bundle,
        title: (shipped.title)(),
        description: (shipped.description)(),
    })
}

#[derive(Debug)]
pub enum TemplateError {
    Unknown(String),
    BadName(String),
    Taken(LayoutId),
    Store(StoreError),
}

impl TemplateError {
    pub fn message(&self) -> Message {
        match self {
            TemplateError::Unknown(name) => util::message!(
                "finding.template_unknown",
                name = name,
                known = all()
                    .iter()
                    .map(|template| format!("`{}`", template.name()))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            TemplateError::BadName(name) => {
                util::message!("finding.bundle_file_name", name = name)
            }
            TemplateError::Taken(id) => util::message!("finding.template_taken", name = id),
            TemplateError::Store(why) => why.message(),
        }
    }
}

/// Copies the layout of the template `name` into `store` as a new layout, called `called` or else the template's name, numbered where that is taken. A name a layout or a file of the store's directory already has is refused rather than written over, an unreadable file included. Nothing else in the store changes.
pub fn put(
    store: &mut LayoutStore,
    name: &str,
    called: Option<&str>,
) -> Result<LayoutId, TemplateError> {
    let template = named(name)?;
    let mut layout = template
        .layout()
        .cloned()
        .ok_or_else(|| TemplateError::Unknown(name.to_string()))?;
    let id = match called {
        Some(called) if !is_komponent_name(called) => {
            return Err(TemplateError::BadName(called.to_string()));
        }
        Some(called) if taken(store, called) => {
            return Err(TemplateError::Taken(LayoutId::new(called)));
        }
        Some(called) => {
            layout.name = called.to_string();
            LayoutId::new(called)
        }
        None => free_name(store, template.name()),
    };
    layout.id = id.clone();
    store.put_layout(layout).map_err(TemplateError::Store)?;
    Ok(id)
}

fn taken(store: &LayoutStore, name: &str) -> bool {
    let id = LayoutId::new(name);
    store.get(&id).is_some() || store.path_of(&id).exists()
}

fn free_name(store: &LayoutStore, base: &str) -> LayoutId {
    let name = std::iter::once(base.to_string())
        .chain((2..).map(|nth| format!("{base}-{nth}")))
        .find(|name| !taken(store, name))
        .expect("the counting runs out long after the names do");
    LayoutId::new(name)
}
