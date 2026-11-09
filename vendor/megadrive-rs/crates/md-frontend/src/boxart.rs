//! Box art (front cover) of the Library's games: the user's own picture next
//! to the ROM, or the libretro-thumbnails one, downloaded once in the
//! background and kept in a cache.
//!
//! libretro-thumbnails (the RetroArch database, no account or key) names its
//! pictures after the No-Intro game name, i.e. the usual ROM file name; its
//! README: "If the characters &*/:`<>?\|" appear in a game name [...] they
//! must be replaced with _ in the corresponding thumbnail filename". The
//! pictures are copyrighted: they stay in the user's cache, never in the
//! repository.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, SystemTime};

/// Box art folder of the Mega Drive / Genesis playlist.
pub const LIBRETRO_BOXARTS: &str =
    "https://thumbnails.libretro.com/Sega%20-%20Mega%20Drive%20-%20Genesis/Named_Boxarts";
/// Characters libretro-thumbnails replaces with '_' in file names.
const RESERVED: &str = "&*/:`<>?\\|\"";
/// A "not found" answer is trusted this long before asking again.
const MISSING_RETRY: Duration = Duration::from_secs(7 * 24 * 3600);
/// Exit codes of `curl -f`: HTTP error (404) and unreadable file:// URL.
const CURL_HTTP_ERROR: i32 = 22;
const CURL_FILE_ERROR: i32 = 37;

/// libretro-thumbnails name of the ROM at `rom`: its file name without
/// extension (an archive's own name for a .zip), reserved characters '_'.
pub fn libretro_name(rom: &Path) -> String {
    let stem = rom
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    stem.chars()
        .map(|c| if RESERVED.contains(c) { '_' } else { c })
        .collect()
}

/// URL of `name`'s picture under `base`: every byte outside the unreserved
/// set (RFC 3986 letters, digits, `-._~`) percent-encoded.
pub fn cover_url(base: &str, name: &str) -> String {
    let mut url = format!("{base}/");
    for b in format!("{name}.png").bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            url.push(b as char);
        } else {
            url.push_str(&format!("%{b:02X}"));
        }
    }
    url
}

/// The cache folder: `$XDG_CACHE_HOME/megadrive-rs/boxart`, else
/// `~/.cache/megadrive-rs/boxart`.
pub fn cache_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))?;
    Some(base.join("megadrive-rs").join("boxart"))
}

fn cached_png(cache: &Path, name: &str) -> PathBuf {
    cache.join(format!("{name}.png"))
}

fn missing_marker(cache: &Path, name: &str) -> PathBuf {
    cache.join(format!("{name}.missing"))
}

/// The picture to show for `rom`: the user's own `<rom>.png` next to it,
/// else the cached libretro one.
pub fn cover_source(rom: &Path, cache: Option<&Path>) -> Option<PathBuf> {
    if let Some(own) = own_cover(rom) {
        return Some(own);
    }
    let cached = cached_png(cache?, &libretro_name(rom));
    cached.is_file().then_some(cached)
}

/// Whether `rom`'s libretro picture should be downloaded: no own picture,
/// none cached, and no recent "not found" answer.
pub fn needs_download(rom: &Path, cache: &Path, now: SystemTime) -> bool {
    if cover_source(rom, Some(cache)).is_some() {
        return false;
    }
    let marker = missing_marker(cache, &libretro_name(rom));
    match std::fs::metadata(&marker).and_then(|m| m.modified()) {
        Ok(when) => now.duration_since(when).unwrap_or_default() >= MISSING_RETRY,
        Err(_) => true,
    }
}

/// A decoded picture, pixels as the framebuffer's RGBA8888 u32 values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub pixels: Vec<u32>,
    pub width: usize,
    pub height: usize,
}

/// Decodes the PNG at `path` (any colour type and depth: expanded to 8-bit
/// RGB or RGBA, grey to RGB).
pub fn load_cover(path: &Path) -> Option<Image> {
    let mut decoder = png::Decoder::new(std::fs::File::open(path).ok()?);
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).ok()?;
    let data = &buf[..info.buffer_size()];
    let pixel = |p: &[u8]| -> u32 {
        let (r, g, b, a) = match p.len() {
            1 => (p[0], p[0], p[0], 0xFF),
            2 => (p[0], p[0], p[0], p[1]),
            3 => (p[0], p[1], p[2], 0xFF),
            _ => (p[0], p[1], p[2], p[3]),
        };
        u32::from_be_bytes([r, g, b, a])
    };
    let channels = info.color_type.samples();
    Some(Image {
        pixels: data.chunks_exact(channels).map(pixel).collect(),
        width: info.width as usize,
        height: info.height as usize,
    })
}

/// `image` scaled down to fit `max_w` × `max_h` with its proportions kept,
/// each pixel the mean of the source block it covers (box filter; an image
/// already small enough is kept as is).
pub fn thumbnail(image: &Image, max_w: usize, max_h: usize) -> Image {
    let (sw, sh) = (image.width.max(1), image.height.max(1));
    if sw <= max_w && sh <= max_h {
        return image.clone();
    }
    let (w, h) = if sw * max_h <= sh * max_w {
        ((sw * max_h / sh).max(1), max_h.max(1))
    } else {
        (max_w.max(1), (sh * max_w / sw).max(1))
    };
    let mut pixels = Vec::with_capacity(w * h);
    for ty in 0..h {
        let (y0, y1) = (ty * sh / h, ((ty + 1) * sh / h).max(ty * sh / h + 1));
        for tx in 0..w {
            let (x0, x1) = (tx * sw / w, ((tx + 1) * sw / w).max(tx * sw / w + 1));
            let mut sum = [0u32; 4];
            for y in y0..y1 {
                for &p in &image.pixels[y * sw + x0..y * sw + x1] {
                    for (s, b) in sum.iter_mut().zip(p.to_be_bytes()) {
                        *s += u32::from(b);
                    }
                }
            }
            let n = ((y1 - y0) * (x1 - x0)) as u32;
            pixels.push(u32::from_be_bytes(sum.map(|s| (s / n) as u8)));
        }
    }
    Image {
        pixels,
        width: w,
        height: h,
    }
}

/// The image held under the mouse while a cover is dragged: `image` scaled
/// down to fit `max_w` × `max_h`, its opacity multiplied by `alpha`/255
/// (see-through, so what lies under it stays visible).
pub fn drag_ghost(image: &Image, max_w: usize, max_h: usize, alpha: u8) -> Image {
    let mut ghost = thumbnail(image, max_w, max_h);
    for p in &mut ghost.pixels {
        let [r, g, b, a] = p.to_be_bytes();
        let a = (u16::from(a) * u16::from(alpha) / 255) as u8;
        *p = u32::from_be_bytes([r, g, b, a]);
    }
    ghost
}

/// Makes the cover grid's thumbnails in a background thread: the latest
/// request first (the games on screen now), each answered once with the
/// thumbnail, or None when the game has no cover.
pub struct Thumbnailer {
    queue: Sender<PathBuf>,
    pub done: Receiver<(PathBuf, Option<Image>)>,
}

impl Thumbnailer {
    pub fn start(cache: Option<PathBuf>, max_w: usize, max_h: usize) -> Thumbnailer {
        let (queue, jobs) = channel::<PathBuf>();
        let (report, done) = channel();
        std::thread::spawn(move || {
            let mut waiting: Vec<PathBuf> = Vec::new();
            loop {
                if waiting.is_empty() {
                    match jobs.recv() {
                        Ok(rom) => waiting.push(rom),
                        Err(_) => return,
                    }
                }
                waiting.extend(jobs.try_iter());
                let Some(rom) = waiting.pop() else { continue };
                let image = cover_source(&rom, cache.as_deref())
                    .and_then(|path| load_cover(&path))
                    .map(|image| thumbnail(&image, max_w, max_h));
                if report.send((rom, image)).is_err() {
                    return;
                }
            }
        });
        Thumbnailer { queue, done }
    }

    pub fn request(&self, rom: &Path) {
        let _ = self.queue.send(rom.to_path_buf());
    }
}

/// Outcome of one download.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fetched {
    /// The picture is now in the cache.
    Saved(PathBuf),
    /// libretro has no picture of that name (marker written).
    Missing,
    /// Network or tool error: tried again next run.
    Failed(String),
}

/// Downloads pictures one at a time in a background thread (gentle on the
/// server) with `curl`: an external tool like zenity, no dependency.
pub struct Downloader {
    queue: Sender<(PathBuf, String)>,
    pub done: Receiver<(PathBuf, Fetched)>,
}

impl Downloader {
    /// Downloads from `base` into `cache`.
    pub fn start(base: String, cache: PathBuf) -> Downloader {
        let (queue, jobs) = channel::<(PathBuf, String)>();
        let (report, done) = channel();
        std::thread::spawn(move || {
            for (rom, name) in jobs {
                let fetched = fetch(&base, &cache, &name);
                if report.send((rom, fetched)).is_err() {
                    return;
                }
            }
        });
        Downloader { queue, done }
    }

    /// Queues the picture of `rom`.
    pub fn request(&self, rom: &Path) {
        let _ = self.queue.send((rom.to_path_buf(), libretro_name(rom)));
    }
}

/// Names to try on libretro for the file name `name`, best first:
/// - "<title> (Europe)": the European box when there is one (libretro has
///   them for some games only, e.g. "Sonic The Hedgehog 2 (Europe)");
/// - the name itself;
/// - without No-Intro language tags such as "(En,Ja)" and GoodTools tags
///   such as "[!]": libretro's pictures follow its playlist names, which
///   drop them ("Streets of Rage (World) (En,Ja)" is "Streets of Rage
///   (World)" there; checked on the server);
/// - the title with its first parenthesised group only (drops "(Rev 1)",
///   "(Beta)"...).
pub fn candidate_names(name: &str) -> Vec<String> {
    let title_end = name.find(['(', '[']).unwrap_or(name.len());
    let title = name[..title_end].trim();
    // European box art first when libretro has one (user's choice).
    let mut names = Vec::new();
    let europe = format!("{title} (Europe)");
    if !title.is_empty() && europe != name {
        names.push(europe);
    }
    names.push(name.to_string());
    let mut add = |n: String| {
        let n = n.split_whitespace().collect::<Vec<_>>().join(" ");
        if !n.is_empty() && !names.contains(&n) {
            names.push(n);
        }
    };
    let groups = split_groups(name);
    let kept: String = groups
        .iter()
        .filter(|g| !(g.starts_with('[') || is_language_tag(g)))
        .map(String::as_str)
        .collect();
    add(kept);
    if let Some(first) = groups
        .iter()
        .find(|g| g.starts_with('(') && !is_language_tag(g))
    {
        add(format!("{} {first}", &name[..title_end]));
    }
    names
}

/// `name` cut into plain text and "(...)" / "[...]" groups, in order.
fn split_groups(name: &str) -> Vec<String> {
    let mut groups = Vec::new();
    let mut current = String::new();
    for c in name.chars() {
        if c == '(' || c == '[' {
            if !current.is_empty() {
                groups.push(std::mem::take(&mut current));
            }
            current.push(c);
        } else if (c == ')' || c == ']') && !current.is_empty() {
            current.push(c);
            groups.push(std::mem::take(&mut current));
        } else {
            current.push(c);
        }
    }
    if !current.is_empty() {
        groups.push(current);
    }
    groups
}

/// A No-Intro language tag: "(En)", "(En,Ja)", "(En,Fr,De)"... (two
/// letters, upper then lower case, comma separated).
fn is_language_tag(group: &str) -> bool {
    let Some(inner) = group.strip_prefix('(').and_then(|g| g.strip_suffix(')')) else {
        return false;
    };
    inner.split(',').all(|code| {
        let b = code.as_bytes();
        b.len() == 2 && b[0].is_ascii_uppercase() && b[1].is_ascii_lowercase()
    })
}

/// Downloads the picture of the file name `name` into the cache, under that
/// name, trying `candidate_names` in turn.
fn fetch(base: &str, cache: &Path, name: &str) -> Fetched {
    if let Err(e) = std::fs::create_dir_all(cache) {
        return Fetched::Failed(e.to_string());
    }
    let target = cached_png(cache, name);
    let temporary = cache.join(format!("{name}.png.part"));
    for candidate in candidate_names(name) {
        let status = Command::new("curl")
            .args(["-fsL", "--max-time", "20", "-o"])
            .arg(&temporary)
            .arg(cover_url(base, &candidate))
            .status();
        match status {
            Ok(s) if s.success() => {
                return match std::fs::rename(&temporary, &target) {
                    Ok(()) => Fetched::Saved(target),
                    Err(e) => Fetched::Failed(e.to_string()),
                };
            }
            Ok(s) => {
                let _ = std::fs::remove_file(&temporary);
                if !matches!(s.code(), Some(CURL_HTTP_ERROR | CURL_FILE_ERROR)) {
                    return Fetched::Failed(format!("curl exit {:?}", s.code()));
                }
            }
            Err(e) => return Fetched::Failed(format!("curl: {e}")),
        }
    }
    let _ = std::fs::write(missing_marker(cache, name), b"");
    Fetched::Missing
}

/// The libretro box art names listed by the folder's HTML index: every
/// `href="<name>.png"`, percent-decoded, sorted, without duplicates.
pub fn parse_index(html: &str) -> Vec<String> {
    let mut names: Vec<String> = html
        .split("href=\"")
        .skip(1)
        .filter_map(|rest| rest.split('"').next())
        .filter_map(|href| href.strip_suffix(".png"))
        .filter(|href| !href.contains('/'))
        .map(percent_decode)
        .collect();
    names.sort_by_key(|n| n.to_lowercase());
    names.dedup();
    names
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |b: u8| (b as char).to_digit(16);
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(h), Some(l)) = (hex(bytes[i + 1]), hex(bytes[i + 2]))
        {
            out.push((h * 16 + l) as u8);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The index is downloaded again after this long.
const INDEX_MAX_AGE: Duration = Duration::from_secs(7 * 24 * 3600);
/// Pictures kept in the browsing cache before it is emptied.
const BROWSE_CACHE_FILES: usize = 200;

/// Downloads `url` to `target` with curl (through a `.part` file, renamed
/// once complete). Err(true) when the server has no such file.
fn download(url: &str, target: &Path) -> Result<(), bool> {
    let temporary = target.with_extension("part");
    let status = Command::new("curl")
        .args(["-fsL", "--max-time", "30", "-o"])
        .arg(&temporary)
        .arg(url)
        .status();
    match status {
        Ok(s) if s.success() => std::fs::rename(&temporary, target).map_err(|_| false),
        Ok(s) => {
            let _ = std::fs::remove_file(&temporary);
            Err(matches!(s.code(), Some(CURL_HTTP_ERROR | CURL_FILE_ERROR)))
        }
        Err(_) => Err(false),
    }
}

/// The libretro names of `base`'s folder, in a background thread: the
/// index cached in `cache` when less than a week old, else downloaded.
/// None when it cannot be had.
pub fn load_index(base: String, cache: PathBuf) -> Receiver<Option<Vec<String>>> {
    let (sender, receiver) = channel();
    std::thread::spawn(move || {
        let path = cache.join("index.html");
        let fresh = std::fs::metadata(&path)
            .and_then(|m| m.modified())
            .is_ok_and(|t| t.elapsed().unwrap_or_default() < INDEX_MAX_AGE);
        let ok = fresh
            || (std::fs::create_dir_all(&cache).is_ok()
                && download(&format!("{base}/"), &path).is_ok());
        let names = ok
            .then(|| std::fs::read_to_string(&path).ok())
            .flatten()
            .map(|html| parse_index(&html))
            .filter(|names| !names.is_empty());
        let _ = sender.send(names);
    });
    receiver
}

/// Downloads the pictures looked at while choosing a cover into `dir`, in a
/// background thread; only the latest request is served when several wait
/// (the user moves through the list faster than pictures arrive).
pub struct Browser {
    queue: Sender<String>,
    /// (name, picture file or None when missing or failed).
    pub done: Receiver<(String, Option<PathBuf>)>,
}

impl Browser {
    pub fn start(base: String, dir: PathBuf) -> Browser {
        let (queue, jobs) = channel::<String>();
        let (report, done) = channel();
        std::thread::spawn(move || {
            let _ = std::fs::create_dir_all(&dir);
            if std::fs::read_dir(&dir).map_or(0, |d| d.count()) > BROWSE_CACHE_FILES {
                let _ = std::fs::remove_dir_all(&dir);
                let _ = std::fs::create_dir_all(&dir);
            }
            while let Ok(mut name) = jobs.recv() {
                while let Ok(newer) = jobs.try_recv() {
                    name = newer;
                }
                let target = cached_png(&dir, &name);
                let found = target.is_file() || download(&cover_url(&base, &name), &target).is_ok();
                if report.send((name, found.then_some(target))).is_err() {
                    return;
                }
            }
        });
        Browser { queue, done }
    }

    pub fn request(&self, name: &str) {
        let _ = self.queue.send(name.to_string());
    }
}

/// Makes `picture` the cached cover of `rom` (replacing the one found
/// automatically) and drops its "not found" marker.
pub fn choose(cache: &Path, rom: &Path, picture: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(cache)?;
    let name = libretro_name(rom);
    let target = cached_png(cache, &name);
    let temporary = cache.join(format!("{name}.png.part"));
    std::fs::copy(picture, &temporary)?;
    std::fs::rename(&temporary, &target)?;
    let _ = std::fs::remove_file(missing_marker(cache, &name));
    Ok(())
}

/// Forgets `rom`'s cached cover and marker: it is looked for again.
pub fn reset(cache: &Path, rom: &Path) {
    let name = libretro_name(rom);
    let _ = std::fs::remove_file(cached_png(cache, &name));
    let _ = std::fs::remove_file(missing_marker(cache, &name));
}

/// The user's own picture next to `rom`, which wins over the cache.
pub fn own_cover(rom: &Path) -> Option<PathBuf> {
    let own = rom.with_extension("png");
    own.is_file().then_some(own)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("md-rs-boxart-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Writes a `w` × `h` PNG of `color_type` with pixel (x, y) = f(x, y).
    fn write_png(
        path: &Path,
        w: u32,
        h: u32,
        color_type: png::ColorType,
        f: impl Fn(u32, u32) -> Vec<u8>,
    ) {
        let mut encoder = png::Encoder::new(std::fs::File::create(path).unwrap(), w, h);
        encoder.set_color(color_type);
        encoder.set_depth(png::BitDepth::Eight);
        let data: Vec<u8> = (0..h)
            .flat_map(|y| {
                let f = &f;
                (0..w).flat_map(move |x| f(x, y))
            })
            .collect();
        encoder
            .write_header()
            .unwrap()
            .write_image_data(&data)
            .unwrap();
    }

    #[test]
    fn libretro_names_and_urls() {
        assert_eq!(
            libretro_name(Path::new("/r/Sonic The Hedgehog (USA, Europe).md")),
            "Sonic The Hedgehog (USA, Europe)"
        );
        assert_eq!(
            libretro_name(Path::new("Game: Part 2 (Japan).zip")),
            "Game_ Part 2 (Japan)"
        );
        assert_eq!(libretro_name(Path::new("A&B <\"x\"> ?.bin")), "A_B __x__ _");
        assert_eq!(
            cover_url(LIBRETRO_BOXARTS, "Sonic The Hedgehog (USA, Europe)"),
            format!("{LIBRETRO_BOXARTS}/Sonic%20The%20Hedgehog%20%28USA%2C%20Europe%29.png"),
            "the URL checked against the server (200 OK)"
        );
    }

    #[test]
    fn own_picture_first_then_cache_and_missing_markers() {
        let dir = temp_dir("source");
        let cache = dir.join("cache");
        std::fs::create_dir_all(&cache).unwrap();
        let rom = dir.join("Game (USA).bin");
        let now = SystemTime::now();
        assert_eq!(cover_source(&rom, Some(&cache)), None);
        assert!(needs_download(&rom, &cache, now));
        std::fs::write(cache.join("Game (USA).png"), b"cached").unwrap();
        assert_eq!(
            cover_source(&rom, Some(&cache)),
            Some(cache.join("Game (USA).png"))
        );
        assert!(!needs_download(&rom, &cache, now));
        std::fs::write(dir.join("Game (USA).png"), b"mine").unwrap();
        assert_eq!(
            cover_source(&rom, Some(&cache)),
            Some(dir.join("Game (USA).png")),
            "own first"
        );
        // A "not found" marker holds back new requests for a week.
        let other = dir.join("Unknown.bin");
        std::fs::write(cache.join("Unknown.missing"), b"").unwrap();
        assert!(!needs_download(&other, &cache, now));
        assert!(needs_download(
            &other,
            &cache,
            now + MISSING_RETRY + Duration::from_secs(1)
        ));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn decodes_rgb_and_rgba_pngs() {
        let dir = temp_dir("png");
        let rgb = dir.join("rgb.png");
        write_png(&rgb, 3, 2, png::ColorType::Rgb, |x, y| {
            vec![x as u8 * 50, y as u8 * 100, 7]
        });
        let image = load_cover(&rgb).unwrap();
        assert_eq!((image.width, image.height), (3, 2));
        assert_eq!(
            image.pixels[3 + 2], // (x 2, y 1)
            u32::from_be_bytes([100, 100, 7, 0xFF])
        );
        let rgba = dir.join("rgba.png");
        write_png(&rgba, 1, 1, png::ColorType::Rgba, |_, _| vec![1, 2, 3, 4]);
        assert_eq!(
            load_cover(&rgba).unwrap().pixels,
            [u32::from_be_bytes([1, 2, 3, 4])]
        );
        assert_eq!(load_cover(&dir.join("absent.png")), None);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn thumbnails_keep_proportions_and_average_colours() {
        let solid = Image {
            pixels: vec![u32::from_be_bytes([10, 20, 30, 255]); 512 * 720],
            width: 512,
            height: 720,
        };
        let t = thumbnail(&solid, 128, 180);
        assert_eq!((t.width, t.height), (128, 180));
        assert!(
            t.pixels.iter().all(|&p| p == solid.pixels[0]),
            "a solid colour stays"
        );
        let wide = thumbnail(
            &Image {
                pixels: vec![0; 400 * 100],
                width: 400,
                height: 100,
            },
            128,
            180,
        );
        assert_eq!((wide.width, wide.height), (128, 32), "limited by the width");
        let quad = Image {
            pixels: [
                [0, 0, 0, 255],
                [200, 100, 0, 255],
                [0, 100, 200, 255],
                [200, 200, 200, 255],
            ]
            .map(u32::from_be_bytes)
            .to_vec(),
            width: 2,
            height: 2,
        };
        assert_eq!(
            thumbnail(&quad, 1, 1).pixels,
            [u32::from_be_bytes([100, 100, 100, 255])],
            "mean"
        );
        let small = Image {
            pixels: vec![7; 4],
            width: 2,
            height: 2,
        };
        assert_eq!(thumbnail(&small, 128, 180), small, "small enough already");
    }

    #[test]
    fn drag_ghost_is_small_and_see_through() {
        let cover = Image {
            pixels: vec![u32::from_be_bytes([200, 100, 50, 255]); 512 * 720],
            width: 512,
            height: 720,
        };
        let ghost = drag_ghost(&cover, 96, 135, 160);
        assert_eq!((ghost.width, ghost.height), (96, 135));
        assert!(
            ghost
                .pixels
                .iter()
                .all(|&p| p == u32::from_be_bytes([200, 100, 50, 160]))
        );
        let half = Image {
            pixels: vec![u32::from_be_bytes([0, 0, 0, 128])],
            width: 1,
            height: 1,
        };
        assert_eq!(
            drag_ghost(&half, 96, 135, 128).pixels[0].to_be_bytes()[3],
            64,
            "alphas multiply"
        );
    }

    #[test]
    fn thumbnailer_answers_with_or_without_a_cover() {
        let dir = temp_dir("thumbs");
        let rom = dir.join("Game.bin");
        write_png(
            &dir.join("Game.png"),
            64,
            90,
            png::ColorType::Rgb,
            |_, _| vec![5, 6, 7],
        );
        let thumbs = Thumbnailer::start(None, 32, 45);
        thumbs.request(&rom);
        let (path, image) = thumbs.done.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!(path, rom);
        let image = image.expect("own cover next to the ROM");
        assert_eq!((image.width, image.height), (32, 45));
        thumbs.request(&dir.join("Other.bin"));
        assert_eq!(
            thumbs.done.recv_timeout(Duration::from_secs(10)).unwrap().1,
            None,
            "no cover"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn index_names_are_decoded_and_sorted() {
        // Shape of the real index (thumbnails.libretro.com, 2026-11-01).
        let html = r#"<a href="../">../</a>
<a href="Streets%20of%20Rage%20(USA).png">Streets of Rage (USA).png</a>
<a href="Bare%20Knuckle%20-%20Ikari%20no%20Tekken%20~%20Streets%20of%20Rage%20(World).png">x</a>
<a href="Bare%20Knuckle%20II%20-%20Shitou%20e%20no%20Requiem%20~%20Streets%20of%20Rage%20II%20(Japan,%20Europe)%20(En,Ja).png">x</a>
<a href="Streets%20of%20Rage%20(USA).png">again</a>"#;
        assert_eq!(
            parse_index(html),
            [
                "Bare Knuckle - Ikari no Tekken ~ Streets of Rage (World)",
                "Bare Knuckle II - Shitou e no Requiem ~ Streets of Rage II (Japan, Europe) (En,Ja)",
                "Streets of Rage (USA)",
            ]
        );
        assert_eq!(percent_decode("100%"), "100%", "lone percent kept");
    }

    #[test]
    fn choose_and_reset_a_cover() {
        let dir = temp_dir("choose");
        let cache = dir.join("cache");
        let rom = dir.join("Streets of Rage (World) (En,Ja).zip");
        std::fs::create_dir_all(&cache).unwrap();
        std::fs::write(
            cache.join("Streets of Rage (World) (En,Ja).png"),
            b"automatic",
        )
        .unwrap();
        std::fs::write(cache.join("Streets of Rage (World) (En,Ja).missing"), b"").unwrap();
        let picked = dir.join("Streets of Rage (USA).png");
        std::fs::write(&picked, b"chosen").unwrap();
        choose(&cache, &rom, &picked).unwrap();
        let cached = cache.join("Streets of Rage (World) (En,Ja).png");
        assert_eq!(std::fs::read(&cached).unwrap(), b"chosen");
        assert!(
            !cache
                .join("Streets of Rage (World) (En,Ja).missing")
                .exists()
        );
        assert!(
            !cache
                .join("Streets of Rage (World) (En,Ja).png.part")
                .exists()
        );
        // The own picture next to the ROM still wins.
        std::fs::write(rom.with_extension("png"), b"mine").unwrap();
        assert_eq!(
            cover_source(&rom, Some(&cache)),
            Some(rom.with_extension("png"))
        );
        std::fs::remove_file(rom.with_extension("png")).unwrap();
        reset(&cache, &rom);
        assert_eq!(cover_source(&rom, Some(&cache)), None);
        assert!(
            needs_download(&rom, &cache, SystemTime::now()),
            "looked for again"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The index and the browsed pictures through a file:// "server".
    #[test]
    fn index_and_browser_without_network() {
        let dir = temp_dir("browse");
        let server = dir.join("server");
        std::fs::create_dir_all(&server).unwrap();
        std::fs::write(server.join("index.html"), r#"<a href="Game%20(USA).png">"#).unwrap();
        write_png(
            &server.join("Game (USA).png"),
            1,
            1,
            png::ColorType::Rgb,
            |_, _| vec![1, 2, 3],
        );
        // file:// lists no folder: the index is the server's index.html.
        let base = format!("file://{}", server.display());
        let cache = dir.join("cache");
        std::fs::create_dir_all(&cache).unwrap();
        std::fs::copy(server.join("index.html"), cache.join("index.html")).unwrap();
        let names = load_index(base.clone(), cache.clone())
            .recv_timeout(Duration::from_secs(10))
            .unwrap();
        assert_eq!(
            names,
            Some(vec!["Game (USA)".to_string()]),
            "fresh cached index"
        );
        let browser = Browser::start(base, cache.join("browse"));
        browser.request("Game (USA)");
        let (name, file) = browser.done.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!(name, "Game (USA)");
        assert_eq!(
            load_cover(&file.unwrap()).unwrap().pixels,
            [u32::from_be_bytes([1, 2, 3, 0xFF])]
        );
        browser.request("Absent (USA)");
        assert_eq!(
            browser
                .done
                .recv_timeout(Duration::from_secs(10))
                .unwrap()
                .1,
            None
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn candidate_names_drop_language_and_dump_tags() {
        assert_eq!(
            candidate_names("Streets of Rage (World) (En,Ja)"),
            [
                "Streets of Rage (Europe)",
                "Streets of Rage (World) (En,Ja)",
                "Streets of Rage (World)"
            ]
        );
        assert_eq!(
            candidate_names("Sonic The Hedgehog (USA, Europe)"),
            [
                "Sonic The Hedgehog (Europe)",
                "Sonic The Hedgehog (USA, Europe)"
            ],
            "Europe first, nothing to drop"
        );
        assert_eq!(
            candidate_names("Game (Europe) (En,Fr,De) (Rev 1) [!]"),
            [
                "Game (Europe)",
                "Game (Europe) (En,Fr,De) (Rev 1) [!]",
                "Game (Europe) (Rev 1)"
            ]
        );
        assert_eq!(candidate_names("Game (Europe)"), ["Game (Europe)"]);
        assert_eq!(candidate_names("Plain"), ["Plain (Europe)", "Plain"]);
    }

    /// Downloads from a file:// "server": a present picture lands in the
    /// cache, an absent one leaves a marker.
    #[test]
    fn downloader_without_network() {
        let dir = temp_dir("download");
        let server = dir.join("server");
        let cache = dir.join("cache");
        std::fs::create_dir_all(&server).unwrap();
        write_png(
            &server.join("Found (USA).png"),
            2,
            2,
            png::ColorType::Rgb,
            |_, _| vec![9, 9, 9],
        );
        let base = format!("file://{}", server.display());
        let downloader = Downloader::start(base, cache.clone());
        let wait = || {
            downloader
                .done
                .recv_timeout(Duration::from_secs(10))
                .expect("an answer")
        };
        downloader.request(Path::new("/roms/Found (USA).md"));
        let (rom, fetched) = wait();
        assert_eq!(rom, Path::new("/roms/Found (USA).md"));
        assert_eq!(fetched, Fetched::Saved(cache.join("Found (USA).png")));
        assert_eq!(load_cover(&cache.join("Found (USA).png")).unwrap().width, 2);
        assert_eq!(
            cover_source(Path::new("/roms/Found (USA).md"), Some(&cache)),
            Some(cache.join("Found (USA).png"))
        );
        // Found under the name without the language tag, cached under the
        // file's own name.
        downloader.request(Path::new("/roms/Found (USA) (En,Ja).zip"));
        assert_eq!(
            wait().1,
            Fetched::Saved(cache.join("Found (USA) (En,Ja).png"))
        );
        downloader.request(Path::new("/roms/Absent (USA).md"));
        assert_eq!(wait().1, Fetched::Missing);
        assert!(!needs_download(
            Path::new("/roms/Absent (USA).md"),
            &cache,
            SystemTime::now()
        ));
        assert!(
            !cache.join("Absent (USA).png.part").exists(),
            "no partial file left"
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
