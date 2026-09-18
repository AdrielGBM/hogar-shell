//! Taking a picture of the screen.
//!
//! The pixels come from the compositor over a protocol — `ext-image-copy-capture` where there is one, older `wlr-screencopy` where there is not — which the platform crate owns. This layer is what happens to them afterwards: composing several outputs into one desktop, cropping a selection, encoding once, and deciding whether that goes to a file, to the clipboard or to an annotator.
//!
//! Everything here runs off the UI thread. A capture is a round trip to the compositor followed by a PNG encode of several megapixels; done in a click handler it would drop frames on every surface at once.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{SystemTime, UNIX_EPOCH};

use platform_wayland::{CaptureArea, CaptureBackend, EventSender};

use config::ScreenshotConfig;
use util::broadcast::Store;

/// A rectangle in the compositor's logical coordinate space — the space window geometry and output positions are reported in, and the one a selection drawn on an overlay is made in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Area {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl Area {
    pub fn from_corners(from: (f32, f32), to: (f32, f32)) -> Self {
        let (x0, x1) = (from.0.min(to.0), from.0.max(to.0));
        let (y0, y1) = (from.1.min(to.1), from.1.max(to.1));
        Self {
            x: x0.round() as i32,
            y: y0.round() as i32,
            width: (x1 - x0).round() as i32,
            height: (y1 - y0).round() as i32,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.width <= 0 || self.height <= 0
    }

    fn right(&self) -> i32 {
        self.x + self.width
    }

    fn bottom(&self) -> i32 {
        self.y + self.height
    }

    fn contains(&self, other: &Area) -> bool {
        other.x >= self.x
            && other.y >= self.y
            && other.right() <= self.right()
            && other.bottom() <= self.bottom()
    }
}

/// What to capture.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// Every output, composed into one image at its place in the layout.
    Screen,
    Output(String),
    Area(Area),
}

/// One capture request, as the flow that triggered it decided: which pixels, and what to do with them.
#[derive(Clone, Debug)]
pub struct Request {
    pub target: Target,
    pub cursor: bool,
    pub save: bool,
    pub copy: bool,
    /// Hand the saved file to `[screenshot] annotator`, when one is configured.
    pub annotate: bool,
}

impl Request {
    /// The request `[screenshot]` describes for `target` — what a keybind or a chip means by "take a screenshot".
    pub fn from_config(target: Target, config: &ScreenshotConfig) -> Self {
        Self {
            target,
            cursor: config.include_cursor,
            save: config.save,
            copy: config.copy,
            annotate: config.has_annotator(),
        }
    }
}

/// What the last capture did, so a panel can show it and a script can ask about it.
#[derive(Clone, Debug, PartialEq)]
pub struct Shot {
    pub path: Option<PathBuf>,
    pub copied: bool,
    pub size: (u32, u32),
    /// Seconds since the epoch, so a card can say how long ago without keeping a timer.
    pub taken_at: u64,
}

/// The last capture: the shot when one succeeded, the reason when it did not. `None` until the first attempt — which is the third state a panel needs, and the one a `Result` alone cannot carry.
static LAST: Store<Option<Result<Shot, String>>> = Store::new(|| None);

pub fn subscribe(tx: EventSender<Option<Result<Shot, String>>>) {
    LAST.subscribe(tx);
}

pub fn current() -> Option<Result<Shot, String>> {
    LAST.get()
}

/// Whether this compositor implements either capture protocol. Read before offering the gesture, so a missing one greys a button out instead of failing a keypress.
pub fn supported() -> bool {
    platform_wayland::capture_supported()
}

/// Takes `request` on a thread of its own and publishes the outcome. Returns immediately: the caller is a click handler or an IPC command, and neither should wait on a compositor round trip.
pub fn take(request: Request) {
    finish(request, None);
}

/// The same save/copy/annotate path for pixels the caller already has.
///
/// The area picker is the caller that needs it: with `[screenshot] freeze` on, the selection is drawn over a still of the screen taken *before* the overlay mapped, and cropping that still is the only way to capture what the user was looking at — asking the compositor again would photograph the overlay.
pub fn deliver(image: Image, request: Request) {
    finish(request, Some(image));
}

fn finish(request: Request, captured: Option<Image>) {
    let config = config::shared_config()
        .map(|c| c.screenshot.clone())
        .unwrap_or_default();
    let dir = config::shared_config()
        .map(|c| c.screenshot_dir())
        .unwrap_or_else(|| util::paths::cache_dir().join("screenshots"));
    let _ = std::thread::Builder::new()
        .name("hogar-shell-screenshot".to_string())
        .spawn(move || {
            let outcome = perform(&request, captured, &config, &dir);
            if let Err(reason) = &outcome {
                tracing::warn!("screenshot: {reason}");
            }
            announce(&outcome, &config);
            LAST.update(|last| *last = Some(outcome));
        });
}

/// Captures, saves, copies and hands off — in that order, so a failure to reach the clipboard cannot lose the file that was already written.
fn perform(
    request: &Request,
    captured: Option<Image>,
    config: &ScreenshotConfig,
    dir: &Path,
) -> Result<Shot, String> {
    let image = match captured {
        Some(image) => image,
        None => capture_pixels(request, config.backend())?,
    };
    // Encoded once, in memory, so the same bytes can be saved and put on the clipboard without a second encode or a round trip through the disk.
    let bytes = image.to_png()?;
    let path = if request.save {
        Some(write_file(&bytes, dir, &config.file_name)?)
    } else {
        None
    };
    if request.copy {
        util::clipboard::copy_bytes("image/png", bytes);
    }
    if let Some(path) = path.as_ref()
        && request.annotate
    {
        annotate(path, &config.annotator);
    }
    Ok(Shot {
        path,
        copied: request.copy,
        size: (image.width, image.height),
        taken_at: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|since| since.as_secs())
            .unwrap_or_default(),
    })
}

/// One image in memory: tightly-packed RGBA8, top row first.
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

impl From<platform_wayland::Capture> for Image {
    fn from(capture: platform_wayland::Capture) -> Self {
        Self {
            width: capture.width,
            height: capture.height,
            pixels: capture.pixels,
        }
    }
}

impl Image {
    fn to_png(&self) -> Result<Vec<u8>, String> {
        let mut bytes = Vec::new();
        let encoder = image::codecs::png::PngEncoder::new(std::io::Cursor::new(&mut bytes));
        image::ImageEncoder::write_image(
            encoder,
            &self.pixels,
            self.width,
            self.height,
            image::ExtendedColorType::Rgba8,
        )
        .map_err(|e| format!("encoding the capture: {e}"))?;
        Ok(bytes)
    }
}

/// Where an output's pixels sit in one screen-wide image: its logical rectangle multiplied by its own scale.
///
/// Exact whenever every output runs at the same scale, which is every single-monitor session and most others. A mixed-scale layout is the one case this can only approximate — the compositor reports an integer scale per output, so two screens at 1× and 1.5× have no common pixel grid to compose onto.
fn output_rects() -> Vec<(String, Area, i32)> {
    platform_wayland::outputs()
        .into_iter()
        .filter_map(|out| {
            let name = out.name?;
            let (width, height) = out.logical_size?;
            let scale = out.scale.max(1);
            Some((
                name,
                Area {
                    x: out.position.0,
                    y: out.position.1,
                    width,
                    height,
                },
                scale,
            ))
        })
        .collect()
}

fn capture_pixels(request: &Request, backend: CaptureBackend) -> Result<Image, String> {
    match &request.target {
        Target::Output(name) => capture_output(name, request.cursor, backend),
        Target::Screen => compose_screen(request.cursor, backend),
        Target::Area(area) => capture_area(*area, request.cursor, backend),
    }
}

fn capture_output(name: &str, cursor: bool, backend: CaptureBackend) -> Result<Image, String> {
    platform_wayland::capture(Some(name), CaptureArea::Output, cursor, backend)
        .map(Image::from)
        .map_err(|e| e.to_string())
}

/// Every output at its place in the layout. One capture per screen, composed rather than asked for as a whole: both protocols capture one output, so a desktop-wide picture is something only the shell can assemble.
fn compose_screen(cursor: bool, backend: CaptureBackend) -> Result<Image, String> {
    let mut parts = Vec::new();
    for (name, area, scale) in output_rects() {
        let image = capture_output(&name, cursor, backend).map_err(|e| format!("{name}: {e}"))?;
        parts.push(Placed {
            x: area.x * scale,
            y: area.y * scale,
            image,
        });
    }
    compose(parts).ok_or_else(|| "no outputs to capture".to_string())
}

/// A selection, captured from the one output that holds it where possible.
///
/// A region inside a single screen is asked for as a region, and the platform crate cuts it — off the compositor where the protocol can crop, out of the output's own pixels where it cannot. A selection spanning two screens has no single output to ask, so the whole desktop is composed and cropped instead: slower, and the only answer that is right.
fn capture_area(area: Area, cursor: bool, backend: CaptureBackend) -> Result<Image, String> {
    if area.is_empty() {
        return Err("the selection is empty".to_string());
    }
    if let Some((name, output, _scale)) = output_rects()
        .into_iter()
        .find(|(_, output, _)| output.contains(&area))
    {
        return platform_wayland::capture(
            Some(&name),
            CaptureArea::Region {
                x: area.x - output.x,
                y: area.y - output.y,
                width: area.width,
                height: area.height,
            },
            cursor,
            backend,
        )
        .map(Image::from)
        .map_err(|e| e.to_string());
    }
    let screen = compose_screen(cursor, backend)?;
    let scale = output_rects()
        .first()
        .map(|(_, _, scale)| *scale)
        .unwrap_or(1);
    let origin = screen_origin();
    crop(
        &screen,
        Area {
            x: (area.x - origin.0) * scale,
            y: (area.y - origin.1) * scale,
            width: area.width * scale,
            height: area.height * scale,
        },
    )
}

/// The top-left of the composed desktop in logical coordinates — not always `(0, 0)`, since a screen may sit left of or above the primary one.
fn screen_origin() -> (i32, i32) {
    let rects = output_rects();
    let x = rects.iter().map(|(_, area, _)| area.x).min().unwrap_or(0);
    let y = rects.iter().map(|(_, area, _)| area.y).min().unwrap_or(0);
    (x, y)
}

struct Placed {
    x: i32,
    y: i32,
    image: Image,
}

/// Lays `parts` out at their own offsets on one canvas sized to their bounding box.
fn compose(parts: Vec<Placed>) -> Option<Image> {
    if parts.len() == 1 {
        return parts.into_iter().next().map(|part| part.image);
    }
    let left = parts.iter().map(|p| p.x).min()?;
    let top = parts.iter().map(|p| p.y).min()?;
    let right = parts.iter().map(|p| p.x + p.image.width as i32).max()?;
    let bottom = parts.iter().map(|p| p.y + p.image.height as i32).max()?;
    let width = (right - left).max(1) as u32;
    let height = (bottom - top).max(1) as u32;
    let mut pixels = vec![0u8; width as usize * height as usize * 4];
    for part in &parts {
        let offset_x = (part.x - left) as usize;
        let offset_y = (part.y - top) as usize;
        for row in 0..part.image.height as usize {
            let source = row * part.image.width as usize * 4;
            let target = ((offset_y + row) * width as usize + offset_x) * 4;
            let length = part.image.width as usize * 4;
            if target + length > pixels.len() {
                continue;
            }
            pixels[target..target + length]
                .copy_from_slice(&part.image.pixels[source..source + length]);
        }
    }
    Some(Image {
        width,
        height,
        pixels,
    })
}

/// `area` of `image`, in pixels, clamped to what the image actually holds — a selection dragged past the edge of the screen is a selection to the edge, not a failure.
pub fn crop(image: &Image, area: Area) -> Result<Image, String> {
    let x = area.x.max(0) as u32;
    let y = area.y.max(0) as u32;
    let width = (area.width.max(0) as u32).min(image.width.saturating_sub(x));
    let height = (area.height.max(0) as u32).min(image.height.saturating_sub(y));
    if width == 0 || height == 0 {
        return Err("the selection is outside the screen".to_string());
    }
    let mut pixels = Vec::with_capacity(width as usize * height as usize * 4);
    for row in 0..height {
        let start = (((y + row) * image.width + x) * 4) as usize;
        pixels.extend_from_slice(&image.pixels[start..start + width as usize * 4]);
    }
    Ok(Image {
        width,
        height,
        pixels,
    })
}

/// Saves the capture under a name of its own in `dir` and returns where.
///
/// Written with [`util::fs::write_atomic`] directly rather than through the writer's queue: [`unique`] picks a name nothing else is writing, so there is no order to protect, and a 4K capture's fsync queued alongside the shell's config saves would stall the next save from the settings panel behind a file it has nothing to do with.
fn write_file(bytes: &[u8], dir: &Path, name_format: &str) -> Result<PathBuf, String> {
    let stem = chrono::Local::now().format(name_format).to_string();
    let path = unique(dir, &stem);
    util::fs::write_atomic(&path, bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path)
}

/// `<stem>.png`, or `<stem>-2.png` when that exists. Two captures inside the same second are a user pressing the key twice, and the second one must not overwrite the first.
fn unique(dir: &Path, stem: &str) -> PathBuf {
    let first = dir.join(format!("{stem}.png"));
    if !first.exists() {
        return first;
    }
    (2..1000)
        .map(|n| dir.join(format!("{stem}-{n}.png")))
        .find(|path| !path.exists())
        .unwrap_or(first)
}

/// Hands the saved file to the configured annotator. Detached and unwaited: an annotator is a window the user works in for as long as they like, not a subprocess the shell manages.
fn annotate(path: &Path, command: &str) {
    let mut words = annotator_words(command, path);
    if words.is_empty() {
        return;
    }
    let program = words.remove(0);
    let spawned = util::process::command(&program)
        .args(&words)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    if let Err(e) = spawned {
        tracing::warn!("screenshot: annotator '{program}': {e}");
    }
}

/// The annotator's argv: `{file}` substituted where the user put it, appended when they did not — so both `satty --filename {file}` and a bare `swappy -f` do the right thing.
fn annotator_words(command: &str, path: &Path) -> Vec<String> {
    let file = path.to_string_lossy().to_string();
    let mut words: Vec<String> = command
        .split_whitespace()
        .map(|word| word.replace("{file}", &file))
        .collect();
    if !words.is_empty() && !command.contains("{file}") {
        words.push(file);
    }
    words
}

/// Tells the user where the picture went. A capture with no visible outcome is indistinguishable from a keybind that did nothing.
///
/// Two channels, because they answer different questions: the notification is the *record* — it keeps the file name where the user can find it again — and the toast is the acknowledgement. `[screenshot] notify` and `[toasts.events] screenshot` are separate switches for that reason, and the toast is off by default.
fn announce(outcome: &Result<Shot, String>, config: &ScreenshotConfig) {
    let (title, body) = message(outcome);
    crate::toaster::post(
        crate::toaster::Event::Screenshot,
        crate::screenshot::glyph(),
        title.clone(),
        body.clone(),
    );
    if config.notify {
        crate::notifications::notify_local("hogar-shell", &title, &body);
    }
}

fn message(outcome: &Result<Shot, String>) -> (String, String) {
    match outcome {
        Ok(shot) => (
            telar::t!("screenshot.saved_title"),
            match (&shot.path, shot.copied) {
                (Some(path), true) => {
                    telar::t!("screenshot.saved_and_copied", file = file_label(path))
                }
                (Some(path), false) => telar::t!("screenshot.saved", file = file_label(path)),
                (None, true) => telar::t!("screenshot.copied"),
                (None, false) => telar::t!("screenshot.taken"),
            },
        ),
        Err(reason) => (telar::t!("screenshot.failed_title"), reason.clone()),
    }
}

fn file_label(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| path.display().to_string())
}

/// The pixels for `target`, for a caller that wants an image *on screen* rather than a file — the window info panel's preview. No PNG, no clipboard, no file: a preview that went through the disk would be a screenshot taken every second.
pub fn snapshot(target: Target, cursor: bool) -> Result<Image, String> {
    capture_pixels(
        &Request {
            target,
            cursor,
            save: false,
            copy: false,
            annotate: false,
        },
        configured_backend(),
    )
}

/// The route `[screenshot] backend` names, for the captures that have no request behind them to carry it.
fn configured_backend() -> CaptureBackend {
    config::shared_config()
        .map(|c| c.screenshot.backend())
        .unwrap_or_default()
}

/// Every output's current contents, for an overlay that has to stand still while the user draws on it.
///
/// Taken synchronously, on purpose. "Freeze the screen" means the pixels from the instant *before* the overlay appeared; handing the work to a thread and opening the overlay first would capture the overlay.
pub fn freeze_outputs() -> Vec<(String, Image)> {
    let backend = configured_backend();
    let mut frames = Vec::new();
    for (name, _, _) in output_rects() {
        match capture_output(&name, false, backend) {
            Ok(image) => frames.push((name, image)),
            Err(e) => tracing::warn!("screenshot: freezing {name}: {e}"),
        }
    }
    frames
}

/// The camera this service's toast and every chip that offers a capture share.
pub fn glyph() -> &'static str {
    "camera"
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(width: u32, height: u32, fill: u8) -> Image {
        Image {
            width,
            height,
            pixels: vec![fill; (width * height * 4) as usize],
        }
    }

    #[test]
    fn a_selection_is_normalised_whichever_way_it_was_dragged() {
        let down_right = Area::from_corners((10.0, 20.0), (110.0, 70.0));
        let up_left = Area::from_corners((110.0, 70.0), (10.0, 20.0));
        assert_eq!(down_right, up_left, "a drag has no preferred direction");
        assert_eq!(
            down_right,
            Area {
                x: 10,
                y: 20,
                width: 100,
                height: 50
            }
        );
        assert!(Area::from_corners((5.0, 5.0), (5.0, 5.0)).is_empty());
    }

    #[test]
    fn two_screens_compose_side_by_side_at_their_own_offsets() {
        let left = Placed {
            x: 0,
            y: 0,
            image: image(2, 2, 1),
        };
        let right = Placed {
            x: 2,
            y: 0,
            image: image(2, 2, 9),
        };
        let composed = compose(vec![left, right]).expect("two parts compose");
        assert_eq!((composed.width, composed.height), (4, 2));
        assert_eq!(composed.pixels[0], 1, "the left screen keeps the left half");
        assert_eq!(composed.pixels[2 * 4], 9, "and the right one starts at x=2");
    }

    #[test]
    fn a_screen_left_of_the_primary_one_still_lands_inside_the_canvas() {
        // A monitor at a negative x is the case a canvas rooted at (0,0) would drop entirely.
        let secondary = Placed {
            x: -2,
            y: 0,
            image: image(2, 1, 5),
        };
        let primary = Placed {
            x: 0,
            y: 0,
            image: image(2, 1, 7),
        };
        let composed = compose(vec![secondary, primary]).expect("composes");
        assert_eq!(composed.width, 4);
        assert_eq!(
            composed.pixels[0], 5,
            "the leftmost screen opens the canvas"
        );
        assert_eq!(composed.pixels[2 * 4], 7);
    }

    #[test]
    fn a_single_screen_is_handed_back_without_a_copy() {
        let only = Placed {
            x: 40,
            y: 40,
            image: image(3, 3, 4),
        };
        let composed = compose(vec![only]).expect("composes");
        assert_eq!(
            (composed.width, composed.height),
            (3, 3),
            "no padding to its offset"
        );
    }

    #[test]
    fn a_crop_past_the_edge_clamps_instead_of_failing() {
        let full = image(4, 4, 3);
        let inside = crop(
            &full,
            Area {
                x: 1,
                y: 1,
                width: 2,
                height: 2,
            },
        )
        .expect("crops");
        assert_eq!((inside.width, inside.height), (2, 2));

        let over = crop(
            &full,
            Area {
                x: 3,
                y: 3,
                width: 10,
                height: 10,
            },
        )
        .expect("clamps");
        assert_eq!(
            (over.width, over.height),
            (1, 1),
            "a drag past the screen edge is a selection to the edge"
        );
        assert!(
            crop(
                &full,
                Area {
                    x: 9,
                    y: 9,
                    width: 2,
                    height: 2
                }
            )
            .is_err(),
            "a selection wholly outside the screen has nothing to return"
        );
    }

    #[test]
    fn an_annotator_takes_the_file_where_it_asked_for_it() {
        let path = Path::new("/tmp/shot.png");
        assert_eq!(
            annotator_words("satty --filename {file} --fullscreen", path),
            vec!["satty", "--filename", "/tmp/shot.png", "--fullscreen"]
        );
        // No placeholder: the file is what the command is for, so it goes last rather than nowhere.
        assert_eq!(
            annotator_words("swappy -f", path),
            vec!["swappy", "-f", "/tmp/shot.png"]
        );
        assert!(annotator_words("   ", path).is_empty());
    }

    /// A backend named in the config is "this route or none"; anything else, including a name a past build understood and this one does not, means take whichever route works.
    #[test]
    fn the_configured_backend_names_a_route_or_falls_back_to_either() {
        let with = |backend: &str| {
            ScreenshotConfig {
                backend: backend.to_string(),
                ..Default::default()
            }
            .backend()
        };
        assert_eq!(with("screencopy"), CaptureBackend::Screencopy);
        assert_eq!(
            with(" Image-Copy-Capture "),
            CaptureBackend::ImageCopyCapture
        );
        assert_eq!(with("auto"), CaptureBackend::Auto);
        assert_eq!(
            with("grim"),
            CaptureBackend::Auto,
            "a route this build no longer has"
        );
    }

    #[test]
    fn an_area_belongs_to_the_output_that_holds_all_of_it() {
        let screen = Area {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        };
        assert!(screen.contains(&Area {
            x: 10,
            y: 10,
            width: 100,
            height: 100
        }));
        assert!(
            !screen.contains(&Area {
                x: 1900,
                y: 10,
                width: 100,
                height: 100
            }),
            "a selection running onto the next monitor is not this one's to crop"
        );
    }
}
