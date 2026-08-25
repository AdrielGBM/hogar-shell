//! Wallpaper thumbnails on screen, without a full-resolution decode on the frame that asks for one.
//!
//! The cache itself belongs to the wallpaper service, which is where `hogar-shell wallpaper` reaches it from. What lives here is the *surface* half: a grid asks for a picture and gets a signal, a worker generates the ones that are not cached yet, and each tile swaps its glyph for the real thing as it lands. A grid of two hundred images therefore opens immediately and fills in, rather than freezing the shell for the length of two hundred JPEG decodes.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use telar::{
    AlignItems, Image, ImageFilter, JustifyContent, LayoutError, LayoutItem, LayoutStyle,
    ObjectFit, ReactiveList, ReadSignal, RectStyle, StyledContainer, signal,
};

use config::theme::NordTheme;
use services::wallpaper;
use util::asset::{Load, Loader};

type Store = Loader<(PathBuf, u32), PathBuf>;

thread_local! {
    static THUMBNAILS: RefCell<Option<Store>> = const { RefCell::new(None) };
}

/// The thumbnail for `source`, generating it the first time it is asked for.
pub fn of(source: &Path, size: u32) -> ReadSignal<Load<PathBuf>> {
    let key = (source.to_path_buf(), size);
    ensure_store();
    THUMBNAILS.with(|store| {
        let borrow = store.borrow();
        let Some(store) = borrow.as_ref() else {
            return signal(Load::Missing).read_only();
        };
        store.get(key, |(source, size)| {
            wallpaper::cached_thumbnail(source, *size)
        })
    })
}

fn ensure_store() {
    if THUMBNAILS.with(|store| store.borrow().is_some()) {
        return;
    }
    let store = Loader::new(|(source, size): &(PathBuf, u32)| wallpaper::thumbnail(source, *size));
    THUMBNAILS.with(|cell| *cell.borrow_mut() = Some(store));
}

/// The size `[wallpaper] thumbnail_size` asks for. Read here rather than at each call site so a grid and a picker cannot generate two caches of the same pictures at two sizes.
pub fn size() -> u32 {
    config::config()
        .map(|config| config.wallpaper.thumbnail_size)
        .unwrap_or(320)
}

/// A picture of `source` at `width`×`height`, showing `glyph` until it has one.
///
/// A keyed one-item list rather than a plain image, the same shape the dashboard's cover art uses: the file is decoded once per picture instead of once per repaint, and the placeholder is replaced in place when the thumbnail lands.
pub fn view(
    source: PathBuf,
    width: f32,
    height: f32,
    radius: f32,
    glyph: &'static str,
    theme: NordTheme,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let size = size();
    let list = ReactiveList::new(
        move || vec![of(&source, size).get().ready().cloned()],
        |path: &Option<PathBuf>| {
            path.as_ref()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default()
        },
        move |path: Option<PathBuf>| match path
            .and_then(|path| picture(&path, width, height, radius))
        {
            Some(image) => Ok(image),
            None => placeholder(width, height, radius, glyph, theme),
        },
        0.0,
    )?;
    Ok(Box::new(list))
}

fn picture(path: &Path, width: f32, height: f32, radius: f32) -> Option<Box<dyn LayoutItem>> {
    let data = Arc::new(util::picture::decode(path)?);
    let image = Image::new(
        LayoutStyle::new()
            .width(width)
            .height(height)
            .flex_shrink(0.0),
        move || data.clone(),
        || ImageFilter::Linear,
        || ObjectFit::Cover,
    )
    .map(|image| image.with_radius(radius));
    match image {
        Ok(image) => Some(Box::new(image)),
        Err(e) => {
            tracing::warn!("cannot lay out thumbnail {}: {e}", path.display());
            None
        }
    }
}

fn placeholder(
    width: f32,
    height: f32,
    radius: f32,
    glyph: &'static str,
    theme: NordTheme,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let icon = crate::icon::icon_view(
        move || glyph.to_string(),
        move || theme.muted,
        (width.min(height) * 0.35).max(12.0),
    )?;
    Ok(Box::new(StyledContainer::new(
        LayoutStyle::new()
            .width(width)
            .height(height)
            .flex_shrink(0.0)
            .align_items(AlignItems::CENTER)
            .justify_content(JustifyContent::CENTER),
        move |_| RectStyle::filled(theme.overlay, radius),
        vec![icon],
    )?))
}
