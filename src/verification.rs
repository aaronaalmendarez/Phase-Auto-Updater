#![allow(dead_code)]

use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::path::Path;
use std::sync::Arc;

pub const DEFAULT_BASE_URL: &str = "https://phase.motioncore.xyz";
pub const VERSION_ENDPOINT: &str = "/plugin/version";
pub const UPDATE_STREAM_ENDPOINT: &str = "/plugin/updates";
pub const DOWNLOAD_SESSION_ENDPOINT: &str = "/plugin/download";
pub const PLUGIN_LINK_START_ENDPOINT: &str = "/api/plugin-link/start";
pub const PLUGIN_LINK_STATUS_ENDPOINT: &str = "/api/plugin-link/status";
pub const PLUGIN_LINK_ME_ENDPOINT: &str = "/api/plugin-link/me";
pub const ROBLOX_OAUTH_START_ENDPOINT: &str = "/api/plugin-link/roblox-oauth/start";
pub const ROBLOX_OAUTH_STATUS_ENDPOINT: &str = "/api/plugin-link/roblox-oauth/status";
pub const PHASE_ASSETS_ENDPOINT: &str = "/api/phase-assets";
pub const ACTIVATE_ENDPOINT: &str = "/activate";
pub const ROBLOX_PLUGIN_ASSET_ID: u64 = 130301148315515;
pub const GITHUB_LATEST_RELEASE_URL: &str =
    "https://api.github.com/repos/aaronaalmendarez/Phase-Auto-Updater/releases/latest";

#[derive(Clone, Debug)]
pub struct VerificationPlan {
    pub base_url: String,
    pub current_build_id: String,
}

impl VerificationPlan {
    pub fn new(current_build_id: impl Into<String>) -> Self {
        Self {
            base_url: DEFAULT_BASE_URL.to_owned(),
            current_build_id: current_build_id.into(),
        }
    }

    pub fn version_url(&self) -> String {
        let separator = if self.base_url.contains('?') {
            '&'
        } else {
            '?'
        };
        format!(
            "{}{}{}buildId={}",
            self.base_url.trim_end_matches('/'),
            VERSION_ENDPOINT,
            separator,
            url_escape(&self.current_build_id)
        )
    }

    pub fn update_stream_url(&self) -> String {
        format!(
            "wss://{}{}",
            self.base_url
                .trim_start_matches("https://")
                .trim_start_matches("http://")
                .trim_end_matches('/'),
            UPDATE_STREAM_ENDPOINT
        )
    }

    pub fn download_session_url(&self) -> String {
        format!(
            "{}{}",
            self.base_url.trim_end_matches('/'),
            DOWNLOAD_SESSION_ENDPOINT
        )
    }

    pub fn plugin_link_start_url(&self) -> String {
        format!(
            "{}{}",
            self.base_url.trim_end_matches('/'),
            PLUGIN_LINK_START_ENDPOINT
        )
    }

    pub fn plugin_link_status_url(&self, code: &str) -> String {
        format!(
            "{}{}/{}",
            self.base_url.trim_end_matches('/'),
            PLUGIN_LINK_STATUS_ENDPOINT,
            url_escape(code)
        )
    }

    pub fn plugin_link_me_url(&self) -> String {
        format!(
            "{}{}",
            self.base_url.trim_end_matches('/'),
            PLUGIN_LINK_ME_ENDPOINT
        )
    }

    pub fn activate_url(&self) -> String {
        format!(
            "{}{}",
            self.base_url.trim_end_matches('/'),
            ACTIVATE_ENDPOINT
        )
    }

    pub fn roblox_oauth_start_url(&self) -> String {
        format!(
            "{}{}",
            self.base_url.trim_end_matches('/'),
            ROBLOX_OAUTH_START_ENDPOINT
        )
    }

    pub fn roblox_oauth_status_url(&self, state: &str, install_id: &str) -> String {
        format!(
            "{}{}/{}?installId={}",
            self.base_url.trim_end_matches('/'),
            ROBLOX_OAUTH_STATUS_ENDPOINT,
            url_escape(state),
            url_escape(install_id)
        )
    }

    pub fn phase_themes_url(&self, page: u64) -> String {
        format!(
            "{}{}?kind=theme&sort=popular&page={}",
            self.base_url.trim_end_matches('/'),
            PHASE_ASSETS_ENDPOINT,
            page.max(1)
        )
    }

    pub fn phase_theme_install_url(&self, asset_id: &str) -> String {
        format!(
            "{}{}/{}/install",
            self.base_url.trim_end_matches('/'),
            PHASE_ASSETS_ENDPOINT,
            url_escape(asset_id)
        )
    }
}

pub fn fetch_version(plan: &VerificationPlan) -> Result<VersionResponse, String> {
    http_agent()?
        .get(&plan.version_url())
        .timeout(std::time::Duration::from_secs(10))
        .call()
        .map_err(|error| format!("Version check failed: {error}"))?
        .into_json::<VersionResponse>()
        .map_err(|error| format!("Invalid version response: {error}"))
}

pub fn listen_for_updates(plan: VerificationPlan) -> Result<UpdateStreamEvent, String> {
    let (mut socket, _) = tungstenite::connect(plan.update_stream_url())
        .map_err(|error| format!("Could not watch for updates: {error}"))?;
    loop {
        // The first frame is usually just the current state. The app only needs
        // to wake up users when the server says a real update was published.
        let message = socket
            .read()
            .map_err(|error| format!("Update watch disconnected: {error}"))?;
        if !message.is_text() {
            continue;
        }
        let event = serde_json::from_str::<UpdateStreamEvent>(
            message
                .to_text()
                .map_err(|error| format!("Invalid update notice: {error}"))?,
        )
        .map_err(|error| format!("Invalid update notice: {error}"))?;
        if event.kind == "update" {
            return Ok(event);
        }
    }
}

pub fn start_plugin_link(
    plan: &VerificationPlan,
    request: &PluginLinkStartRequest,
) -> Result<PluginLinkStartResponse, String> {
    http_agent()?
        .post(&plan.plugin_link_start_url())
        .timeout(std::time::Duration::from_secs(10))
        .send_json(ureq::json!(request))
        .map_err(|error| format!("Could not start Phase account link: {error}"))?
        .into_json::<PluginLinkStartResponse>()
        .map_err(|error| format!("Invalid link response: {error}"))
}

pub fn fetch_plugin_link_status(
    plan: &VerificationPlan,
    code: &str,
) -> Result<PluginLinkStatusResponse, String> {
    http_agent()?
        .get(&plan.plugin_link_status_url(code))
        .timeout(std::time::Duration::from_secs(10))
        .call()
        .map_err(|error| format!("Could not check Phase account link: {error}"))?
        .into_json::<PluginLinkStatusResponse>()
        .map_err(|error| format!("Invalid link status: {error}"))
}

pub fn fetch_plugin_me(
    plan: &VerificationPlan,
    plugin_token: &str,
) -> Result<PluginMeResponse, String> {
    http_agent()?
        .get(&plan.plugin_link_me_url())
        .set("authorization", &format!("Bearer {plugin_token}"))
        .timeout(std::time::Duration::from_secs(10))
        .call()
        .map_err(|error| format!("Could not refresh Phase account: {error}"))?
        .into_json::<PluginMeResponse>()
        .map_err(|error| format!("Invalid Phase account response: {error}"))
}

pub fn disconnect_plugin_me(plan: &VerificationPlan, plugin_token: &str) -> Result<(), String> {
    http_agent()?
        .delete(&plan.plugin_link_me_url())
        .set("authorization", &format!("Bearer {plugin_token}"))
        .timeout(std::time::Duration::from_secs(10))
        .call()
        .map(|_| ())
        .map_err(|error| format!("Could not disconnect Phase account: {error}"))
}

pub fn activate_install(
    plan: &VerificationPlan,
    request: &ActivationRequest,
) -> Result<ActivationResponse, String> {
    http_agent()?
        .post(&plan.activate_url())
        .timeout(std::time::Duration::from_secs(20))
        .send_json(ureq::json!(request))
        .map_err(|error| format!("Activation failed: {error}"))?
        .into_json::<ActivationResponse>()
        .map_err(|error| format!("Invalid activation response: {error}"))
}

pub fn create_download_session(
    plan: &VerificationPlan,
    request: &DownloadSessionRequest,
) -> Result<DownloadSessionResponse, String> {
    http_agent()?
        .post(&plan.download_session_url())
        .timeout(std::time::Duration::from_secs(20))
        .send_json(ureq::json!(request))
        .map_err(|error| format!("Install authorization failed: {error}"))?
        .into_json::<DownloadSessionResponse>()
        .map_err(|error| format!("Invalid install response: {error}"))
}

pub fn download_plugin_to_file(url: &str, path: &Path) -> Result<(), String> {
    download_url_to_file(url, path)
}

pub fn download_url_to_file(url: &str, path: &Path) -> Result<(), String> {
    // Write straight to disk so we don't keep a full .rbxm in memory. These
    // files are not huge today, but that assumption ages badly.
    let mut response = http_agent()?
        .get(url)
        .timeout(std::time::Duration::from_secs(120))
        .call()
        .map_err(|error| format!("Download failed: {error}"))?
        .into_reader();
    let mut file = std::fs::File::create(path)
        .map_err(|error| format!("Could not prepare update file: {error}"))?;
    std::io::copy(&mut response, &mut file)
        .map_err(|error| format!("Could not save update file: {error}"))?;
    file.flush()
        .map_err(|error| format!("Could not save update file: {error}"))
}

pub fn fetch_latest_app_update(current_version: &str) -> Result<Option<AppUpdateInfo>, String> {
    let response = match http_agent()?
        .get(GITHUB_LATEST_RELEASE_URL)
        .set("user-agent", "phase-auto-updater")
        .timeout(std::time::Duration::from_secs(10))
        .call()
    {
        Ok(response) => response,
        Err(ureq::Error::Status(404, _)) => return Ok(None),
        Err(error) => return Err(format!("App update check failed: {error}")),
    };

    let release = response
        .into_json::<GithubRelease>()
        .map_err(|error| format!("Invalid app update response: {error}"))?;

    let latest_version = release.tag_name.trim_start_matches('v').to_owned();
    if latest_version.is_empty() || !version_is_newer(&latest_version, current_version) {
        return Ok(None);
    }

    let Some(asset) = release.assets.into_iter().find(|asset| {
        let name = asset.name.to_ascii_lowercase();
        name.ends_with(".msi") && name.contains("phaseautoupdater")
    }) else {
        return Ok(None);
    };

    Ok(Some(AppUpdateInfo {
        version: latest_version,
        release_url: release.html_url,
        asset_name: asset.name,
        download_url: asset.browser_download_url,
    }))
}

pub fn start_roblox_oauth(
    plan: &VerificationPlan,
    request: &RobloxOAuthStartRequest,
) -> Result<RobloxOAuthStartResponse, String> {
    http_agent()?
        .post(&plan.roblox_oauth_start_url())
        .timeout(std::time::Duration::from_secs(10))
        .send_json(ureq::json!(request))
        .map_err(|error| format!("Could not start Roblox OAuth: {error}"))?
        .into_json::<RobloxOAuthStartResponse>()
        .map_err(|error| format!("Invalid Roblox OAuth response: {error}"))
}

pub fn fetch_roblox_oauth_status(
    plan: &VerificationPlan,
    state: &str,
    install_id: &str,
) -> Result<RobloxOAuthStatusResponse, String> {
    http_agent()?
        .get(&plan.roblox_oauth_status_url(state, install_id))
        .timeout(std::time::Duration::from_secs(10))
        .call()
        .map_err(|error| format!("Could not check Roblox OAuth: {error}"))?
        .into_json::<RobloxOAuthStatusResponse>()
        .map_err(|error| format!("Invalid Roblox OAuth status: {error}"))
}

pub fn fetch_phase_avatar_image(url: &str) -> Result<Vec<u8>, String> {
    fetch_image_bytes(&site_url(url))
}

pub fn fetch_roblox_avatar_image(user_id: &str) -> Result<Vec<u8>, String> {
    let user_id = user_id.trim();
    if user_id.is_empty() {
        return Err("Missing Roblox user ID.".to_owned());
    }

    let url = format!(
        "https://thumbnails.roblox.com/v1/users/avatar-headshot?userIds={}&size=150x150&format=Png&isCircular=true",
        url_escape(user_id)
    );
    let response = http_agent()?
        .get(&url)
        .timeout(std::time::Duration::from_secs(10))
        .call()
        .map_err(|error| format!("Could not fetch Roblox avatar: {error}"))?
        .into_json::<RobloxThumbnailResponse>()
        .map_err(|error| format!("Invalid Roblox avatar response: {error}"))?;

    let image_url = response
        .data
        .into_iter()
        .find_map(|item| item.image_url.filter(|url| !url.trim().is_empty()))
        .ok_or_else(|| "Roblox did not return an avatar image.".to_owned())?;

    fetch_image_bytes(&image_url)
}

pub fn fetch_roblox_asset_thumbnail_image(asset_id: &str) -> Result<Vec<u8>, String> {
    let asset_id = asset_id.trim();
    if asset_id.is_empty() {
        return Err("Missing Roblox background asset ID.".to_owned());
    }

    let url = format!(
        "https://thumbnails.roblox.com/v1/assets?assetIds={}&size=768x432&format=Png&isCircular=false",
        url_escape(asset_id)
    );
    let response = http_agent()?
        .get(&url)
        .timeout(std::time::Duration::from_secs(10))
        .call()
        .map_err(|error| format!("Could not fetch Roblox background: {error}"))?
        .into_json::<RobloxThumbnailResponse>()
        .map_err(|error| format!("Invalid Roblox background response: {error}"))?;

    let image_url = response
        .data
        .into_iter()
        .find_map(|item| item.image_url.filter(|url| !url.trim().is_empty()))
        .ok_or_else(|| "Roblox did not return a background image.".to_owned())?;

    fetch_image_bytes(&image_url)
}

pub fn fetch_phase_themes(plan: &VerificationPlan) -> Result<Vec<PhaseThemeAsset>, String> {
    let mut page = 1;
    let mut themes = Vec::new();

    loop {
        let response = http_agent()?
            .get(&plan.phase_themes_url(page))
            .timeout(std::time::Duration::from_secs(10))
            .call()
            .map_err(|error| format!("Could not load Phase themes: {error}"))?
            .into_json::<PhaseThemeListResponse>()
            .map_err(|error| format!("Invalid Phase themes response: {error}"))?;

        themes.extend(
            response
                .assets
                .into_iter()
                .filter(|asset| asset.kind == "theme" && asset.has_theme_code),
        );

        if page >= response.pages.max(1) {
            break;
        }
        page += 1;
    }

    Ok(themes)
}

pub fn install_phase_theme(
    plan: &VerificationPlan,
    asset_id: &str,
) -> Result<PhaseThemeInstallResponse, String> {
    http_agent()?
        .post(&plan.phase_theme_install_url(asset_id))
        .timeout(std::time::Duration::from_secs(12))
        .call()
        .map_err(|error| format!("Could not prepare Phase theme: {error}"))?
        .into_json::<PhaseThemeInstallResponse>()
        .map_err(|error| format!("Invalid Phase theme response: {error}"))
}

fn fetch_image_bytes(url: &str) -> Result<Vec<u8>, String> {
    let mut response = http_agent()?
        .get(url)
        .timeout(std::time::Duration::from_secs(10))
        .call()
        .map_err(|error| format!("Could not fetch avatar image: {error}"))?
        .into_reader();
    let mut bytes = Vec::new();
    response
        .read_to_end(&mut bytes)
        .map_err(|error| format!("Could not read avatar image: {error}"))?;
    Ok(bytes)
}

fn site_url(url: &str) -> String {
    let value = url.trim();
    if value.starts_with("http://") || value.starts_with("https://") {
        value.to_owned()
    } else if value.starts_with('/') {
        format!("{}{}", DEFAULT_BASE_URL.trim_end_matches('/'), value)
    } else {
        value.to_owned()
    }
}

fn http_agent() -> Result<ureq::Agent, String> {
    // native-tls keeps Windows/macOS using the OS trust store. rustls would be
    // fine too, but it needs a little more release packaging care.
    let connector = ureq::native_tls::TlsConnector::new()
        .map_err(|error| format!("Native TLS setup failed: {error}"))?;
    Ok(ureq::AgentBuilder::new()
        .tls_connector(Arc::new(connector))
        .build())
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionResponse {
    pub ok: bool,
    pub product: String,
    pub latest_version: String,
    pub latest_build_id: String,
    pub minimum_build_id: String,
    pub current_build_id: Option<String>,
    pub update_required: bool,
    pub required: bool,
    pub blocked: bool,
    pub download_available: bool,
    pub storage: Option<String>,
    pub message: String,
    pub notes: String,
    pub release: Option<ReleaseInfo>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseInfo {
    pub build_id: String,
    pub version: String,
    pub size: u64,
    pub sha256: String,
    pub created_at: u64,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateStreamEvent {
    #[serde(rename = "type")]
    pub kind: String,
    pub latest_version: Option<String>,
    pub latest_build_id: Option<String>,
    pub required: Option<bool>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginLinkStartRequest {
    pub roblox_user_id: String,
    pub install_id: String,
    pub build_id: String,
    pub product: String,
    pub version: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginLinkStartResponse {
    pub code: String,
    pub status: String,
    pub expires_at: String,
    pub verify_url: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginLinkStatusResponse {
    pub status: String,
    pub plugin_token: Option<String>,
    pub user: Option<LinkedUser>,
    pub access_status: Option<String>,
    pub roblox_user_id: Option<String>,
    pub activation_mode: Option<String>,
    pub install_id: Option<String>,
    pub token: Option<String>,
    pub licensee: Option<String>,
    pub message: Option<String>,
    pub expires_at: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkedUser {
    pub username: String,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivationRequest {
    pub activation_mode: String,
    pub license_key: Option<String>,
    pub user_id: u64,
    pub install_id: String,
    pub asset_id: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivationResponse {
    pub ok: bool,
    pub active: bool,
    pub activation_mode: String,
    pub product: String,
    pub user_id: u64,
    pub install_id: String,
    pub asset_id: Option<u64>,
    pub token: String,
    pub expires_at: u64,
    pub licensee: String,
    pub message: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginMeResponse {
    pub user: LinkedUser,
    pub plugin_linked: bool,
    pub plugin_session: Option<PluginSessionInfo>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhaseThemeListResponse {
    pub assets: Vec<PhaseThemeAsset>,
    #[serde(default = "default_page_count")]
    pub pages: u64,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhaseThemeInstallResponse {
    pub asset: PhaseThemeAsset,
    pub theme_code: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhaseThemeAsset {
    #[serde(rename = "_id")]
    pub id: String,
    pub title: String,
    pub description: String,
    pub kind: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub owner: Option<PhaseThemeOwner>,
    #[serde(default)]
    pub theme_preview: PhaseThemePreview,
    #[serde(default)]
    pub theme_preview_image_url: String,
    #[serde(default)]
    pub has_theme_code: bool,
    #[serde(default)]
    pub install_count: u64,
    #[serde(default)]
    pub download_count: u64,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhaseThemePreview {
    #[serde(default)]
    pub background: String,
    #[serde(default)]
    pub panel: String,
    #[serde(default)]
    pub accent: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhaseThemeOwner {
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub display_name: Option<String>,
}

fn default_page_count() -> u64 {
    1
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginSessionInfo {
    pub id: String,
    pub build_id: Option<String>,
    pub version: Option<String>,
    pub linked_at: Option<String>,
    pub last_seen_at: Option<String>,
    pub roblox_user_id: Option<String>,
    pub activation_mode: Option<String>,
    pub install_id: Option<String>,
    pub activation_token: Option<String>,
    pub licensee: Option<String>,
    pub message: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RobloxOAuthStartRequest {
    pub install_id: String,
    pub build_id: String,
    pub product: String,
    pub version: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RobloxOAuthStartResponse {
    pub ok: bool,
    pub status: String,
    pub state: String,
    pub expires_at: String,
    pub url: String,
    pub asset_id: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RobloxOAuthStatusResponse {
    pub ok: bool,
    pub status: String,
    pub roblox_user_id: Option<String>,
    pub roblox_username: Option<String>,
    pub asset_id: Option<String>,
    pub activation_mode: Option<String>,
    pub install_id: Option<String>,
    pub token: Option<String>,
    pub expires_at: Option<String>,
    pub licensee: Option<String>,
    pub message: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RobloxThumbnailResponse {
    data: Vec<RobloxThumbnail>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RobloxThumbnail {
    image_url: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadSessionRequest {
    pub activation_mode: String,
    pub user_id: u64,
    pub install_id: String,
    pub asset_id: Option<u64>,
    pub license_key: Option<String>,
    pub token: String,
    pub build_id: String,
    pub target_build_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadSessionResponse {
    pub ok: bool,
    pub product: String,
    pub build_id: String,
    pub version: String,
    pub size: u64,
    pub sha256: String,
    pub expires_at: u64,
    pub ttl_seconds: u64,
    pub max_uses: u64,
    pub download_url: String,
    pub message: String,
}

#[derive(Clone, Debug)]
pub struct AppUpdateInfo {
    pub version: String,
    pub release_url: String,
    pub asset_name: String,
    pub download_url: String,
}

#[derive(Clone, Debug, Deserialize)]
struct GithubRelease {
    tag_name: String,
    html_url: String,
    assets: Vec<GithubReleaseAsset>,
}

#[derive(Clone, Debug, Deserialize)]
struct GithubReleaseAsset {
    name: String,
    browser_download_url: String,
}

fn version_is_newer(latest: &str, current: &str) -> bool {
    let latest_parts = version_parts(latest);
    let current_parts = version_parts(current);
    latest_parts > current_parts
}

fn version_parts(value: &str) -> Vec<u64> {
    value
        .split(['.', '-', '+'])
        .map(|part| part.parse::<u64>().unwrap_or(0))
        .collect()
}

fn url_escape(value: &str) -> String {
    value
        .bytes()
        .flat_map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                vec![byte as char]
            }
            _ => format!("%{byte:02X}").chars().collect(),
        })
        .collect()
}
