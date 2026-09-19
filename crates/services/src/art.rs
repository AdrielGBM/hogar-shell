//! Cover art: the local file for whatever `mpris:artUrl` a player handed over; cached by URL rather than by track, since several players reuse one temporary path across tracks, which would poison a track-keyed cache.

use std::cell::RefCell;
use std::path::PathBuf;
use std::time::Duration;

use telar::{ReadSignal, signal};

use util::asset::{Load, Loader};
use util::paths;

const FETCH_TIMEOUT: Duration = Duration::from_secs(15);
/// Cover art is a few hundred KB at most; anything far larger is a server handing back something that is not an image, and writing it to the user's cache would be the only lasting effect.
const MAX_BYTES: usize = 8 * 1024 * 1024;

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
///
/// Written with [`util::fs::write_atomic`] directly rather than through the writer's queue: the name is the URL's hash, so there is no order between writes to protect, only a file that must never be seen half-written — [`ready`] answers from `exists()`, and would hand a partial download to the decoder as a finished image.
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
    let path = cache_path(url);
    match util::fs::write_atomic(&path, &bytes) {
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

type Store = Loader<String, PathBuf>;

thread_local! {
    static ART: RefCell<Option<Store>> = const { RefCell::new(None) };
}

/// The state of `url`, starting a fetch if this is the first time it has been asked for; cached per URL so a card rebuilt on every track change does not re-download art it already has.
pub fn art(url: &str) -> ReadSignal<Load<PathBuf>> {
    let url = url.trim();
    if url.is_empty() {
        return signal(Load::Missing).read_only();
    }
    ensure_store();
    ART.with(|store| {
        let borrow = store.borrow();
        let Some(store) = borrow.as_ref() else {
            return signal(Load::Missing).read_only();
        };
        store.get(url.to_string(), |url| ready(url))
    })
}

fn ensure_store() {
    if ART.with(|store| store.borrow().is_some()) {
        return;
    }
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(FETCH_TIMEOUT))
        .build()
        .into();
    let store = Loader::new(move |url: &String| fetch(url, &agent));
    ART.with(|cell| *cell.borrow_mut() = Some(store));
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

    /// The media card that first shows a cover asks inside its own build, and closing that panel disposed the signal the cache went on handing out: the next open read freed storage and took the UI thread down with it.
    #[test]
    fn a_cover_outlives_the_panel_that_first_showed_it() {
        let cover = cache_dir().join("outlives-its-panel.png");
        util::fs::write_atomic(&cover, &[0x89, b'P', b'N', b'G']).expect("scratch cover");
        let url = format!("file://{}", cover.display());

        let panel = telar::owner_scope();
        let owner = panel.id();
        assert_eq!(art(&url).get(), Load::Ready(cover.clone()));
        drop(panel);
        telar::dispose_owner(owner);

        assert_eq!(
            art(&url).get(),
            Load::Ready(cover.clone()),
            "the reopened panel reads the cached cover, not a freed handle"
        );
        telar::reset_layout_runtime();
        assert_eq!(art(&url).get(), Load::Ready(cover));
    }
}
