use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FolderHealth {
    Ready,
    Missing,
    Empty,
}

#[derive(Clone, Debug)]
pub struct PluginFolderCandidate {
    pub path: PathBuf,
    pub source: String,
    pub health: FolderHealth,
    pub plugin_files: Vec<PluginFile>,
}

#[derive(Clone, Debug)]
pub struct PluginFile {
    pub path: PathBuf,
    pub size_bytes: u64,
    pub modified: Option<SystemTime>,
}

pub fn detect_plugin_folders() -> Vec<PluginFolderCandidate> {
    let mut paths = Vec::<(PathBuf, String)>::new();

    // Roblox has moved this around a few times. Keep the guesses predictable
    // and let the manual picker handle unusual installs.
    #[cfg(target_os = "windows")]
    {
        if let Ok(local_app_data) = env::var("LOCALAPPDATA") {
            paths.push((
                PathBuf::from(local_app_data).join("Roblox").join("Plugins"),
                "%LOCALAPPDATA%\\Roblox\\Plugins".to_owned(),
            ));
        }

        if let Some(document_dir) = dirs::document_dir() {
            paths.push((
                document_dir.join("Roblox").join("Plugins"),
                "Documents\\Roblox\\Plugins fallback".to_owned(),
            ));
        }
    }

    #[cfg(target_os = "macos")]
    {
        // Studio still tends to use Documents for local plugins on macOS, but
        // the Application Support fallback helps when users have newer installs.
        if let Some(document_dir) = dirs::document_dir() {
            paths.push((
                document_dir.join("Roblox").join("Plugins"),
                "~/Documents/Roblox/Plugins".to_owned(),
            ));
        }

        if let Some(data_dir) = dirs::data_dir() {
            paths.push((
                data_dir.join("Roblox").join("Plugins"),
                "~/Library/Application Support/Roblox/Plugins fallback".to_owned(),
            ));
        }
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        if let Some(data_dir) = dirs::data_dir() {
            paths.push((
                data_dir.join("Roblox").join("Plugins"),
                "XDG data Roblox/Plugins fallback".to_owned(),
            ));
        }
    }

    dedupe_paths(paths)
        .into_iter()
        .map(|(path, source)| inspect_candidate(path, source))
        .collect()
}

pub fn best_candidate(candidates: &[PluginFolderCandidate]) -> Option<PluginFolderCandidate> {
    candidates
        .iter()
        .find(|candidate| candidate.health == FolderHealth::Ready)
        .or_else(|| {
            candidates
                .iter()
                .find(|candidate| candidate.health == FolderHealth::Empty)
        })
        .or_else(|| candidates.first())
        .cloned()
}

pub fn inspect_candidate(path: PathBuf, source: String) -> PluginFolderCandidate {
    let plugin_files = discover_plugin_files(&path);
    let health = if !path.exists() {
        FolderHealth::Missing
    } else if plugin_files.is_empty() {
        FolderHealth::Empty
    } else {
        FolderHealth::Ready
    };

    PluginFolderCandidate {
        path,
        source,
        health,
        plugin_files,
    }
}

fn dedupe_paths(paths: Vec<(PathBuf, String)>) -> Vec<(PathBuf, String)> {
    let mut deduped = Vec::<(PathBuf, String)>::new();

    for (path, source) in paths {
        let normalized = normalize_path_for_compare(&path);
        if deduped
            .iter()
            .any(|(existing, _)| normalize_path_for_compare(existing) == normalized)
        {
            continue;
        }
        deduped.push((path, source));
    }

    deduped
}

fn normalize_path_for_compare(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/").to_lowercase()
}

fn discover_plugin_files(path: &Path) -> Vec<PluginFile> {
    let Ok(entries) = fs::read_dir(path) else {
        return Vec::new();
    };

    let mut files: Vec<PluginFile> = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let extension = path.extension()?.to_string_lossy().to_lowercase();
            if extension != "rbxm" && extension != "rbxmx" {
                return None;
            }

            let metadata = entry.metadata().ok()?;
            Some(PluginFile {
                path,
                size_bytes: metadata.len(),
                modified: metadata.modified().ok(),
            })
        })
        .collect();

    files.sort_by(|a, b| a.path.file_name().cmp(&b.path.file_name()));
    files
}
