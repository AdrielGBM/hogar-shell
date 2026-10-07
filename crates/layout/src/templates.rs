//! Starting layouts the shell ships as bundles. Using one copies its layout into the store under a name of the user's own, as a file nobody imported: what a shipped template runs is trusted like the user's own.

use std::path::Path;
use std::sync::LazyLock;

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

pub fn shipped() -> &'static [(&'static str, Result<Template, Report>)] {
    static READ: LazyLock<Vec<(&'static str, Result<Template, Report>)>> = LazyLock::new(|| {
        SHIPPED
            .iter()
            .map(|shipped| (shipped.name, read(shipped)))
            .collect()
    });
    &READ
}

/// Every shipped template that reads. One that does not is a build of the shell gone wrong rather than anything the user did, so it is logged and left out of what is offered.
pub fn all() -> &'static [Template] {
    static READABLE: LazyLock<Vec<Template>> = LazyLock::new(|| {
        shipped()
            .iter()
            .filter_map(|(name, read)| match read {
                Ok(template) => Some(template.clone()),
                Err(report) => {
                    tracing::error!(
                        "the shipped template `{name}` does not read, so it is not offered:\n{}",
                        report.render()
                    );
                    None
                }
            })
            .collect()
    });
    &READABLE
}

pub fn named(name: &str) -> Result<&'static Template, TemplateError> {
    all()
        .iter()
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

/// Copies the layout of the template `name` into `store` as a new layout, called `called` or else the template's name, numbered where that is taken, and named after the id it lands under where that is not the template's own. A name a layout or a file of the store's directory already has is refused rather than written over, an unreadable file included. Nothing else in the store changes.
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
        Some(called) if store.is_taken(&LayoutId::new(called)) => {
            return Err(TemplateError::Taken(LayoutId::new(called)));
        }
        Some(called) => LayoutId::new(called),
        None => store.free_id(template.name()),
    };
    if id.as_str() != template.name() {
        layout.name = id.to_string();
    }
    layout.id = id.clone();
    store.put_layout(layout).map_err(TemplateError::Store)?;
    Ok(id)
}
