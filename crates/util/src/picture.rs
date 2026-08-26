//! Reading an image file off disk into something a surface can draw.
//!
//! Three surfaces want the same three lines — the wallpaper, the dashboard's avatar and its cover art — and each one that wrote them itself would also have to decide for itself what a missing file means. It means `None`: the caller falls back to a colour or a glyph rather than reserving space for a picture that is not coming.

use std::path::Path;
use std::sync::Arc;

use telar::{
    Image, ImageData, Raster, LayoutError, LayoutItem, LayoutStyle, ObjectFit, SizeDimension,
};

/// Decodes an image file into RGBA, or `None` when the path is missing or the format is unsupported.
pub fn decode(path: &Path) -> Option<ImageData> {
    let rgba = ::image::open(path).ok()?.to_rgba8();
    let (width, height) = rgba.dimensions();
    Some(ImageData::new(rgba.into_raw(), width, height))
}

/// A fixed-size, cover-cropped picture with rounded corners — a cover, a tile.
///
/// The radius goes on the `Image` and not on a box around it: a `StyledContainer`'s radius rounds the *fill* it paints, and a bitmap child draws straight over that. This is `Image::with_radius`, which telar grew for exactly this reason.
pub fn square(path: &Path, size: f32, radius: f32) -> Option<Box<dyn LayoutItem>> {
    fitted(
        path,
        LayoutStyle::new().width(size).height(size).flex_shrink(0.0),
        radius,
    )
}

/// A square picture rounded all the way to a circle — a face, which is the one picture in this shell that is a person rather than a thing.
pub fn circle(path: &Path, size: f32) -> Option<Box<dyn LayoutItem>> {
    square(path, size, size / 2.0)
}

/// A picture that fills its container's width at a fixed height.
pub fn banner(path: &Path, height: f32, radius: f32) -> Option<Box<dyn LayoutItem>> {
    fitted(
        path,
        LayoutStyle::new()
            .width(SizeDimension::Percent(1.0))
            .height(height),
        radius,
    )
}

fn fitted(path: &Path, style: LayoutStyle, radius: f32) -> Option<Box<dyn LayoutItem>> {
    let data = Arc::new(decode(path)?);
    let image: Result<Image, LayoutError> = Image::new(
        style,
        move || data.clone(),
        || Raster::Smooth,
        || ObjectFit::Cover,
    );
    match image {
        Ok(image) => Some(Box::new(image.with_radius(radius))),
        Err(e) => {
            tracing::warn!("cannot lay out {}: {e}", path.display());
            None
        }
    }
}
