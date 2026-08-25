//! Cover art: the local file for whatever `mpris:artUrl` a player handed over.
//!
//! Three cases behind one call, which is the point of the module. A `file://` URL is already on disk and needs nothing but percent-decoding. An `http(s)://` one has to be downloaded, and downloading it on the UI thread would stall the frame for as long as the server takes — so it goes through the same request/worker shape the Iconify store uses: ask, get a signal, and let the worker fill it in. A `data:` URL carries the bytes inline and is written straight to the cache.
//!
//! The cache is keyed by the URL rather than by the track, because that is what actually identifies the image: two tracks from one album share an `artUrl` and should share one download, and a player that reuses a temporary path for every track (several do) would otherwise poison a track-keyed cache.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

use platform_wayland::{EventSender, watch};
use telar::{ReadSignal, RwSignal, signal};

use util::paths;

const FETCH_TIMEOUT: Duration = Duration::from_secs(15);
/// Cover art is a few hundred KB at most; anything far larger is a server handing back something that is not an image, and writing it to the user's cache would be the only lasting effect.
const MAX_BYTES: usize = 8 * 1024 * 1024;

/// Where a request has got to. Mirrors the icon store's states so a view can branch the same way.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ArtState {
    Loading,
    Ready(PathBuf),
    /// Nothing to show: no art URL, an unreachable one, or a payload that was not an image.
    Missing,
}

/// `$XDG_CACHE_HOME/hogar-shell/art`.
pub fn cache_dir() -> PathBuf {
    paths::cache_dir().join("art")
}

/// A stable, filesystem-safe name for a URL.
///
/// Hashed rather than sanitised: an `artUrl` can be a query string hundreds of characters long, longer than any filesystem's name limit, and two URLs differing only past that limit would collide. The extension is carried over where the URL has a plausible one, purely so the cache is browsable.
fn cache_name(url: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in url.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    match extension_of(url) {
        Some(ext) => format!("{hash:016x}.{ext}"),
        None => format!("{hash:016x}"),
    }
}

/// The image extension a URL ends in, if it is one the shell can decode.
fn extension_of(url: &str) -> Option<&str> {
    let path = url.split(['?', '#']).next()?;
    let ext = path.rsplit_once('.')?.1;
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "png" | "jpg" | "jpeg" | "webp"
    )
    .then_some(ext)
}

pub fn cache_path(url: &str) -> PathBuf {
    cache_dir().join(cache_name(url))
}

/// Percent-decodes a `file://` URL into a path. Players emit them encoded, so a track in a directory with a space or an accent resolves to a path that does not exist unless this runs.
fn decode_file_url(url: &str) -> Option<PathBuf> {
    let rest = url.strip_prefix("file://")?;
    // `file://localhost/path` and `file:///path` both mean the local machine.
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    let mut out: Vec<u8> = Vec::with_capacity(rest.len());
    let bytes = rest.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok()?;
            if let Ok(byte) = u8::from_str_radix(hex, 16) {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    Some(PathBuf::from(String::from_utf8(out).ok()?))
}

/// The local file for `url` without touching the network: a decoded `file://` path, or a cache entry that is already there. `None` means it would have to be fetched.
pub fn ready(url: &str) -> Option<PathBuf> {
    let url = url.trim();
    if url.is_empty() {
        return None;
    }
    if url.starts_with("file://") {
        return decode_file_url(url).filter(|p| p.exists());
    }
    if url.starts_with('/') {
        let path = PathBuf::from(url);
        return path.exists().then_some(path);
    }
    let cached = cache_path(url);
    cached.exists().then_some(cached)
}

/// Downloads `url` into the cache and returns the file. Blocking — only ever called on the worker thread.
fn fetch(url: &str, agent: &ureq::Agent) -> Option<PathBuf> {
    if let Some(local) = ready(url) {
        return Some(local);
    }
    let bytes = if let Some(payload) = url.strip_prefix("data:") {
        decode_data_url(payload)?
    } else {
        let mut response = agent.get(url).call().ok()?;
        let body = response.body_mut().with_config().limit(MAX_BYTES as u64);
        body.read_to_vec().ok()?
    };
    if !looks_like_an_image(&bytes) {
        tracing::warn!("cover art at {url} was not an image");
        return None;
    }
    let path = paths::ensure_dir(cache_dir()).join(cache_name(url));
    match std::fs::write(&path, &bytes) {
        Ok(()) => Some(path),
        Err(e) => {
            tracing::warn!("cannot cache cover art at {}: {e}", path.display());
            None
        }
    }
}

/// The bytes of a `data:` URL, which some players use for embedded art.
fn decode_data_url(payload: &str) -> Option<Vec<u8>> {
    let (meta, data) = payload.split_once(',')?;
    if !meta.ends_with(";base64") {
        return None;
    }
    base64_decode(data)
}

/// Standard base64, no padding required. A dependency for one call site would not earn its place.
fn base64_decode(text: &str) -> Option<Vec<u8>> {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    let mut buffer: u32 = 0;
    let mut bits = 0u32;
    for byte in text.bytes() {
        if byte == b'=' || byte.is_ascii_whitespace() {
            continue;
        }
        let value = ALPHABET.iter().position(|c| *c == byte)? as u32;
        buffer = (buffer << 6) | value;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
        }
    }
    Some(out)
}

/// Whether the bytes start with a magic number the shell's decoders understand. A server answering an error page with a 200 is common enough that trusting the content type is not enough.
fn looks_like_an_image(bytes: &[u8]) -> bool {
    bytes.starts_with(&[0x89, b'P', b'N', b'G'])
        || bytes.starts_with(&[0xFF, 0xD8, 0xFF])
        || (bytes.len() > 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP")
}

struct ArtStore {
    signals: RefCell<HashMap<String, RwSignal<ArtState>>>,
    requests: Sender<String>,
}

thread_local! {
    static STORE: RefCell<Option<ArtStore>> = const { RefCell::new(None) };
}

/// The state of `url`, starting a fetch if this is the first time it has been asked for.
///
/// The signal is cached per URL, so a card rebuilt on every track change does not re-download art it already has, and two surfaces showing the same player share one request.
pub fn art(url: &str) -> ReadSignal<ArtState> {
    let url = url.trim().to_string();
    if url.is_empty() {
        return signal(ArtState::Missing).read_only();
    }
    ensure_store();
    STORE.with(|s| {
        let borrow = s.borrow();
        let Some(store) = borrow.as_ref() else {
            return signal(ArtState::Missing).read_only();
        };
        if let Some(existing) = store.signals.borrow().get(&url) {
            return existing.read_only();
        }
        let initial = match ready(&url) {
            Some(path) => ArtState::Ready(path),
            None => ArtState::Loading,
        };
        let handle = signal(initial.clone());
        store
            .signals
            .borrow_mut()
            .insert(url.clone(), handle.clone());
        if initial == ArtState::Loading {
            let _ = store.requests.send(url);
        }
        handle.read_only()
    })
}

fn ensure_store() {
    if STORE.with(|s| s.borrow().is_some()) {
        return;
    }
    let (requests, incoming) = channel::<String>();
    STORE.with(|s| {
        *s.borrow_mut() = Some(ArtStore {
            signals: RefCell::new(HashMap::new()),
            requests,
        });
    });
    // Headless, `watch` is a no-op: no worker runs and every request stays on `Loading`, which is what an offline render shows.
    watch(
        move |sender| run_worker(incoming, sender),
        |(url, path)| deliver(url, path),
    );
}

fn run_worker(incoming: Receiver<String>, sender: EventSender<(String, Option<PathBuf>)>) {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(FETCH_TIMEOUT))
        .build()
        .into();
    for url in incoming {
        let path = fetch(&url, &agent);
        if !sender.send((url, path)) {
            break;
        }
    }
}

fn deliver(url: String, path: Option<PathBuf>) {
    STORE.with(|s| {
        let borrow = s.borrow();
        let Some(store) = borrow.as_ref() else {
            return;
        };
        // Clone the handle out and drop the map borrow BEFORE `set`: a signal write flushes effects synchronously, and an effect that asks for another URL would re-enter this borrow and panic.
        let handle = store.signals.borrow().get(&url).cloned();
        if let Some(handle) = handle {
            handle.set(match path {
                Some(path) => ArtState::Ready(path),
                None => ArtState::Missing,
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cache_name_is_stable_filesystem_safe_and_keeps_a_usable_extension() {
        let url = "https://example.test/covers/album cover.jpg?token=abc";
        assert_eq!(cache_name(url), cache_name(url), "stable across calls");
        assert_ne!(
            cache_name(url),
            cache_name("https://example.test/other.jpg")
        );

        let name = cache_name(url);
        assert!(name.ends_with(".jpg"), "browsable: {name}");
        assert!(
            !name.contains('/') && !name.contains(' ') && !name.contains('?'),
            "nothing a filesystem would refuse: {name}"
        );
        // A URL with no extension, or one that is not an image, gets the bare hash rather than a made-up one.
        assert!(!cache_name("https://example.test/art").contains('.'));
        assert!(!cache_name("https://example.test/a.php?x=1").contains('.'));
    }

    #[test]
    fn a_file_url_is_percent_decoded() {
        assert_eq!(
            decode_file_url("file:///home/u/My%20Music/cover.png"),
            Some(PathBuf::from("/home/u/My Music/cover.png"))
        );
        assert_eq!(
            decode_file_url("file://localhost/tmp/a.png"),
            Some(PathBuf::from("/tmp/a.png")),
            "the localhost authority means the same machine"
        );
        assert_eq!(
            decode_file_url("file:///m%C3%BAsica/t.jpg"),
            Some(PathBuf::from("/música/t.jpg")),
            "a multi-byte character survives decoding"
        );
        assert_eq!(decode_file_url("https://example.test/a.png"), None);
    }

    #[test]
    fn only_bytes_that_are_actually_an_image_reach_the_cache() {
        assert!(looks_like_an_image(&[0x89, b'P', b'N', b'G', 13, 10]));
        assert!(looks_like_an_image(&[0xFF, 0xD8, 0xFF, 0xE0]));
        let mut webp = b"RIFF____WEBPVP8 ".to_vec();
        webp.extend_from_slice(&[0; 8]);
        assert!(looks_like_an_image(&webp));
        // The case this exists for: a server answering a 200 with an error page.
        assert!(!looks_like_an_image(b"<!DOCTYPE html><html>404"));
        assert!(!looks_like_an_image(&[]));
    }

    #[test]
    fn an_inline_data_url_decodes_to_its_bytes() {
        // "PNG" in base64, behind the header a player would send.
        let png = decode_data_url("image/png;base64,iVBORw0KGgo=").expect("decodes");
        assert_eq!(&png[..4], &[0x89, b'P', b'N', b'G']);
        assert!(looks_like_an_image(&png));
        assert_eq!(
            decode_data_url("image/png,notbase64"),
            None,
            "only the base64 form carries bytes"
        );
    }

    #[test]
    fn nothing_to_show_is_not_a_request() {
        assert!(ready("").is_none());
        assert!(ready("   ").is_none());
        assert!(
            ready("file:///nonexistent-cover-9e3f.png").is_none(),
            "a path the player named but that is not there"
        );
    }
}
