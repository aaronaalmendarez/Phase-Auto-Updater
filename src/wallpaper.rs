//! Animated theme wallpapers: sprite sheets played as a flipbook.
//!
//! A wallpaper is a set of 1024x1024 sheets, each holding a grid of frames.
//! Playback picks the frame from elapsed time and draws that region of the
//! sheet, so there is one texture upload per sheet and no per-frame work
//! beyond choosing UV coordinates.
//!
//! Wallpapers are not bundled. The catalog and thumbnails are fetched from
//! the Phase server, and a wallpaper's sheets are downloaded the first time
//! it is picked, then kept in the local cache.
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, Context, Pos2, Rect, TextureHandle, TextureOptions};
use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WallpaperEntry {
    pub id: String,
    pub title: String,
    pub frame_width: u32,
    pub frame_height: u32,
    pub columns: u32,
    pub rows: u32,
    pub frame_count: u32,
    pub fps: f32,
    /// Playback order when frames repeat (a ping-pong loop stores each frame
    /// once). Empty means play frames in order.
    #[serde(default)]
    pub sequence: Vec<u32>,
    pub sheets: Vec<SheetEntry>,
    pub color: ColorProfile,
    /// "Pixelated" for pixel art (scaled with hard edges), else "Default".
    #[serde(default)]
    pub resample: String,
    /// Each animated wallpaper is a complete theme with its own palette.
    pub theme: Option<WallpaperTheme>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct WallpaperTheme {
    pub name: String,
    pub palette: serde_json::Map<String, serde_json::Value>,
}

impl WallpaperEntry {
    /// The wallpaper's palette as a PA2 theme code.
    pub fn theme_code(&self) -> Option<String> {
        let theme = self.theme.as_ref()?;
        let payload = serde_json::json!({ "palette": theme.palette });
        Some(format!("PA2|{}|{payload}", theme.name.replace('|', " ")))
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct SheetEntry {
    pub file: String,
    /// Content hash (first 16 hex digits of SHA-256). Sheets are cached and
    /// requested by hash, so a re-published sheet is never served stale.
    #[serde(default)]
    pub hash: Option<String>,
    /// File size, for download progress.
    #[serde(default)]
    pub bytes: Option<u64>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ColorProfile {
    pub recommended_scrim: f32,
    pub palette: Vec<String>,
}

#[derive(Deserialize)]
struct Registry {
    wallpapers: Vec<WallpaperEntry>,
}

/// Where the Companion downloads wallpapers from (theme-loops/ship.py publishes here).
const SERVER: &str = "https://phase.motioncore.xyz/wallpapers/v1/";
/// Largest file accepted from the server; sheets are about 1 MB.
const MAX_DOWNLOAD: u64 = 16 * 1024 * 1024;

/// Where wallpapers come from.
#[derive(Clone, Debug)]
pub enum Library {
    /// The Phase server, with everything downloaded kept in `cache`.
    Remote { cache: PathBuf },
    /// A working folder laid out as `registry.json` + `wallpapers/<id>/sheet-N.png`,
    /// for trying new wallpapers without publishing them (`PHASE_WALLPAPER_DIR`).
    Folder(PathBuf),
}

impl Library {
    pub fn open() -> Self {
        if let Some(dir) = std::env::var_os("PHASE_WALLPAPER_DIR") {
            return Self::Folder(PathBuf::from(dir));
        }
        let mut cache = dirs::cache_dir().unwrap_or_else(std::env::temp_dir);
        cache.push("Phase");
        cache.push("Phase Animator Installer");
        cache.push("wallpapers");
        Self::Remote { cache }
    }

    /// The catalog as last downloaded (or from the folder). Never touches
    /// the network, so it is safe to call on the UI thread.
    pub fn cached_catalog(&self) -> Vec<WallpaperEntry> {
        let path = match self {
            Self::Remote { cache } => cache.join("registry.json"),
            Self::Folder(root) => root.join("registry.json"),
        };
        std::fs::read(path)
            .ok()
            .map(|bytes| parse_catalog(&bytes))
            .unwrap_or_default()
    }

    /// The latest catalog from the server, cached for next time. Falls back
    /// to the cached copy when offline. Blocking.
    pub fn fetch_catalog(&self) -> Result<Vec<WallpaperEntry>, String> {
        let Self::Remote { cache } = self else {
            return Ok(self.cached_catalog());
        };
        match download(&format!("{SERVER}registry.json")) {
            Ok(bytes) if serde_json::from_slice::<Registry>(&bytes).is_ok() => {
                write_atomic(&cache.join("registry.json"), &bytes);
                Ok(parse_catalog(&bytes))
            }
            Ok(_) => Err("The animated theme list from the server is invalid.".to_owned()),
            Err(error) => {
                let cached = self.cached_catalog();
                if cached.is_empty() {
                    Err(error)
                } else {
                    Ok(cached)
                }
            }
        }
    }

    /// A small picture of the wallpaper for pickers. Blocking.
    pub fn thumbnail(&self, entry: &WallpaperEntry) -> Option<egui::ColorImage> {
        let image = match self {
            Self::Remote { cache } => {
                let path = cache.join(&entry.id).join("thumb.png");
                let bytes = match std::fs::read(&path) {
                    Ok(bytes) => bytes,
                    Err(_) => {
                        let bytes = download(&format!("{SERVER}{}/thumb.png", entry.id)).ok()?;
                        write_atomic(&path, &bytes);
                        bytes
                    }
                };
                image::load_from_memory(&bytes).ok()?
            }
            Self::Folder(_) => {
                // A working folder has no thumbnail file; use the first frame.
                let sheet = self.sheet(entry, entry.sheets.first()?).ok()?;
                let frame =
                    image::imageops::crop_imm(&sheet, 0, 0, entry.frame_width, entry.frame_height);
                image::DynamicImage::ImageRgba8(frame.to_image())
            }
        };
        Some(color_image(image.thumbnail(192, 108).to_rgba8()))
    }

    /// Total download size of the sheets not cached yet, in bytes, when the
    /// registry lists sizes. Zero means the wallpaper loads from the cache.
    pub fn missing_bytes(&self, entry: &WallpaperEntry) -> u64 {
        let Self::Remote { cache } = self else {
            return 0;
        };
        entry
            .sheets
            .iter()
            .filter(|sheet| !cached_sheet(cache, entry, sheet).is_file())
            .map(|sheet| sheet.bytes.unwrap_or(0))
            .sum()
    }

    /// Every sheet of `entry`, decoded, downloading the ones not cached yet.
    /// `progress` receives the bytes downloaded so far. Blocking.
    pub fn sheets(
        &self,
        entry: &WallpaperEntry,
        progress: &dyn Fn(u64),
    ) -> Result<Vec<egui::ColorImage>, String> {
        let mut done = 0;
        let mut images = Vec::with_capacity(entry.sheets.len());
        for sheet in &entry.sheets {
            if let Self::Remote { cache } = self {
                let path = cached_sheet(cache, entry, sheet);
                if !path.is_file() {
                    let bytes = download(&sheet_url(entry, sheet))?;
                    if sheet
                        .hash
                        .as_deref()
                        .is_some_and(|hash| hash != content_hash(&bytes))
                    {
                        return Err(format!(
                            "{} did not download correctly. Try again.",
                            entry.title
                        ));
                    }
                    done += bytes.len() as u64;
                    progress(done);
                    write_atomic(&path, &bytes);
                }
            }
            images.push(color_image(self.sheet(entry, sheet)?));
        }
        if let Self::Remote { cache } = self {
            remove_stale_sheets(cache, entry);
        }
        Ok(images)
    }

    fn sheet(
        &self,
        entry: &WallpaperEntry,
        sheet: &SheetEntry,
    ) -> Result<image::RgbaImage, String> {
        let path = match self {
            Self::Remote { cache } => cached_sheet(cache, entry, sheet),
            Self::Folder(root) => root.join("wallpapers").join(&entry.id).join(&sheet.file),
        };
        image::open(&path)
            .map(|image| image.to_rgba8())
            .map_err(|error| format!("Could not load {} ({}): {error}", entry.title, sheet.file))
    }
}

fn parse_catalog(bytes: &[u8]) -> Vec<WallpaperEntry> {
    serde_json::from_slice::<Registry>(bytes)
        .map(|registry| registry.wallpapers)
        .unwrap_or_default()
        .into_iter()
        .filter(|entry| entry.theme.is_some())
        .collect()
}

fn sheet_url(entry: &WallpaperEntry, sheet: &SheetEntry) -> String {
    match &sheet.hash {
        Some(hash) => format!("{SERVER}{}/{}?v={hash}", entry.id, sheet.file),
        None => format!("{SERVER}{}/{}", entry.id, sheet.file),
    }
}

fn cached_sheet(cache: &Path, entry: &WallpaperEntry, sheet: &SheetEntry) -> PathBuf {
    let name = match &sheet.hash {
        Some(hash) => format!("{hash}.png"),
        None => sheet.file.clone(),
    };
    cache.join(&entry.id).join(name)
}

/// Deletes cached sheets a re-published wallpaper no longer uses.
fn remove_stale_sheets(cache: &Path, entry: &WallpaperEntry) {
    let keep: Vec<PathBuf> = entry
        .sheets
        .iter()
        .map(|sheet| cached_sheet(cache, entry, sheet))
        .collect();
    let Ok(files) = std::fs::read_dir(cache.join(&entry.id)) else {
        return;
    };
    for file in files.flatten().map(|file| file.path()) {
        let is_thumb = file.file_name().is_some_and(|name| name == "thumb.png");
        if !is_thumb && !keep.contains(&file) {
            let _ = std::fs::remove_file(file);
        }
    }
}

fn content_hash(bytes: &[u8]) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(bytes)
        .iter()
        .take(8)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn download(url: &str) -> Result<Vec<u8>, String> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(Duration::from_secs(30))
        .build();
    let response = agent
        .get(url)
        .call()
        .map_err(|error| format!("Could not download animated theme: {error}"))?;
    let mut bytes = Vec::new();
    response
        .into_reader()
        .take(MAX_DOWNLOAD)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("Could not download animated theme: {error}"))?;
    Ok(bytes)
}

/// Writes through a temporary file so an interrupted download never leaves
/// a truncated file that looks cached.
fn write_atomic(path: &Path, bytes: &[u8]) {
    let Some(dir) = path.parent() else {
        return;
    };
    let temp = path.with_extension("part");
    let written = std::fs::create_dir_all(dir).is_ok() && std::fs::write(&temp, bytes).is_ok();
    if !written || std::fs::rename(&temp, path).is_err() {
        let _ = std::fs::remove_file(&temp);
    }
}

fn color_image(image: image::RgbaImage) -> egui::ColorImage {
    let size = [image.width() as usize, image.height() as usize];
    egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw())
}

pub struct AnimatedWallpaper {
    pub entry: WallpaperEntry,
    sheets: Vec<TextureHandle>,
    started: Instant,
    /// Holds one frame (for screenshots) instead of playing.
    pub frozen_frame: Option<u32>,
}

impl AnimatedWallpaper {
    /// Uploads decoded sheets (from [`Library::sheets`]) as textures.
    pub fn new(ctx: &Context, entry: WallpaperEntry, images: Vec<egui::ColorImage>) -> Self {
        let filter = if entry.resample == "Pixelated" {
            TextureOptions::NEAREST
        } else {
            TextureOptions::LINEAR
        };
        let sheets = images
            .into_iter()
            .enumerate()
            .map(|(n, image)| {
                ctx.load_texture(format!("wallpaper-{}-{n}", entry.id), image, filter)
            })
            .collect();
        Self {
            entry,
            sheets,
            started: Instant::now(),
            frozen_frame: None,
        }
    }

    pub fn current_frame(&self) -> u32 {
        let sequence = &self.entry.sequence;
        let length = if sequence.is_empty() {
            self.entry.frame_count
        } else {
            sequence.len() as u32
        };
        let step = self.frozen_frame.unwrap_or_else(|| {
            (self.started.elapsed().as_secs_f32() * self.entry.fps) as u32 % length.max(1)
        });
        sequence.get(step as usize).copied().unwrap_or(step)
    }

    /// Texture and UV rect for `frame`.
    pub fn frame(&self, frame: u32) -> Option<(&TextureHandle, Rect)> {
        let per_sheet = self.entry.columns * self.entry.rows;
        let texture = self.sheets.get((frame / per_sheet) as usize)?;
        let local = frame % per_sheet;
        let [sheet_w, sheet_h] = texture.size().map(|v| v as f32);
        let x = (local % self.entry.columns * self.entry.frame_width) as f32;
        let y = (local / self.entry.columns * self.entry.frame_height) as f32;
        let uv = Rect::from_min_max(
            Pos2::new(x / sheet_w, y / sheet_h),
            Pos2::new(
                (x + self.entry.frame_width as f32) / sheet_w,
                (y + self.entry.frame_height as f32) / sheet_h,
            ),
        );
        Some((texture, uv))
    }

    /// Frame size, for crop/fit layout.
    pub fn frame_size(&self) -> egui::Vec2 {
        egui::vec2(
            self.entry.frame_width as f32,
            self.entry.frame_height as f32,
        )
    }

    /// Seconds until the next frame should be shown.
    pub fn frame_interval(&self) -> std::time::Duration {
        std::time::Duration::from_secs_f32(1.0 / self.entry.fps.max(1.0))
    }

    /// Dominant color, used to tint UI surfaces toward the wallpaper.
    pub fn tint(&self) -> Option<Color32> {
        let hex = self.entry.color.palette.first()?.trim_start_matches('#');
        let value = u32::from_str_radix(hex, 16).ok()?;
        Some(Color32::from_rgb(
            (value >> 16) as u8,
            (value >> 8) as u8,
            value as u8,
        ))
    }
}
