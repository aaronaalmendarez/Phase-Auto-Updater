#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod detector;
mod verification;

use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use detector::{
    FolderHealth, PluginFile, PluginFolderCandidate, best_candidate, detect_plugin_folders,
    inspect_candidate,
};
use eframe::egui::{
    self, Align2, Color32, ColorImage, Context, FontData, FontFamily, FontId, IconData, Margin,
    Pos2, Rect, RichText, Rounding, Sense, Stroke, TextureHandle, TextureOptions, Ui, Vec2,
};

const APP_NAME: &str = "Phase Animator Installer";
const CURRENT_BUILD_ID: &str = "phase-2026-05-18-custom";
const PHOSPHOR_FONT: &str = "phosphor-icons";
const APP_WIDTH: f32 = 450.0;
const CONTENT_WIDTH: f32 = 410.0;
const SCROLL_BODY_WIDTH: f32 = 422.0;
const CARD_WIDTH: f32 = 410.0;
const CARD_INNER_WIDTH: f32 = 394.0;
const THEME_ROW_WIDTH: f32 = CARD_INNER_WIDTH;
const THEME_ROW_MARGIN: f32 = 12.0;
const THEME_ROW_INNER_WIDTH: f32 = THEME_ROW_WIDTH - THEME_ROW_MARGIN * 2.0;

fn main() -> eframe::Result<()> {
    if std::env::args().any(|arg| arg == "--smoke-test") {
        if run_smoke_test().is_err() {
            std::process::exit(1);
        }
        return Ok(());
    }

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([APP_WIDTH, 620.0])
            .with_min_inner_size([320.0, 520.0])
            .with_title(APP_NAME)
            .with_icon(load_window_icon()),
        ..Default::default()
    };

    eframe::run_native(
        APP_NAME,
        native_options,
        Box::new(|cc| Box::new(PhaseInstallerApp::new(cc))),
    )
}

fn run_smoke_test() -> Result<(), String> {
    if include_bytes!("../assets/PhaseAnimator.png").is_empty() {
        return Err("Missing PhaseAnimator.png".to_owned());
    }
    if include_bytes!("../assets/Phosphor.ttf").is_empty() {
        return Err("Missing Phosphor.ttf".to_owned());
    }

    let _folders = detect_plugin_folders();
    let plan = verification::VerificationPlan::new(CURRENT_BUILD_ID);
    if !plan.version_url().starts_with("https://") {
        return Err("Invalid version URL".to_owned());
    }
    if !plan.update_stream_url().starts_with("wss://") {
        return Err("Invalid update stream URL".to_owned());
    }
    let _ = install_id();
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum InstallPhase {
    Idle,
    Checking,
    Ready,
    Downloading,
    Installing,
    Complete,
    Error,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ViewTab {
    Install,
    Account,
    Folders,
    Options,
}

struct ActivityLine {
    color: Color32,
    text: String,
}

#[derive(Clone, Copy)]
enum AvatarKind {
    Phase,
    Roblox,
}

struct AvatarFetchResult {
    kind: AvatarKind,
    key: String,
    image: Result<ColorImage, String>,
}

struct ThemeBackgroundFetchResult {
    key: String,
    image: Result<ColorImage, String>,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ThemeSelection {
    asset_id: String,
    title: String,
    theme_code: String,
    background_image_id: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
enum ThemeBackgroundMode {
    Fit,
    Stretch,
    #[default]
    Crop,
}

impl ThemeBackgroundMode {
    fn label(self) -> &'static str {
        match self {
            ThemeBackgroundMode::Crop => "Crop",
            ThemeBackgroundMode::Fit => "Fit",
            ThemeBackgroundMode::Stretch => "Stretch",
        }
    }
}

struct InstallOutcome {
    target_path: PathBuf,
    backup_path: Option<PathBuf>,
    version: String,
}

enum InstallEvent {
    Progress {
        phase: InstallPhase,
        color: Color32,
        message: String,
        progress: f32,
    },
    Finished(Result<InstallOutcome, String>),
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct AccountCache {
    plugin_token: Option<String>,
    linked_user: Option<verification::LinkedUser>,
    roblox_user_id: String,
    roblox_username: Option<String>,
    activation: Option<verification::ActivationResponse>,
    #[serde(default)]
    selected_theme: Option<ThemeSelection>,
    #[serde(default)]
    theme_background_mode: ThemeBackgroundMode,
}

struct PhaseInstallerApp {
    logo: Option<TextureHandle>,
    phase_avatar: Option<TextureHandle>,
    phase_avatar_key: Option<String>,
    roblox_avatar: Option<TextureHandle>,
    roblox_avatar_key: Option<String>,
    avatar_tx: Sender<AvatarFetchResult>,
    avatar_rx: Receiver<AvatarFetchResult>,
    theme_background: Option<TextureHandle>,
    theme_background_key: Option<String>,
    theme_background_tx: Sender<ThemeBackgroundFetchResult>,
    theme_background_rx: Receiver<ThemeBackgroundFetchResult>,
    candidates: Vec<PluginFolderCandidate>,
    selected_folder: Option<PathBuf>,
    release: Option<verification::VersionResponse>,
    release_error: Option<String>,
    release_rx: Option<Receiver<Result<verification::VersionResponse, String>>>,
    update_stream_rx: Option<Receiver<Result<verification::UpdateStreamEvent, String>>>,
    link_code: Option<String>,
    link_url: Option<String>,
    link_expires_at: Option<String>,
    link_rx: Option<Receiver<Result<verification::PluginLinkStartResponse, String>>>,
    link_status_rx: Option<Receiver<Result<verification::PluginLinkStatusResponse, String>>>,
    account_refresh_rx: Option<Receiver<Result<verification::PluginMeResponse, String>>>,
    phase_disconnect_rx: Option<Receiver<Result<(), String>>>,
    app_update_rx: Option<Receiver<Result<Option<verification::AppUpdateInfo>, String>>>,
    app_update_install_rx: Option<Receiver<Result<PathBuf, String>>>,
    app_update: Option<verification::AppUpdateInfo>,
    app_update_error: Option<String>,
    theme_assets: Vec<verification::PhaseThemeAsset>,
    theme_fetch_rx: Option<Receiver<Result<Vec<verification::PhaseThemeAsset>, String>>>,
    theme_apply_rx: Option<Receiver<Result<ThemeSelection, String>>>,
    theme_error: Option<String>,
    selected_theme: Option<ThemeSelection>,
    theme_search: String,
    visible_theme_count: usize,
    theme_background_mode: ThemeBackgroundMode,
    last_link_poll: Option<Instant>,
    linked_user: Option<verification::LinkedUser>,
    plugin_token: Option<String>,
    license_key: String,
    roblox_user_id: String,
    roblox_username: Option<String>,
    roblox_oauth_state: Option<String>,
    roblox_oauth_url: Option<String>,
    roblox_oauth_expires_at: Option<String>,
    roblox_oauth_rx: Option<Receiver<Result<verification::RobloxOAuthStartResponse, String>>>,
    roblox_oauth_status_rx:
        Option<Receiver<Result<verification::RobloxOAuthStatusResponse, String>>>,
    last_roblox_oauth_poll: Option<Instant>,
    activation_rx: Option<Receiver<Result<verification::ActivationResponse, String>>>,
    install_rx: Option<Receiver<InstallEvent>>,
    activation: Option<verification::ActivationResponse>,
    activation_error: Option<String>,
    backup_before_install: bool,
    restart_studio_hint: bool,
    phase: InstallPhase,
    active_tab: ViewTab,
    progress: f32,
    phase_started_at: Option<Instant>,
    activity: Vec<ActivityLine>,

    // Tiny bit of animation state for the tab bar. Kept here because splitting
    // the egui view code too early made the first UI pass harder to tune.
    tab_lerp: f32,
    milestone: u32,
}

impl PhaseInstallerApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        configure_style(&cc.egui_ctx);

        let candidates = detect_plugin_folders();
        let selected_folder = best_candidate(&candidates).map(|candidate| candidate.path);
        let (avatar_tx, avatar_rx) = mpsc::channel();
        let (theme_background_tx, theme_background_rx) = mpsc::channel();

        let mut app = Self {
            logo: load_logo(&cc.egui_ctx),
            phase_avatar: None,
            phase_avatar_key: None,
            roblox_avatar: None,
            roblox_avatar_key: None,
            avatar_tx,
            avatar_rx,
            theme_background: None,
            theme_background_key: None,
            theme_background_tx,
            theme_background_rx,
            candidates,
            selected_folder,
            release: None,
            release_error: None,
            release_rx: None,
            update_stream_rx: None,
            link_code: None,
            link_url: None,
            link_expires_at: None,
            link_rx: None,
            link_status_rx: None,
            account_refresh_rx: None,
            phase_disconnect_rx: None,
            app_update_rx: None,
            app_update_install_rx: None,
            app_update: None,
            app_update_error: None,
            theme_assets: Vec::new(),
            theme_fetch_rx: None,
            theme_apply_rx: None,
            theme_error: None,
            selected_theme: None,
            theme_search: String::new(),
            visible_theme_count: 6,
            theme_background_mode: ThemeBackgroundMode::Crop,
            last_link_poll: None,
            linked_user: None,
            plugin_token: None,
            license_key: String::new(),
            roblox_user_id: String::new(),
            roblox_username: None,
            roblox_oauth_state: None,
            roblox_oauth_url: None,
            roblox_oauth_expires_at: None,
            roblox_oauth_rx: None,
            roblox_oauth_status_rx: None,
            last_roblox_oauth_poll: None,
            activation_rx: None,
            install_rx: None,
            activation: None,
            activation_error: None,
            backup_before_install: true,
            restart_studio_hint: true,
            phase: InstallPhase::Idle,
            active_tab: ViewTab::Install,
            progress: 0.0,
            phase_started_at: None,
            activity: Vec::new(),
            tab_lerp: 0.0,
            milestone: 0,
        };

        app.load_cached_accounts(&cc.egui_ctx);

        app.log(
            phase::blue(),
            "Detected local Roblox Studio plugin folders.",
        );
        if let Some(path) = app.selected_folder.clone() {
            app.log(
                phase::green(),
                format!("Install location: {}", compact_path(&path, 30)),
            );
        } else {
            app.log(
                phase::warning(),
                "Choose a Roblox Studio plugin folder to continue.",
            );
        }
        app.begin_version_check(Some(cc.egui_ctx.clone()));
        app.begin_update_stream(&cc.egui_ctx);
        app.begin_phase_account_refresh(&cc.egui_ctx);
        app.begin_app_update_check(&cc.egui_ctx);
        app.begin_theme_fetch(&cc.egui_ctx);

        app
    }

    fn tick(&mut self, ctx: &Context) {
        self.poll_version_check(ctx);
        self.poll_update_stream(ctx);
        self.poll_phase_account_link(ctx);
        self.poll_roblox_oauth(ctx);
        self.poll_activation(ctx);
        self.poll_install(ctx);
        self.poll_phase_account_refresh(ctx);
        self.poll_phase_disconnect(ctx);
        self.poll_app_update_check(ctx);
        self.poll_app_update_install(ctx);
        self.poll_theme_fetch(ctx);
        self.poll_theme_apply(ctx);
        self.poll_avatar_fetches(ctx);
        self.ensure_avatar_fetches(ctx);
        self.poll_theme_background_fetches(ctx);
        self.ensure_theme_background_fetch(ctx);

        let Some(started_at) = self.phase_started_at else {
            return;
        };

        let elapsed = started_at.elapsed();
        self.check_milestones();

        match self.phase {
            InstallPhase::Checking => {
                self.progress = progress_for(elapsed, Duration::from_millis(900));
                if self.progress >= 1.0 {
                    if self.release_rx.is_some() {
                        self.progress = 0.95;
                        ctx.request_repaint();
                    } else {
                        self.phase = if self.release.is_some() {
                            InstallPhase::Ready
                        } else {
                            InstallPhase::Idle
                        };
                        self.phase_started_at = None;
                    }
                } else {
                    ctx.request_repaint();
                }
            }
            InstallPhase::Downloading => {
                self.progress = progress_for(elapsed, Duration::from_secs(12)).min(0.92);
                ctx.request_repaint();
            }
            InstallPhase::Installing => {
                self.progress =
                    (0.72 + progress_for(elapsed, Duration::from_secs(6)) * 0.2).min(0.94);
                ctx.request_repaint();
            }
            _ => {}
        }
    }

    fn primary_action(&mut self) {
        match self.phase {
            InstallPhase::Idle | InstallPhase::Error | InstallPhase::Complete => self.start_check(),
            InstallPhase::Ready => self.start_install(),
            InstallPhase::Checking | InstallPhase::Downloading | InstallPhase::Installing => {}
        }
    }

    fn start_check(&mut self) {
        self.phase = InstallPhase::Checking;
        self.progress = 0.0;
        self.phase_started_at = Some(Instant::now());
        self.milestone = 0;
        self.activity.clear();
        self.log(phase::blue(), "Checking for updates...");
        self.begin_version_check(None);
    }

    fn start_install(&mut self) {
        let Some(folder) = self.selected_folder.clone() else {
            self.phase = InstallPhase::Error;
            self.log(phase::red(), "Select an install location first.");
            return;
        };

        let Some(release) = self.release.clone() else {
            self.phase = InstallPhase::Error;
            self.log(phase::red(), "Check for updates before installing.");
            return;
        };

        if release.blocked || !release.download_available {
            self.phase = InstallPhase::Error;
            self.log(phase::red(), "This update is not available for install.");
            return;
        }

        let Some(activation) = self.activation.clone() else {
            self.phase = InstallPhase::Error;
            self.active_tab = ViewTab::Account;
            self.log(
                phase::red(),
                "Connect or verify your account before installing.",
            );
            return;
        };

        self.phase = InstallPhase::Downloading;
        self.progress = 0.0;
        self.phase_started_at = Some(Instant::now());
        self.milestone = 3;
        self.activity.clear();
        self.log(phase::blue(), "Preparing update.");

        let plugin_files = self
            .selected_candidate()
            .map(|candidate| candidate.plugin_files.clone())
            .unwrap_or_default();
        let license_key = self.license_key.trim().to_owned();
        let backup_before_install = self.backup_before_install;
        let (tx, rx) = mpsc::channel();
        self.install_rx = Some(rx);

        std::thread::spawn(move || {
            run_install_worker(
                tx,
                folder,
                plugin_files,
                release,
                activation,
                license_key,
                backup_before_install,
            );
        });
    }

    fn begin_version_check(&mut self, repaint: Option<Context>) {
        if self.release_rx.is_some() {
            return;
        }

        let plan = verification::VerificationPlan::new(CURRENT_BUILD_ID);
        let (tx, rx) = mpsc::channel();
        self.release_error = None;
        self.release_rx = Some(rx);

        std::thread::spawn(move || {
            let result = verification::fetch_version(&plan);
            let _ = tx.send(result);
            if let Some(ctx) = repaint {
                ctx.request_repaint();
            }
        });
    }

    fn poll_version_check(&mut self, ctx: &Context) {
        let Some(result) = self.release_rx.as_ref().and_then(|rx| rx.try_recv().ok()) else {
            return;
        };

        self.release_rx = None;
        match result {
            Ok(release) => {
                let latest_version = release.latest_version.clone();
                let required = release.required || release.update_required;
                let local_current = self.local_matches_latest(&release);
                let available = release.download_available && !release.blocked && !local_current;
                self.release = Some(release);
                self.release_error = None;

                if available {
                    self.phase = InstallPhase::Ready;
                    self.progress = 1.0;
                    self.phase_started_at = None;
                    self.log(
                        phase::green(),
                        format!("Version {latest_version} is available."),
                    );
                    if required {
                        self.log(phase::warning(), "This update is required.");
                    }
                } else {
                    self.phase = InstallPhase::Complete;
                    self.progress = 1.0;
                    self.phase_started_at = None;
                    self.log(phase::green(), "Installed plugin is current.");
                }
            }
            Err(error) => {
                self.release_error = Some(error.clone());
                self.phase = InstallPhase::Error;
                self.phase_started_at = None;
                self.log(phase::red(), error);
            }
        }
        ctx.request_repaint();
    }

    fn begin_update_stream(&mut self, ctx: &Context) {
        if self.update_stream_rx.is_some() {
            return;
        }

        let plan = verification::VerificationPlan::new(CURRENT_BUILD_ID);
        let (tx, rx) = mpsc::channel();
        self.update_stream_rx = Some(rx);

        let repaint = ctx.clone();
        std::thread::spawn(move || {
            let result = verification::listen_for_updates(plan);
            let _ = tx.send(result);
            repaint.request_repaint();
        });
    }

    fn poll_update_stream(&mut self, ctx: &Context) {
        let Some(result) = self
            .update_stream_rx
            .as_ref()
            .and_then(|rx| rx.try_recv().ok())
        else {
            return;
        };

        self.update_stream_rx = None;
        match result {
            Ok(event) => {
                let version = event
                    .latest_version
                    .as_deref()
                    .filter(|value| !value.trim().is_empty())
                    .unwrap_or("new version");
                self.log(phase::green(), format!("Update available: {version}."));
                show_update_notification(version);
                self.begin_version_check(Some(ctx.clone()));
            }
            Err(_) => {}
        }
        self.begin_update_stream(ctx);
        ctx.request_repaint();
    }

    fn local_matches_latest(&self, release: &verification::VersionResponse) -> bool {
        let expected = release
            .release
            .as_ref()
            .map(|release| release.sha256.trim().to_ascii_lowercase())
            .filter(|hash| hash.len() == 64);
        let Some(expected) = expected else {
            return false;
        };
        let Some(candidate) = self.selected_candidate() else {
            return false;
        };
        let target_path = choose_install_target(&candidate.path, &candidate.plugin_files);
        if !target_path.exists() {
            return false;
        };
        let Ok(actual) = sha256_file(&target_path) else {
            return false;
        };
        actual == expected
    }

    fn start_phase_account_link(&mut self, ctx: &Context) {
        if self.link_rx.is_some() {
            return;
        }

        let plan = verification::VerificationPlan::new(CURRENT_BUILD_ID);
        let request = verification::PluginLinkStartRequest {
            roblox_user_id: self.roblox_user_id.trim().to_owned(),
            install_id: install_id(),
            build_id: CURRENT_BUILD_ID.to_owned(),
            product: "Phase Animator".to_owned(),
            version: self
                .release
                .as_ref()
                .map(|release| release.latest_version.clone())
                .unwrap_or_else(|| "dev".to_owned()),
        };
        let (tx, rx) = mpsc::channel();
        self.link_rx = Some(rx);
        self.link_code = None;
        self.link_url = None;
        self.linked_user = None;
        self.plugin_token = None;
        self.log(phase::blue(), "Opening Phase account connection.");

        let repaint = ctx.clone();
        std::thread::spawn(move || {
            let result = verification::start_plugin_link(&plan, &request);
            let _ = tx.send(result);
            repaint.request_repaint();
        });
    }

    fn begin_link_status_check(&mut self, ctx: &Context) {
        if self.link_status_rx.is_some() || self.plugin_token.is_some() {
            return;
        }

        let Some(code) = self.link_code.clone() else {
            return;
        };

        let plan = verification::VerificationPlan::new(CURRENT_BUILD_ID);
        let (tx, rx) = mpsc::channel();
        self.link_status_rx = Some(rx);
        self.last_link_poll = Some(Instant::now());

        let repaint = ctx.clone();
        std::thread::spawn(move || {
            let result = verification::fetch_plugin_link_status(&plan, &code);
            let _ = tx.send(result);
            repaint.request_repaint();
        });
    }

    fn poll_phase_account_link(&mut self, ctx: &Context) {
        if let Some(result) = self.link_rx.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.link_rx = None;
            match result {
                Ok(session) => {
                    self.link_code = Some(session.code.clone());
                    self.link_url = Some(session.verify_url.clone());
                    self.link_expires_at = Some(session.expires_at);
                    self.log(phase::green(), format!("Link code ready: {}", session.code));
                    if let Err(error) = open::that(&session.verify_url) {
                        self.log(phase::warning(), format!("Open browser failed: {error}"));
                    }
                    self.begin_link_status_check(ctx);
                }
                Err(error) => self.log(phase::red(), error),
            }
            ctx.request_repaint();
        }

        if self.link_code.is_some()
            && self.plugin_token.is_none()
            && self.link_status_rx.is_none()
            && self
                .last_link_poll
                .is_none_or(|last| last.elapsed() >= Duration::from_secs(2))
        {
            self.begin_link_status_check(ctx);
        }

        let Some(result) = self
            .link_status_rx
            .as_ref()
            .and_then(|rx| rx.try_recv().ok())
        else {
            return;
        };

        self.link_status_rx = None;
        match result {
            Ok(status) if status.status == "linked" => {
                let access_token = status.token.clone().unwrap_or_default();
                let access_message = status.message.clone().unwrap_or_default();
                self.plugin_token = status.plugin_token.clone();
                self.linked_user = status.user.clone();
                self.link_code = None;
                self.link_url = None;
                self.link_expires_at = status.expires_at.clone();
                let name = self
                    .linked_user
                    .as_ref()
                    .map(display_linked_user)
                    .unwrap_or_else(|| "Phase account".to_owned());
                self.log(phase::green(), format!("Connected to {name}."));
                if !access_token.is_empty() {
                    let user_id_text = status.roblox_user_id.clone().unwrap_or_default();
                    if let Ok(user_id) = user_id_text.parse::<u64>() {
                        self.roblox_user_id = user_id_text;
                        self.activation_error = None;
                        self.activation = Some(verification::ActivationResponse {
                            ok: true,
                            active: true,
                            activation_mode: status
                                .activation_mode
                                .clone()
                                .unwrap_or_else(|| "licenseKey".to_owned()),
                            product: "Phase Animator".to_owned(),
                            user_id,
                            install_id: status.install_id.clone().unwrap_or_else(install_id),
                            asset_id: Some(verification::ROBLOX_PLUGIN_ASSET_ID),
                            token: access_token,
                            expires_at: 0,
                            licensee: status
                                .licensee
                                .clone()
                                .unwrap_or_else(|| format!("Phase account {name}")),
                            message: if access_message.is_empty() {
                                "Phase account license verified.".to_owned()
                            } else {
                                access_message.clone()
                            },
                        });
                        self.log(phase::green(), "Phase account license verified.");
                    }
                } else if status
                    .access_status
                    .as_deref()
                    .is_some_and(|value| value != "verified" && value != "pending")
                    && !access_message.is_empty()
                {
                    self.log(phase::warning(), access_message);
                }
                self.save_account_cache();
            }
            Ok(status) => {
                self.link_expires_at = status.expires_at;
            }
            Err(error) => self.log(phase::warning(), error),
        }
        ctx.request_repaint();
    }

    fn begin_phase_account_refresh(&mut self, ctx: &Context) {
        if self.account_refresh_rx.is_some() {
            return;
        }
        let Some(plugin_token) = self.plugin_token.clone() else {
            return;
        };

        let plan = verification::VerificationPlan::new(CURRENT_BUILD_ID);
        let (tx, rx) = mpsc::channel();
        self.account_refresh_rx = Some(rx);

        let repaint = ctx.clone();
        std::thread::spawn(move || {
            let result = verification::fetch_plugin_me(&plan, &plugin_token);
            let _ = tx.send(result);
            repaint.request_repaint();
        });
    }

    fn poll_phase_account_refresh(&mut self, ctx: &Context) {
        let Some(result) = self
            .account_refresh_rx
            .as_ref()
            .and_then(|rx| rx.try_recv().ok())
        else {
            return;
        };

        self.account_refresh_rx = None;
        match result {
            Ok(me) if me.plugin_linked => {
                self.linked_user = Some(me.user);
                if self.plugin_token.is_some() {
                    self.log(phase::green(), "Phase account restored.");
                }
                self.save_account_cache();
            }
            Ok(_) => {
                self.clear_phase_account(true);
                self.log(phase::warning(), "Phase account link expired.");
            }
            Err(error) => {
                self.clear_phase_account(true);
                self.log(phase::warning(), error);
            }
        }
        ctx.request_repaint();
    }

    fn start_roblox_oauth(&mut self, ctx: &Context) {
        if self.roblox_oauth_rx.is_some() {
            return;
        }

        let plan = verification::VerificationPlan::new(CURRENT_BUILD_ID);
        let request = verification::RobloxOAuthStartRequest {
            install_id: install_id(),
            build_id: CURRENT_BUILD_ID.to_owned(),
            product: "Phase Animator".to_owned(),
            version: self
                .release
                .as_ref()
                .map(|release| release.latest_version.clone())
                .unwrap_or_else(|| "dev".to_owned()),
        };
        let (tx, rx) = mpsc::channel();
        self.roblox_oauth_rx = Some(rx);
        self.roblox_oauth_state = None;
        self.roblox_oauth_url = None;
        self.roblox_oauth_expires_at = None;
        self.roblox_username = None;
        self.roblox_user_id.clear();
        self.activation = None;
        self.activation_error = None;
        self.log(phase::blue(), "Starting Roblox browser verification.");

        let repaint = ctx.clone();
        std::thread::spawn(move || {
            let result = verification::start_roblox_oauth(&plan, &request);
            let _ = tx.send(result);
            repaint.request_repaint();
        });
    }

    fn begin_roblox_oauth_status_check(&mut self, ctx: &Context) {
        if self.roblox_oauth_status_rx.is_some() || self.activation.is_some() {
            return;
        }

        let Some(state) = self.roblox_oauth_state.clone() else {
            return;
        };

        let plan = verification::VerificationPlan::new(CURRENT_BUILD_ID);
        let current_install_id = install_id();
        let (tx, rx) = mpsc::channel();
        self.roblox_oauth_status_rx = Some(rx);
        self.last_roblox_oauth_poll = Some(Instant::now());

        let repaint = ctx.clone();
        std::thread::spawn(move || {
            let result =
                verification::fetch_roblox_oauth_status(&plan, &state, &current_install_id);
            let _ = tx.send(result);
            repaint.request_repaint();
        });
    }

    fn poll_roblox_oauth(&mut self, ctx: &Context) {
        if let Some(result) = self
            .roblox_oauth_rx
            .as_ref()
            .and_then(|rx| rx.try_recv().ok())
        {
            self.roblox_oauth_rx = None;
            match result {
                Ok(session) => {
                    self.roblox_oauth_state = Some(session.state.clone());
                    self.roblox_oauth_url = Some(session.url.clone());
                    self.roblox_oauth_expires_at = Some(session.expires_at);
                    self.log(phase::green(), "Roblox verification link ready.");
                    if let Err(error) = open::that(&session.url) {
                        self.log(phase::warning(), format!("Open browser failed: {error}"));
                    }
                    self.begin_roblox_oauth_status_check(ctx);
                }
                Err(error) => {
                    self.activation_error = Some(error.clone());
                    self.log(phase::red(), error);
                }
            }
            ctx.request_repaint();
        }

        if self.roblox_oauth_state.is_some()
            && self.activation.is_none()
            && self.roblox_oauth_status_rx.is_none()
            && self
                .last_roblox_oauth_poll
                .is_none_or(|last| last.elapsed() >= Duration::from_secs(2))
        {
            self.begin_roblox_oauth_status_check(ctx);
        }

        let Some(result) = self
            .roblox_oauth_status_rx
            .as_ref()
            .and_then(|rx| rx.try_recv().ok())
        else {
            return;
        };

        self.roblox_oauth_status_rx = None;
        match result {
            Ok(status) if status.status == "verified" => {
                let user_id_text = status.roblox_user_id.clone().unwrap_or_default();
                let Ok(user_id) = user_id_text.parse::<u64>() else {
                    self.activation_error =
                        Some("Roblox OAuth returned an invalid user ID.".to_owned());
                    self.log(phase::red(), "Roblox OAuth returned an invalid user ID.");
                    ctx.request_repaint();
                    return;
                };

                self.roblox_user_id = user_id_text;
                self.roblox_username = status.roblox_username.clone();
                self.activation_error = None;
                self.activation = Some(verification::ActivationResponse {
                    ok: true,
                    active: true,
                    activation_mode: status
                        .activation_mode
                        .unwrap_or_else(|| "robloxPurchase".to_owned()),
                    product: "Phase Animator".to_owned(),
                    user_id,
                    install_id: status.install_id.unwrap_or_else(install_id),
                    asset_id: status
                        .asset_id
                        .as_deref()
                        .and_then(|asset_id| asset_id.parse::<u64>().ok()),
                    token: status.token.unwrap_or_default(),
                    expires_at: 0,
                    licensee: status
                        .licensee
                        .unwrap_or_else(|| format!("Roblox account {user_id}")),
                    message: status
                        .message
                        .unwrap_or_else(|| "Roblox OAuth purchase verified.".to_owned()),
                });
                let name = self
                    .roblox_username
                    .as_deref()
                    .filter(|name| !name.trim().is_empty())
                    .unwrap_or(&self.roblox_user_id);
                self.log(phase::green(), format!("Roblox verified: {name}."));
                self.save_account_cache();
            }
            Ok(status) if status.status == "denied" => {
                let message = status
                    .message
                    .unwrap_or_else(|| "Roblox verification was denied.".to_owned());
                self.activation = None;
                self.activation_error = Some(message.clone());
                self.log(phase::red(), message);
            }
            Ok(status) => {
                self.roblox_oauth_expires_at = status.expires_at;
            }
            Err(error) => {
                self.activation_error = Some(error.clone());
                self.log(phase::warning(), error);
            }
        }
        ctx.request_repaint();
    }

    fn start_activation(&mut self, ctx: &Context) {
        if self.activation_rx.is_some() {
            return;
        }

        let Ok(user_id) = self.roblox_user_id.trim().parse::<u64>() else {
            self.activation_error = Some("Verify Roblox in browser first.".to_owned());
            self.log(phase::red(), "Verify Roblox in browser first.");
            return;
        };

        let license_key = self.license_key.trim().to_owned();
        if license_key.is_empty() {
            self.activation_error = Some("Enter a Phase license key.".to_owned());
            self.log(phase::red(), "Enter a Phase license key.");
            return;
        }

        let plan = verification::VerificationPlan::new(CURRENT_BUILD_ID);
        let request = verification::ActivationRequest {
            activation_mode: "licenseKey".to_owned(),
            license_key: Some(license_key),
            user_id,
            install_id: install_id(),
            asset_id: None,
        };

        let (tx, rx) = mpsc::channel();
        self.activation_rx = Some(rx);
        self.activation = None;
        self.activation_error = None;
        self.log(
            phase::blue(),
            "Activating license key for verified Roblox account.",
        );

        let repaint = ctx.clone();
        std::thread::spawn(move || {
            let result = verification::activate_install(&plan, &request);
            let _ = tx.send(result);
            repaint.request_repaint();
        });
    }

    fn poll_activation(&mut self, ctx: &Context) {
        let Some(result) = self
            .activation_rx
            .as_ref()
            .and_then(|rx| rx.try_recv().ok())
        else {
            return;
        };

        self.activation_rx = None;
        match result {
            Ok(activation) => {
                self.log(phase::green(), activation.message.clone());
                self.activation_error = None;
                self.activation = Some(activation);
                self.save_account_cache();
            }
            Err(error) => {
                self.activation = None;
                self.activation_error = Some(error.clone());
                self.log(phase::red(), error);
            }
        }
        ctx.request_repaint();
    }

    fn poll_install(&mut self, ctx: &Context) {
        let Some(rx) = self.install_rx.as_ref() else {
            return;
        };
        let events: Vec<InstallEvent> = rx.try_iter().collect();
        if events.is_empty() {
            return;
        }

        let mut finished = false;
        for event in events {
            match event {
                InstallEvent::Progress {
                    phase,
                    color,
                    message,
                    progress,
                } => {
                    self.phase = phase;
                    self.phase_started_at = Some(Instant::now());
                    self.progress = progress;
                    self.log(color, message);
                }
                InstallEvent::Finished(result) => {
                    finished = true;
                    self.phase_started_at = None;
                    match result {
                        Ok(outcome) => {
                            self.phase = InstallPhase::Complete;
                            self.progress = 1.0;
                            self.log(
                                phase::green(),
                                format!("Installed Phase Animator {}.", outcome.version),
                            );
                            if let Some(backup) = outcome.backup_path {
                                self.log(
                                    phase::blue(),
                                    format!("Backup saved: {}", compact_path(&backup, 34)),
                                );
                            }
                            self.log(
                                phase::green(),
                                format!("Installed at {}", compact_path(&outcome.target_path, 34)),
                            );
                            self.refresh_detection();
                            self.begin_version_check(Some(ctx.clone()));
                        }
                        Err(error) => {
                            self.phase = InstallPhase::Error;
                            self.progress = 0.0;
                            self.log(phase::red(), error);
                        }
                    }
                }
            }
        }

        if finished {
            self.install_rx = None;
        }
        ctx.request_repaint();
    }

    fn start_phase_disconnect(&mut self, ctx: &Context) {
        if self.phase_disconnect_rx.is_some() {
            return;
        }

        let Some(plugin_token) = self.plugin_token.clone() else {
            self.clear_phase_account(true);
            return;
        };

        let plan = verification::VerificationPlan::new(CURRENT_BUILD_ID);
        let (tx, rx) = mpsc::channel();
        self.phase_disconnect_rx = Some(rx);
        self.log(phase::blue(), "Disconnecting Phase account.");

        let repaint = ctx.clone();
        std::thread::spawn(move || {
            let result = verification::disconnect_plugin_me(&plan, &plugin_token);
            let _ = tx.send(result);
            repaint.request_repaint();
        });
    }

    fn poll_phase_disconnect(&mut self, ctx: &Context) {
        let Some(result) = self
            .phase_disconnect_rx
            .as_ref()
            .and_then(|rx| rx.try_recv().ok())
        else {
            return;
        };

        self.phase_disconnect_rx = None;
        match result {
            Ok(()) => {
                self.clear_phase_account(true);
                self.log(phase::green(), "Phase account disconnected.");
            }
            Err(error) => {
                self.clear_phase_account(true);
                self.log(phase::warning(), error);
            }
        }
        ctx.request_repaint();
    }

    fn begin_app_update_check(&mut self, ctx: &Context) {
        if self.app_update_rx.is_some() {
            return;
        }

        let (tx, rx) = mpsc::channel();
        self.app_update_rx = Some(rx);
        self.app_update_error = None;
        let repaint = ctx.clone();
        std::thread::spawn(move || {
            let result = verification::fetch_latest_app_update(env!("CARGO_PKG_VERSION"));
            let _ = tx.send(result);
            repaint.request_repaint();
        });
    }

    fn poll_app_update_check(&mut self, ctx: &Context) {
        let Some(result) = self
            .app_update_rx
            .as_ref()
            .and_then(|rx| rx.try_recv().ok())
        else {
            return;
        };

        self.app_update_rx = None;
        match result {
            Ok(Some(update)) => {
                self.log(
                    phase::blue(),
                    format!("Installer update {} is available.", update.version),
                );
                show_system_notification(
                    "Phase Auto Updater",
                    &format!("Installer update {} is available", update.version),
                );
                self.app_update = Some(update);
                self.app_update_error = None;
            }
            Ok(None) => {
                self.app_update_error = None;
            }
            Err(error) => {
                self.app_update_error = Some(error);
            }
        }
        ctx.request_repaint();
    }

    fn start_app_update_install(&mut self, ctx: &Context) {
        if self.app_update_install_rx.is_some() {
            return;
        }

        let Some(update) = self.app_update.clone() else {
            self.begin_app_update_check(ctx);
            return;
        };

        let (tx, rx) = mpsc::channel();
        self.app_update_install_rx = Some(rx);
        self.log(
            phase::blue(),
            format!("Downloading installer {}.", update.version),
        );

        let repaint = ctx.clone();
        std::thread::spawn(move || {
            let result = download_and_launch_app_update(update);
            let _ = tx.send(result);
            repaint.request_repaint();
        });
    }

    fn poll_app_update_install(&mut self, ctx: &Context) {
        let Some(result) = self
            .app_update_install_rx
            .as_ref()
            .and_then(|rx| rx.try_recv().ok())
        else {
            return;
        };

        self.app_update_install_rx = None;
        match result {
            Ok(path) => {
                self.log(
                    phase::green(),
                    format!("Installer launched: {}", compact_path(&path, 34)),
                );
            }
            Err(error) => {
                self.app_update_error = Some(error.clone());
                self.log(phase::red(), error);
            }
        }
        ctx.request_repaint();
    }

    fn begin_theme_fetch(&mut self, ctx: &Context) {
        if self.theme_fetch_rx.is_some() {
            return;
        }

        let plan = verification::VerificationPlan::new(CURRENT_BUILD_ID);
        let (tx, rx) = mpsc::channel();
        self.theme_fetch_rx = Some(rx);
        self.theme_error = None;
        let repaint = ctx.clone();
        std::thread::spawn(move || {
            let result = verification::fetch_phase_themes(&plan);
            let _ = tx.send(result);
            repaint.request_repaint();
        });
    }

    fn poll_theme_fetch(&mut self, ctx: &Context) {
        let Some(result) = self
            .theme_fetch_rx
            .as_ref()
            .and_then(|rx| rx.try_recv().ok())
        else {
            return;
        };

        self.theme_fetch_rx = None;
        match result {
            Ok(themes) => {
                self.theme_assets = themes;
                self.visible_theme_count = self.visible_theme_count.max(6);
                self.theme_error = None;
            }
            Err(error) => {
                self.theme_error = Some(error);
            }
        }
        ctx.request_repaint();
    }

    fn start_theme_apply(&mut self, ctx: &Context, asset: verification::PhaseThemeAsset) {
        if self.theme_apply_rx.is_some() {
            return;
        }

        let plan = verification::VerificationPlan::new(CURRENT_BUILD_ID);
        let (tx, rx) = mpsc::channel();
        self.theme_apply_rx = Some(rx);
        self.theme_error = None;
        self.log(phase::blue(), format!("Applying {} theme.", asset.title));
        let repaint = ctx.clone();
        std::thread::spawn(move || {
            let result = verification::install_phase_theme(&plan, &asset.id).and_then(|response| {
                let theme_code = response.theme_code.trim().to_owned();
                if theme_code.is_empty() {
                    return Err("Theme did not include a Phase theme code.".to_owned());
                }
                Ok(ThemeSelection {
                    asset_id: response.asset.id,
                    title: response.asset.title,
                    background_image_id: parse_theme_background_image_id(&theme_code),
                    theme_code,
                })
            });
            let _ = tx.send(result);
            repaint.request_repaint();
        });
    }

    fn poll_theme_apply(&mut self, ctx: &Context) {
        let Some(result) = self
            .theme_apply_rx
            .as_ref()
            .and_then(|rx| rx.try_recv().ok())
        else {
            return;
        };

        self.theme_apply_rx = None;
        match result {
            Ok(selection) => {
                if let Some(palette) = phase::palette_from_theme_code(&selection.theme_code) {
                    phase::set_palette(palette);
                    configure_style(ctx);
                    self.theme_background = None;
                    self.theme_background_key = None;
                    self.selected_theme = Some(selection.clone());
                    self.save_account_cache();
                    self.log(
                        phase::green(),
                        format!("Theme applied: {}", selection.title),
                    );
                } else {
                    self.theme_error = Some("Theme code did not include enough colors.".to_owned());
                }
            }
            Err(error) => {
                self.theme_error = Some(error.clone());
                self.log(phase::red(), error);
            }
        }
        ctx.request_repaint();
    }

    fn reset_theme(&mut self, ctx: &Context) {
        phase::reset_palette();
        configure_style(ctx);
        self.selected_theme = None;
        self.theme_background = None;
        self.theme_background_key = None;
        self.save_account_cache();
        self.log(phase::green(), "Restored default Phase theme.");
    }

    fn disconnect_roblox_account(&mut self) {
        self.roblox_user_id.clear();
        self.roblox_username = None;
        self.roblox_oauth_state = None;
        self.roblox_oauth_url = None;
        self.roblox_oauth_expires_at = None;
        self.roblox_avatar = None;
        self.roblox_avatar_key = None;
        self.activation = None;
        self.activation_error = None;
        self.save_account_cache();
        self.log(phase::green(), "Roblox account disconnected locally.");
    }

    fn clear_phase_account(&mut self, save: bool) {
        self.linked_user = None;
        self.plugin_token = None;
        self.link_code = None;
        self.link_url = None;
        self.link_expires_at = None;
        self.phase_avatar = None;
        self.phase_avatar_key = None;
        if save {
            self.save_account_cache();
        }
    }

    fn load_cached_accounts(&mut self, ctx: &Context) {
        let Some(cache) = load_account_cache() else {
            return;
        };
        self.plugin_token = cache.plugin_token;
        self.linked_user = cache.linked_user;
        self.roblox_user_id = cache.roblox_user_id;
        self.roblox_username = cache.roblox_username;
        self.activation = cache.activation;
        self.selected_theme = cache.selected_theme;
        self.theme_background_mode = cache.theme_background_mode;
        if let Some(selection) = &self.selected_theme {
            if let Some(palette) = phase::palette_from_theme_code(&selection.theme_code) {
                phase::set_palette(palette);
                configure_style(ctx);
            }
        }
        if self.plugin_token.is_some() || !self.roblox_user_id.trim().is_empty() {
            self.log(phase::blue(), "Restored saved account connection.");
        }
    }

    fn save_account_cache(&self) {
        let cache = AccountCache {
            plugin_token: self.plugin_token.clone(),
            linked_user: self.linked_user.clone(),
            roblox_user_id: self.roblox_user_id.clone(),
            roblox_username: self.roblox_username.clone(),
            activation: self.activation.clone(),
            selected_theme: self.selected_theme.clone(),
            theme_background_mode: self.theme_background_mode,
        };
        save_account_cache(&cache);
    }

    fn ensure_avatar_fetches(&mut self, ctx: &Context) {
        if let Some(url) = self
            .linked_user
            .as_ref()
            .and_then(|user| user.avatar_url.as_deref())
            .filter(|url| !url.trim().is_empty())
        {
            if self.phase_avatar_key.as_deref() != Some(url) {
                let key = url.to_owned();
                self.phase_avatar_key = Some(key.clone());
                self.phase_avatar = None;
                spawn_avatar_fetch(
                    self.avatar_tx.clone(),
                    AvatarKind::Phase,
                    key.clone(),
                    ctx.clone(),
                    move || verification::fetch_phase_avatar_image(&key),
                );
            }
        }

        let roblox_user_id = self.roblox_user_id.trim();
        if !roblox_user_id.is_empty() && self.roblox_avatar_key.as_deref() != Some(roblox_user_id) {
            let key = roblox_user_id.to_owned();
            self.roblox_avatar_key = Some(key.clone());
            self.roblox_avatar = None;
            spawn_avatar_fetch(
                self.avatar_tx.clone(),
                AvatarKind::Roblox,
                key.clone(),
                ctx.clone(),
                move || verification::fetch_roblox_avatar_image(&key),
            );
        }
    }

    fn poll_avatar_fetches(&mut self, ctx: &Context) {
        while let Ok(result) = self.avatar_rx.try_recv() {
            let Ok(image) = result.image else {
                continue;
            };
            let texture = ctx.load_texture(
                format!(
                    "identity-avatar-{}-{}",
                    match result.kind {
                        AvatarKind::Phase => "phase",
                        AvatarKind::Roblox => "roblox",
                    },
                    result.key
                ),
                image,
                TextureOptions::LINEAR,
            );
            match result.kind {
                AvatarKind::Phase => self.phase_avatar = Some(texture),
                AvatarKind::Roblox => self.roblox_avatar = Some(texture),
            }
            ctx.request_repaint();
        }
    }

    fn ensure_theme_background_fetch(&mut self, ctx: &Context) {
        let Some(image_id) = self
            .selected_theme
            .as_ref()
            .and_then(|theme| theme.background_image_id.as_deref())
            .filter(|id| !id.trim().is_empty())
        else {
            return;
        };

        if self.theme_background_key.as_deref() == Some(image_id) {
            return;
        }

        let key = image_id.to_owned();
        self.theme_background_key = Some(key.clone());
        self.theme_background = None;
        spawn_theme_background_fetch(
            self.theme_background_tx.clone(),
            key.clone(),
            ctx.clone(),
            move || verification::fetch_roblox_asset_thumbnail_image(&key),
        );
    }

    fn poll_theme_background_fetches(&mut self, ctx: &Context) {
        while let Ok(result) = self.theme_background_rx.try_recv() {
            let Ok(image) = result.image else {
                continue;
            };
            self.theme_background = Some(ctx.load_texture(
                format!("theme-background-{}", result.key),
                image,
                TextureOptions::LINEAR,
            ));
            ctx.request_repaint();
        }
    }

    fn refresh_detection(&mut self) {
        let previous = self.selected_folder.clone();
        self.candidates = detect_plugin_folders();
        self.selected_folder = previous
            .filter(|path| path.exists())
            .or_else(|| best_candidate(&self.candidates).map(|candidate| candidate.path));
        self.log(phase::blue(), "Install locations refreshed.");
    }

    fn choose_folder(&mut self) {
        if let Some(folder) = rfd::FileDialog::new().pick_folder() {
            let candidate = inspect_candidate(folder.clone(), "Manual selection".to_owned());
            self.selected_folder = Some(folder.clone());
            if !self
                .candidates
                .iter()
                .any(|existing| normalize_path(&existing.path) == normalize_path(&folder))
            {
                self.candidates.insert(0, candidate);
            }
            self.log(
                phase::green(),
                format!("Selected {}", compact_path(&folder, 30)),
            );
        }
    }

    fn open_folder(&mut self) {
        let Some(path) = self.selected_folder.clone() else {
            self.log(phase::warning(), "No install location selected.");
            return;
        };

        match open::that(&path) {
            Ok(_) => self.log(phase::blue(), "Opened install location."),
            Err(error) => self.log(phase::red(), format!("Could not open folder: {error}")),
        }
    }

    fn selected_candidate(&self) -> Option<&PluginFolderCandidate> {
        let selected = self.selected_folder.as_ref()?;
        self.candidates
            .iter()
            .find(|candidate| normalize_path(&candidate.path) == normalize_path(selected))
    }

    fn log(&mut self, color: Color32, text: impl Into<String>) {
        self.activity.push(ActivityLine {
            color,
            text: text.into(),
        });
        if self.activity.len() > 7 {
            self.activity.remove(0);
        }
    }
}

fn run_install_worker(
    tx: Sender<InstallEvent>,
    folder: PathBuf,
    plugin_files: Vec<PluginFile>,
    release: verification::VersionResponse,
    activation: verification::ActivationResponse,
    license_key: String,
    backup_before_install: bool,
) {
    let result = install_update(
        &tx,
        folder,
        plugin_files,
        release,
        activation,
        license_key,
        backup_before_install,
    );
    let _ = tx.send(InstallEvent::Finished(result));
}

fn download_and_launch_app_update(update: verification::AppUpdateInfo) -> Result<PathBuf, String> {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "PhaseAutoUpdater-{}.msi",
        safe_file_fragment(&update.version)
    ));
    verification::download_url_to_file(&update.download_url, &path)?;

    Command::new("msiexec")
        .arg("/i")
        .arg(&path)
        .arg("/passive")
        .spawn()
        .map_err(|error| format!("Could not launch installer update: {error}"))?;

    Ok(path)
}

fn install_update(
    tx: &Sender<InstallEvent>,
    folder: PathBuf,
    plugin_files: Vec<PluginFile>,
    release: verification::VersionResponse,
    activation: verification::ActivationResponse,
    license_key: String,
    backup_before_install: bool,
) -> Result<InstallOutcome, String> {
    std::fs::create_dir_all(&folder)
        .map_err(|error| format!("Could not prepare install folder: {error}"))?;

    // Never replace "the first .rbxm" unless it clearly looks like ours. A lot
    // of creators keep multiple local plugins in this folder.
    let target_path = choose_install_target(&folder, &plugin_files);
    let temp_path = temporary_download_path(&folder);
    let expected_from_version = release
        .release
        .as_ref()
        .map(|info| info.sha256.trim().to_ascii_lowercase())
        .filter(|hash| hash.len() == 64);

    send_install_progress(
        tx,
        InstallPhase::Downloading,
        phase::blue(),
        "Authorizing install.",
        0.12,
    );

    let plan = verification::VerificationPlan::new(CURRENT_BUILD_ID);
    let request = verification::DownloadSessionRequest {
        activation_mode: activation.activation_mode.clone(),
        user_id: activation.user_id,
        install_id: activation.install_id.clone(),
        asset_id: (activation.activation_mode == "robloxPurchase")
            .then_some(activation.asset_id)
            .flatten(),
        license_key: (activation.activation_mode == "licenseKey" && !license_key.is_empty())
            .then_some(license_key),
        token: activation.token.clone(),
        build_id: CURRENT_BUILD_ID.to_owned(),
        target_build_id: Some(release.latest_build_id.clone()),
    };
    let session = verification::create_download_session(&plan, &request)?;
    if !session.ok {
        return Err("Install was not authorized.".to_owned());
    }

    send_install_progress(
        tx,
        InstallPhase::Downloading,
        phase::blue(),
        "Downloading update package.",
        0.36,
    );
    verification::download_plugin_to_file(&session.download_url, &temp_path)?;

    let expected_hash = session
        .sha256
        .trim()
        .to_ascii_lowercase()
        .chars()
        .collect::<String>();
    let expected_hash = if expected_hash.len() == 64 {
        expected_hash
    } else {
        expected_from_version.ok_or_else(|| "Update metadata is missing a file hash.".to_owned())?
    };

    let actual_hash = sha256_file(&temp_path)?;
    if actual_hash != expected_hash {
        let _ = std::fs::remove_file(&temp_path);
        return Err("Downloaded update could not be verified.".to_owned());
    }

    if session.size > 0 {
        let actual_size = std::fs::metadata(&temp_path)
            .map_err(|error| format!("Could not inspect update package: {error}"))?
            .len();
        if actual_size != session.size {
            let _ = std::fs::remove_file(&temp_path);
            return Err("Downloaded update size did not match.".to_owned());
        }
    }

    send_install_progress(
        tx,
        InstallPhase::Installing,
        phase::green(),
        "Update package verified.",
        0.72,
    );

    let backup_path = if target_path.exists() && backup_before_install {
        let backup_path = next_backup_path(&target_path);
        std::fs::copy(&target_path, &backup_path)
            .map_err(|error| format!("Could not create backup: {error}"))?;
        send_install_progress(
            tx,
            InstallPhase::Installing,
            phase::blue(),
            "Created local backup.",
            0.82,
        );
        Some(backup_path)
    } else {
        None
    };

    if target_path.exists() {
        // Windows will not overwrite an existing file with rename(), so remove
        // after the backup has been made and the new file has passed checks.
        std::fs::remove_file(&target_path)
            .map_err(|error| format!("Could not replace installed plugin: {error}"))?;
    }
    std::fs::rename(&temp_path, &target_path)
        .map_err(|error| format!("Could not install update: {error}"))?;

    send_install_progress(
        tx,
        InstallPhase::Installing,
        phase::green(),
        "Plugin files updated.",
        0.94,
    );

    Ok(InstallOutcome {
        target_path,
        backup_path,
        version: session.version,
    })
}

fn send_install_progress(
    tx: &Sender<InstallEvent>,
    phase: InstallPhase,
    color: Color32,
    message: impl Into<String>,
    progress: f32,
) {
    let _ = tx.send(InstallEvent::Progress {
        phase,
        color,
        message: message.into(),
        progress,
    });
}

fn choose_install_target(folder: &Path, plugin_files: &[PluginFile]) -> PathBuf {
    let preferred = folder.join("PhaseAnimator.rbxm");
    if preferred.exists() {
        return preferred;
    }

    for plugin_file in plugin_files {
        if plugin_file
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.eq_ignore_ascii_case("PhaseAnimator.rbxm"))
        {
            return plugin_file.path.clone();
        }
    }

    for plugin_file in plugin_files {
        let name = plugin_file
            .path
            .file_stem()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .replace([' ', '_', '-'], "")
            .to_ascii_lowercase();
        if name.contains("phaseanimator") {
            return plugin_file.path.clone();
        }
    }

    // New install or folder has unrelated plugins. Use our own filename.
    preferred
}

fn temporary_download_path(folder: &Path) -> PathBuf {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    folder.join(format!(".phase-animator-{stamp}.download"))
}

fn next_backup_path(target_path: &Path) -> PathBuf {
    let file_name = target_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("PhaseAnimator.rbxm");
    let first = target_path.with_file_name(format!("{file_name}.bak"));
    if !first.exists() {
        return first;
    }

    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    target_path.with_file_name(format!("{file_name}.{stamp}.bak"))
}

fn safe_file_fragment(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '.' || ch == '-' || ch == '_' {
                ch
            } else {
                '-'
            }
        })
        .collect()
}

impl eframe::App for PhaseInstallerApp {
    fn update(&mut self, ctx: &Context, frame: &mut eframe::Frame) {
        apply_windows_title_bar(frame);
        self.tick(ctx);

        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(phase::background()))
            .show(ctx, |ui| {
                self.paint_theme_background(ui);
                ui.vertical_centered(|ui| {
                    ui.set_width(SCROLL_BODY_WIDTH);
                    ui.add_space(12.0);
                    self.identity_strip(ui);
                    self.logo_block(ui);
                    ui.add_space(8.0);
                    self.title_block(ui);
                    ui.add_space(8.0);

                    // Custom flat tabs segment selector
                    self.draw_custom_tabs(ui);

                    ui.add_space(8.0);
                    egui::ScrollArea::vertical()
                        .id_source("phase-installer-body")
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.vertical_centered(|ui| {
                                ui.set_width(CONTENT_WIDTH);
                                self.current_tab(ui);
                                ui.add_space(8.0);
                                self.activity_block(ui);
                            });
                        });
                });
            });
    }
}

impl PhaseInstallerApp {
    fn paint_theme_background(&self, ui: &mut Ui) {
        let Some(texture) = &self.theme_background else {
            return;
        };

        let rect = ui.max_rect();
        let texture_size = texture.size_vec2();
        let (image_rect, uv_rect) =
            theme_background_layout(rect, texture_size, self.theme_background_mode);
        ui.painter().image(
            texture.id(),
            image_rect,
            uv_rect,
            Color32::from_rgba_premultiplied(255, 255, 255, 44),
        );
        ui.painter().rect_filled(
            rect,
            Rounding::ZERO,
            Color32::from_rgba_premultiplied(0, 0, 0, 156),
        );
    }

    fn check_milestones(&mut self) {
        let Some(started_at) = self.phase_started_at else {
            return;
        };
        let elapsed = started_at.elapsed();

        match self.phase {
            InstallPhase::Checking => {
                if elapsed >= Duration::from_millis(150) && self.milestone == 0 {
                    self.log(phase::blue(), "Checking for updates...");
                    self.milestone = 1;
                } else if elapsed >= Duration::from_millis(400) && self.milestone == 1 {
                    self.log(phase::green(), "Update service is available.");
                    self.milestone = 2;
                } else if elapsed >= Duration::from_millis(650) && self.milestone == 2 {
                    let target = self
                        .release
                        .as_ref()
                        .map(|release| release.latest_version.as_str())
                        .unwrap_or("latest");
                    self.log(phase::blue(), format!("Preparing version {target}."));
                    self.milestone = 3;
                }
            }
            InstallPhase::Downloading | InstallPhase::Installing => {}
            _ => {}
        }
    }

    fn draw_custom_tabs(&mut self, ui: &mut Ui) {
        let target_x = match self.active_tab {
            ViewTab::Install => 0.0,
            ViewTab::Account => 1.0,
            ViewTab::Folders => 2.0,
            ViewTab::Options => 3.0,
        };

        let diff = target_x - self.tab_lerp;
        if diff.abs() > 0.002 {
            self.tab_lerp += diff * 0.25;
            ui.ctx().request_repaint();
        } else {
            self.tab_lerp = target_x;
        }

        let width = CONTENT_WIDTH;
        let height = 42.0;

        let (rect, response) = ui.allocate_exact_size(Vec2::new(width, height), Sense::click());
        let painter = ui.painter();

        painter.rect_filled(rect, Rounding::same(8.0), phase::input());
        painter.rect_stroke(rect, Rounding::same(8.0), Stroke::new(1.0, phase::line()));

        let tab_width = width / 4.0;
        let highlight_rect = Rect::from_min_max(
            Pos2::new(
                rect.left() + self.tab_lerp * tab_width + 2.0,
                rect.top() + 2.0,
            ),
            Pos2::new(
                rect.left() + (self.tab_lerp + 1.0) * tab_width - 2.0,
                rect.bottom() - 2.0,
            ),
        );

        painter.rect_filled(highlight_rect, Rounding::same(6.0), phase::surface());
        painter.rect_stroke(
            highlight_rect,
            Rounding::same(6.0),
            Stroke::new(1.0, phase::line()),
        );

        if response.clicked() {
            if let Some(pos) = response.interact_pointer_pos() {
                let rel_x = pos.x - rect.left();
                let tab_idx = (rel_x / tab_width).floor() as i32;
                match tab_idx {
                    0 => self.active_tab = ViewTab::Install,
                    1 => self.active_tab = ViewTab::Account,
                    2 => self.active_tab = ViewTab::Folders,
                    3 => self.active_tab = ViewTab::Options,
                    _ => {}
                }
            }
        }

        let labels = ["Install", "Account", "Folders", "Options"];
        let icons = [
            MiniIcon::Bolt,
            MiniIcon::User,
            MiniIcon::Folder,
            MiniIcon::Gear,
        ];
        for i in 0..4 {
            let x_center = rect.left() + (i as f32 + 0.5) * tab_width;
            let y_center = rect.center().y;

            let is_active = match (self.active_tab, i) {
                (ViewTab::Install, 0) => true,
                (ViewTab::Account, 1) => true,
                (ViewTab::Folders, 2) => true,
                (ViewTab::Options, 3) => true,
                _ => false,
            };

            let color = if is_active {
                phase::text()
            } else {
                phase::text_muted()
            };

            let icon_rect =
                Rect::from_center_size(Pos2::new(x_center - 26.0, y_center), Vec2::splat(15.0));
            draw_icon_at(painter, icon_rect, icons[i], color);

            painter.text(
                Pos2::new(x_center + 9.0, y_center),
                Align2::CENTER_CENTER,
                labels[i],
                FontId::proportional(13.2),
                color,
            );
        }
    }

    fn draw_progress(&self, ui: &mut Ui) {
        let width = CARD_INNER_WIDTH;
        let height = 22.0;

        let (rect, _) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
        let painter = ui.painter();

        painter.rect_filled(rect, Rounding::same(6.0), phase::input());
        painter.rect_stroke(rect, Rounding::same(6.0), Stroke::new(1.0, phase::line()));

        let progress = self.progress;
        if progress > 0.0 {
            let fill_width = width * progress;
            let fill_rect =
                Rect::from_min_max(rect.min, Pos2::new(rect.min.x + fill_width, rect.max.y));

            painter.rect_filled(fill_rect, Rounding::same(6.0), phase_color(self.phase));
        }

        let pct_text = format!("{}%", (progress * 100.0) as i32);
        painter.text(
            rect.center(),
            Align2::CENTER_CENTER,
            pct_text,
            FontId::proportional(13.0),
            if progress > 0.4 {
                phase::text_on_accent()
            } else {
                phase::text()
            },
        );
    }

    fn logo_block(&self, ui: &mut Ui) {
        if let Some(logo) = &self.logo {
            let image = egui::Image::new(logo).fit_to_exact_size(Vec2::splat(84.0));
            ui.add(image);
        } else {
            ui.label(
                RichText::new("Phase Animator")
                    .font(FontId::proportional(24.0))
                    .color(phase::text()),
            );
        }
    }

    fn identity_strip(&self, ui: &mut Ui) {
        let phase_name = self.linked_user.as_ref().map(display_linked_user);
        let roblox_name = self
            .roblox_username
            .as_deref()
            .filter(|name| !name.trim().is_empty())
            .map(str::to_owned)
            .or_else(|| {
                (!self.roblox_user_id.trim().is_empty()).then(|| self.roblox_user_id.clone())
            });

        let count = phase_name.is_some() as usize + roblox_name.is_some() as usize;
        if count == 0 {
            return;
        }

        let gap = 18.0;
        let width = if count == 1 {
            264.0
        } else {
            (CONTENT_WIDTH - gap) / 2.0
        };
        let row_size = Vec2::new(CONTENT_WIDTH, 60.0);
        let row_layout =
            egui::Layout::left_to_right(egui::Align::Center).with_main_align(egui::Align::Center);
        ui.allocate_ui_with_layout(row_size, row_layout, |ui| {
            if let Some(name) = phase_name {
                identity_card(
                    ui,
                    self.phase_avatar.as_ref(),
                    &name,
                    "Phase account",
                    width,
                    phase::accent(),
                );
            }
            if count == 2 {
                ui.add_space(gap);
            }
            if let Some(name) = roblox_name {
                identity_card(
                    ui,
                    self.roblox_avatar.as_ref(),
                    &name,
                    "Roblox verified",
                    width,
                    phase::blue(),
                );
            }
        });
        ui.add_space(8.0);
    }

    fn title_block(&self, ui: &mut Ui) {
        ui.label(
            RichText::new("Phase Animator")
                .font(FontId::proportional(28.0))
                .strong()
                .color(phase::text()),
        );
        ui.add_space(3.0);
        ui.label(
            RichText::new("Roblox Studio plugin installer")
                .font(FontId::proportional(13.5))
                .color(phase::text_secondary()),
        );
        ui.add_space(8.0);
        let row_size = Vec2::new(ui.available_width(), 24.0);
        let row_layout =
            egui::Layout::left_to_right(egui::Align::Center).with_main_align(egui::Align::Center);
        ui.allocate_ui_with_layout(row_size, row_layout, |ui| {
            let current_phase = phase_text(self.phase);
            let current_channel = "Default channel";
            status_pill(ui, current_phase, phase_color(self.phase));
            status_pill(ui, current_channel, phase::accent());
        });
    }

    fn current_tab(&mut self, ui: &mut Ui) {
        match self.active_tab {
            ViewTab::Install => self.install_tab(ui),
            ViewTab::Account => self.account_tab(ui),
            ViewTab::Folders => self.folders_tab(ui),
            ViewTab::Options => self.options_tab(ui),
        }
    }

    fn install_tab(&mut self, ui: &mut Ui) {
        let _time = ui.input(|i| i.time);
        draw_panel(ui, |ui| {
            section_label(ui, "Release");
            ui.add_space(4.0);
            release_metric(ui, MiniIcon::Folder, "Installed Plugin", "Local install");
            ui.add_space(6.0);
            let latest = self
                .release
                .as_ref()
                .map(|release| release.latest_version.as_str())
                .unwrap_or("Checking...");
            release_metric(ui, MiniIcon::Rocket, "Latest Version Check", latest);

            ui.add_space(8.0);
            egui::Frame::none()
                .fill(phase::input())
                .stroke(Stroke::new(1.0, phase::line()))
                .rounding(Rounding::same(6.0))
                .inner_margin(Margin::same(10.0))
                .show(ui, |ui| {
                    ui.set_width(CARD_INNER_WIDTH - 16.0);
                    let status = self
                        .release
                        .as_ref()
                        .map(|release| {
                            if release.download_available && !release.blocked {
                                "Ready"
                            } else {
                                "Current"
                            }
                        })
                        .unwrap_or("Checking");
                    info_row(ui, "Update status", status);
                    ui.add_space(4.0);
                    let latest = self
                        .release
                        .as_ref()
                        .map(|release| release.latest_version.as_str())
                        .unwrap_or("pending");
                    info_row(ui, "Available version", latest);
                });

            ui.add_space(10.0);
            let button_text = match self.phase {
                InstallPhase::Ready => "Install Update",
                InstallPhase::Complete => "Check Again",
                _ => "Check for Update",
            };
            let button_icon = match self.phase {
                InstallPhase::Ready => MiniIcon::Bolt,
                InstallPhase::Complete => MiniIcon::Refresh,
                _ => MiniIcon::Download,
            };
            let busy = matches!(
                self.phase,
                InstallPhase::Checking | InstallPhase::Downloading | InstallPhase::Installing
            );
            ui.vertical_centered(|ui| {
                ui.add_enabled_ui(!busy, |ui| {
                    if primary_button(
                        ui,
                        button_icon,
                        button_text,
                        Vec2::new(CARD_INNER_WIDTH, 48.0),
                    )
                    .clicked()
                    {
                        self.primary_action();
                    }
                });
                ui.add_space(8.0);
                self.draw_progress(ui);
            });

            ui.add_space(12.0);
            section_label(ui, "Install Notes");
            ui.add_space(6.0);
            for note in [
                "Keeps your local plugin build current.",
                "Creates a backup before replacing local files.",
                "Use after closing Roblox Studio for best results.",
            ] {
                ui.horizontal(|ui| {
                    draw_icon(ui, MiniIcon::Check, Vec2::splat(16.0), phase::green());
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new(note)
                            .font(FontId::proportional(13.0))
                            .color(phase::text_secondary()),
                    );
                });
                ui.add_space(4.0);
            }
        });
    }

    fn account_tab(&mut self, ui: &mut Ui) {
        draw_panel(ui, |ui| {
            section_label(ui, "Connection");
            ui.add_space(6.0);

            egui::Frame::none()
                .fill(phase::input())
                .stroke(Stroke::new(1.0, phase::line()))
                .rounding(Rounding::same(8.0))
                .inner_margin(Margin::symmetric(14.0, 12.0))
                .show(ui, |ui| {
                    ui.set_width(CARD_INNER_WIDTH - 16.0);
                    ui.horizontal(|ui| {
                        draw_icon(ui, MiniIcon::User, Vec2::splat(34.0), phase::accent());
                        ui.add_space(10.0);
                        ui.vertical(|ui| {
                            let title = self
                                .linked_user
                                .as_ref()
                                .map(display_linked_user)
                                .unwrap_or_else(|| "Phase account".to_owned());
                            ui.label(
                                RichText::new(title)
                                    .font(FontId::proportional(15.0))
                                    .strong()
                                    .color(phase::text()),
                            );
                            let detail = if self.plugin_token.is_some() {
                                "Connected to this installer"
                            } else if let Some(code) = &self.link_code {
                                code.as_str()
                            } else {
                                "Open browser to sign in and approve this install"
                            };
                            ui.label(
                                RichText::new(detail)
                                    .font(FontId::proportional(11.0))
                                    .color(phase::text_muted()),
                            );
                        });
                    });
                });

            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.set_width(CARD_INNER_WIDTH - 16.0);
                let busy = self.link_rx.is_some() || self.link_status_rx.is_some();
                let disconnecting = self.phase_disconnect_rx.is_some();
                let phase_linked = self.plugin_token.is_some();
                let button_width = (CARD_INNER_WIDTH - 32.0) / 3.0;

                if phase_linked {
                    status_action(
                        ui,
                        MiniIcon::Check,
                        "Connected",
                        Vec2::new(button_width, 36.0),
                    );
                } else {
                    let connect_label = if busy { "Waiting" } else { "Connect" };
                    ui.add_enabled_ui(!busy, |ui| {
                        if secondary_button(
                            ui,
                            MiniIcon::External,
                            connect_label,
                            Vec2::new(button_width, 36.0),
                        )
                        .clicked()
                        {
                            self.start_phase_account_link(ui.ctx());
                        }
                    });
                }

                if secondary_button(
                    ui,
                    MiniIcon::Refresh,
                    "Check",
                    Vec2::new(button_width, 36.0),
                )
                .clicked()
                {
                    if self.plugin_token.is_some() {
                        self.begin_phase_account_refresh(ui.ctx());
                    } else {
                        self.begin_link_status_check(ui.ctx());
                    }
                }
                if phase_linked {
                    ui.add_enabled_ui(!disconnecting, |ui| {
                        if secondary_button(
                            ui,
                            MiniIcon::Lock,
                            "Disconnect",
                            Vec2::new(button_width, 36.0),
                        )
                        .clicked()
                        {
                            self.start_phase_disconnect(ui.ctx());
                        }
                    });
                } else if let Some(url) = self.link_url.clone() {
                    if secondary_button(
                        ui,
                        MiniIcon::External,
                        "Open",
                        Vec2::new(button_width, 36.0),
                    )
                    .clicked()
                    {
                        if let Err(error) = open::that(url) {
                            self.log(phase::warning(), format!("Open browser failed: {error}"));
                        }
                    }
                }
            });

            ui.add_space(14.0);
            section_label(ui, "Verified Access");
            ui.add_space(6.0);
            egui::Frame::none()
                .fill(phase::input())
                .stroke(Stroke::new(1.0, phase::line()))
                .rounding(Rounding::same(8.0))
                .inner_margin(Margin::symmetric(12.0, 10.0))
                .show(ui, |ui| {
                    ui.set_width(CARD_INNER_WIDTH - 16.0);
                    ui.horizontal(|ui| {
                        draw_icon(ui, MiniIcon::Lock, Vec2::splat(24.0), phase::accent());
                        ui.add_space(8.0);
                        ui.vertical(|ui| {
                            ui.label(
                                RichText::new("Roblox OAuth")
                                    .font(FontId::proportional(14.0))
                                    .strong()
                                    .color(phase::text()),
                            );
                            let identity = self
                                .roblox_username
                                .as_deref()
                                .filter(|name| !name.trim().is_empty())
                                .or_else(|| {
                                    (!self.roblox_user_id.trim().is_empty())
                                        .then_some(self.roblox_user_id.as_str())
                                })
                                .unwrap_or("No Roblox account verified");
                            scrolling_label(
                                ui,
                                identity,
                                CARD_INNER_WIDTH - 68.0,
                                FontId::proportional(11.0),
                                phase::text_muted(),
                            );
                        });
                    });
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new("Phase license key")
                            .font(FontId::proportional(11.0))
                            .color(phase::text_muted()),
                    );
                    ui.add(
                        egui::TextEdit::singleline(&mut self.license_key)
                            .desired_width(CARD_INNER_WIDTH - 34.0)
                            .password(true)
                            .hint_text("Optional if Roblox ownership verifies"),
                    );
                });

            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.set_width(CARD_INNER_WIDTH - 16.0);
                let oauth_busy =
                    self.roblox_oauth_rx.is_some() || self.roblox_oauth_status_rx.is_some();
                let activation_busy = self.activation_rx.is_some();
                let verified_roblox = !self.roblox_user_id.trim().is_empty();
                let button_width = (CARD_INNER_WIDTH - 32.0) / 3.0;

                if verified_roblox {
                    status_action(
                        ui,
                        MiniIcon::Check,
                        "Verified",
                        Vec2::new(button_width, 36.0),
                    );
                } else {
                    let label = if oauth_busy { "Waiting" } else { "Roblox" };
                    ui.add_enabled_ui(!oauth_busy, |ui| {
                        if secondary_button(
                            ui,
                            MiniIcon::External,
                            label,
                            Vec2::new(button_width, 36.0),
                        )
                        .clicked()
                        {
                            self.start_roblox_oauth(ui.ctx());
                        }
                    });
                }
                if !verified_roblox {
                    if let Some(url) = self.roblox_oauth_url.clone() {
                        if secondary_button(
                            ui,
                            MiniIcon::External,
                            "Open",
                            Vec2::new(button_width, 36.0),
                        )
                        .clicked()
                        {
                            if let Err(error) = open::that(url) {
                                self.log(phase::warning(), format!("Open browser failed: {error}"));
                            }
                        }
                    }
                }
                ui.add_enabled_ui(!activation_busy && verified_roblox, |ui| {
                    if secondary_button(ui, MiniIcon::Key, "License", Vec2::new(button_width, 36.0))
                        .clicked()
                    {
                        self.start_activation(ui.ctx());
                    }
                });
                if verified_roblox {
                    if secondary_button(
                        ui,
                        MiniIcon::Lock,
                        "Disconnect",
                        Vec2::new(button_width, 36.0),
                    )
                    .clicked()
                    {
                        self.disconnect_roblox_account();
                    }
                }
            });

            if let Some(activation) = &self.activation {
                ui.add_space(8.0);
                info_row(ui, "Access", &activation.licensee);
                info_row(ui, "Mode", &activation.activation_mode);
            } else if let Some(error) = &self.activation_error {
                ui.add_space(8.0);
                scrolling_label(
                    ui,
                    error,
                    CARD_INNER_WIDTH - 16.0,
                    FontId::proportional(11.0),
                    phase::red(),
                );
            }
        });
    }

    fn folders_tab(&mut self, ui: &mut Ui) {
        let _time = ui.input(|i| i.time);
        let selected_text = self
            .selected_folder
            .as_ref()
            .map(|path| path.to_string_lossy().to_string())
            .unwrap_or_else(|| "No folder selected".to_owned());

        draw_panel(ui, |ui| {
            section_label(ui, "Install location");
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                draw_icon(ui, MiniIcon::Folder, Vec2::splat(40.0), phase::accent());
                ui.add_space(8.0);
                ui.vertical(|ui| {
                    ui.label(
                        RichText::new("ACTIVE ROBLOX PATH")
                            .font(FontId::proportional(10.0))
                            .strong()
                            .color(phase::text_muted()),
                    );
                    scrolling_label(
                        ui,
                        &selected_text,
                        CARD_INNER_WIDTH - 74.0,
                        FontId::monospace(13.0),
                        phase::text(),
                    );
                });
            });

            if let Some(candidate) = self.selected_candidate() {
                ui.add_space(8.0);
                egui::Frame::none()
                    .fill(phase::input())
                    .stroke(Stroke::new(1.0, phase::line()))
                    .rounding(Rounding::same(6.0))
                    .inner_margin(Margin::same(10.0))
                    .show(ui, |ui| {
                        ui.set_width(CARD_INNER_WIDTH - 16.0);
                        info_row(ui, "Local files", &candidate.plugin_files.len().to_string());
                        ui.add_space(4.0);
                        info_row(ui, "Source type", &candidate.source);
                        if let Some(plugin_file) = candidate.plugin_files.first() {
                            ui.add_space(4.0);
                            info_row(ui, "Active file", &human_size(plugin_file.size_bytes));
                            if plugin_file.modified.is_some() {
                                ui.add_space(4.0);
                                info_row(ui, "Backup state", "Recommended");
                            }
                        }
                    });
            }

            ui.add_space(10.0);
            section_label(ui, "Detected Paths");
            ui.add_space(4.0);
            self.folder_candidates(ui);

            ui.add_space(12.0);
            ui.horizontal(|ui| {
                let btn_width = (CARD_INNER_WIDTH - 16.0) / 3.0;
                if secondary_button(ui, MiniIcon::Folder, "Browse", Vec2::new(btn_width, 36.0))
                    .clicked()
                {
                    self.choose_folder();
                }
                if secondary_button(ui, MiniIcon::External, "Open", Vec2::new(btn_width, 36.0))
                    .clicked()
                {
                    self.open_folder();
                }
                if secondary_button(ui, MiniIcon::Refresh, "Rescan", Vec2::new(btn_width, 36.0))
                    .clicked()
                {
                    self.refresh_detection();
                }
            });
        });
    }

    fn options_tab(&mut self, ui: &mut Ui) {
        draw_panel(ui, |ui| {
            ui.vertical(|ui| {
                section_label(ui, "Customization");
                ui.add_space(8.0);

                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(
                            RichText::new("Release Channel")
                                .font(FontId::proportional(14.0))
                                .strong()
                                .color(phase::text()),
                        );
                        ui.label(
                            RichText::new("Default public updater track")
                                .font(FontId::proportional(11.0))
                                .color(phase::text_muted()),
                        );
                    });

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        status_pill(ui, "Default", phase::accent());
                    });
                });

                ui.add_space(14.0);

                section_label(ui, "Marketplace Themes");
                ui.add_space(6.0);
                egui::Frame::none()
                    .fill(phase::input())
                    .stroke(Stroke::new(1.0, phase::line()))
                    .rounding(Rounding::same(8.0))
                    .inner_margin(Margin::symmetric(THEME_ROW_MARGIN, 10.0))
                    .show(ui, |ui| {
                        ui.set_width(THEME_ROW_INNER_WIDTH);
                        let current = self
                            .selected_theme
                            .as_ref()
                            .map(|theme| theme.title.as_str())
                            .unwrap_or("Default Phase");
                        info_row_width(ui, "Active", current, THEME_ROW_INNER_WIDTH);
                        if let Some(theme) = &self.selected_theme {
                            if let Some(image_id) = &theme.background_image_id {
                                ui.add_space(4.0);
                                info_row_width(
                                    ui,
                                    "Background image",
                                    image_id,
                                    THEME_ROW_INNER_WIDTH,
                                );
                            }
                        }
                        ui.add_space(4.0);
                        info_row_width(
                            ui,
                            "Background mode",
                            self.theme_background_mode.label(),
                            THEME_ROW_INNER_WIDTH,
                        );
                    });

                ui.add_space(8.0);
                ui.horizontal_centered(|ui| {
                    let btn_width = (THEME_ROW_WIDTH - 16.0) / 3.0;
                    for mode in [
                        ThemeBackgroundMode::Crop,
                        ThemeBackgroundMode::Fit,
                        ThemeBackgroundMode::Stretch,
                    ] {
                        let selected = self.theme_background_mode == mode;
                        let label = if selected {
                            format!("{} ✓", mode.label())
                        } else {
                            mode.label().to_owned()
                        };
                        if secondary_button(ui, MiniIcon::Gear, &label, Vec2::new(btn_width, 34.0))
                            .clicked()
                        {
                            self.theme_background_mode = mode;
                            self.save_account_cache();
                            ui.ctx().request_repaint();
                        }
                    }
                });

                ui.add_space(8.0);
                ui.horizontal_centered(|ui| {
                    let btn_width = (THEME_ROW_WIDTH - 8.0) / 2.0;
                    let loading = self.theme_fetch_rx.is_some();
                    let label = if loading { "Loading" } else { "Refresh" };
                    ui.add_enabled_ui(!loading, |ui| {
                        if secondary_button(
                            ui,
                            MiniIcon::Refresh,
                            label,
                            Vec2::new(btn_width, 36.0),
                        )
                        .clicked()
                        {
                            self.begin_theme_fetch(ui.ctx());
                        }
                    });
                    if secondary_button(ui, MiniIcon::Gear, "Default", Vec2::new(btn_width, 36.0))
                        .clicked()
                    {
                        self.reset_theme(ui.ctx());
                    }
                });

                ui.add_space(8.0);
                self.theme_search_row(ui);

                ui.add_space(8.0);
                if self.theme_assets.is_empty() && self.theme_fetch_rx.is_some() {
                    scrolling_label(
                        ui,
                        "Loading Phase themes...",
                        THEME_ROW_WIDTH,
                        FontId::proportional(11.0),
                        phase::text_muted(),
                    );
                }

                let themes: Vec<_> = self
                    .theme_assets
                    .iter()
                    .filter(|asset| theme_matches_search(asset, &self.theme_search))
                    .cloned()
                    .collect();
                let visible_count = self.visible_theme_count.min(themes.len());
                for asset in themes.iter().take(visible_count).cloned() {
                    self.theme_asset_row(ui, asset);
                    ui.add_space(6.0);
                }

                if !themes.is_empty() && visible_count < themes.len() {
                    let remaining = themes.len() - visible_count;
                    ui.horizontal_centered(|ui| {
                        if secondary_button(
                            ui,
                            MiniIcon::Download,
                            &format!("Show more ({remaining})"),
                            Vec2::new(THEME_ROW_WIDTH, 36.0),
                        )
                        .clicked()
                        {
                            self.visible_theme_count =
                                (self.visible_theme_count + 6).min(themes.len());
                        }
                    });
                    ui.add_space(6.0);
                } else if themes.is_empty()
                    && !self.theme_assets.is_empty()
                    && !self.theme_search.trim().is_empty()
                {
                    scrolling_label(
                        ui,
                        "No themes match that search.",
                        THEME_ROW_WIDTH,
                        FontId::proportional(11.0),
                        phase::text_muted(),
                    );
                    ui.add_space(6.0);
                }

                if let Some(error) = &self.theme_error {
                    ui.add_space(4.0);
                    scrolling_label(
                        ui,
                        error,
                        THEME_ROW_WIDTH,
                        FontId::proportional(11.0),
                        phase::warning(),
                    );
                }

                ui.add_space(16.0);

                // Group checkmark preferences into a sleek card container
                egui::Frame::none()
                    .fill(phase::input())
                    .stroke(Stroke::new(1.0, phase::line()))
                    .rounding(Rounding::same(8.0))
                    .inner_margin(Margin::symmetric(14.0, 12.0))
                    .show(ui, |ui| {
                        ui.set_width(CARD_INNER_WIDTH - 16.0);
                        ui.vertical(|ui| {
                            ui.checkbox(
                                &mut self.backup_before_install,
                                RichText::new("Back up current plugin first")
                                    .font(FontId::proportional(13.0))
                                    .color(phase::text_secondary()),
                            );
                            ui.add_space(10.0);
                            ui.checkbox(
                                &mut self.restart_studio_hint,
                                RichText::new("Show Roblox Studio restart reminder")
                                    .font(FontId::proportional(13.0))
                                    .color(phase::text_secondary()),
                            );
                        });
                    });

                ui.add_space(16.0);

                section_label(ui, "App Updates");
                ui.add_space(6.0);
                egui::Frame::none()
                    .fill(phase::input())
                    .stroke(Stroke::new(1.0, phase::line()))
                    .rounding(Rounding::same(8.0))
                    .inner_margin(Margin::symmetric(14.0, 10.0))
                    .show(ui, |ui| {
                        ui.set_width(CARD_INNER_WIDTH - 16.0);
                        info_row(ui, "Installed", env!("CARGO_PKG_VERSION"));
                        ui.add_space(4.0);
                        let latest = self
                            .app_update
                            .as_ref()
                            .map(|update| update.version.as_str())
                            .unwrap_or("Current");
                        info_row(ui, "Latest", latest);
                        if let Some(update) = &self.app_update {
                            ui.add_space(4.0);
                            info_row(ui, "Package", &update.asset_name);
                        }
                    });

                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    let btn_width = (CARD_INNER_WIDTH - 8.0) / 2.0;
                    let checking = self.app_update_rx.is_some();
                    let installing = self.app_update_install_rx.is_some();
                    let check_label = if checking { "Checking" } else { "Check" };
                    ui.add_enabled_ui(!checking && !installing, |ui| {
                        if secondary_button(
                            ui,
                            MiniIcon::Refresh,
                            check_label,
                            Vec2::new(btn_width, 36.0),
                        )
                        .clicked()
                        {
                            self.begin_app_update_check(ui.ctx());
                        }
                    });

                    let install_label = if installing { "Starting" } else { "Install" };
                    ui.add_enabled_ui(self.app_update.is_some() && !installing, |ui| {
                        if secondary_button(
                            ui,
                            MiniIcon::Download,
                            install_label,
                            Vec2::new(btn_width, 36.0),
                        )
                        .clicked()
                        {
                            self.start_app_update_install(ui.ctx());
                        }
                    });
                });

                if let Some(error) = &self.app_update_error {
                    ui.add_space(6.0);
                    scrolling_label(
                        ui,
                        error,
                        CARD_INNER_WIDTH - 16.0,
                        FontId::proportional(11.0),
                        phase::warning(),
                    );
                }

                ui.add_space(16.0);

                section_label(ui, "About");
                ui.add_space(6.0);
                egui::Frame::none()
                    .fill(phase::input())
                    .stroke(Stroke::new(1.0, phase::line()))
                    .rounding(Rounding::same(8.0))
                    .inner_margin(Margin::symmetric(14.0, 10.0))
                    .show(ui, |ui| {
                        ui.set_width(CARD_INNER_WIDTH - 16.0);
                        ui.horizontal(|ui| {
                            draw_icon(ui, MiniIcon::Lock, Vec2::splat(13.0), phase::text_muted());
                            ui.add_space(6.0);
                            ui.label(
                                RichText::new("Phase Animator installer settings")
                                    .font(FontId::proportional(12.0))
                                    .color(phase::text_muted()),
                            );
                        });
                    });
            });
        });
    }

    fn theme_asset_row(&mut self, ui: &mut Ui, asset: verification::PhaseThemeAsset) {
        egui::Frame::none()
            .fill(phase::surface())
            .stroke(Stroke::new(1.0, phase::line()))
            .rounding(Rounding::same(8.0))
            .inner_margin(Margin::symmetric(THEME_ROW_MARGIN, 10.0))
            .show(ui, |ui| {
                ui.set_width(THEME_ROW_INNER_WIDTH);
                ui.horizontal(|ui| {
                    draw_icon(ui, MiniIcon::Gear, Vec2::splat(18.0), phase::accent());
                    ui.add_space(4.0);
                    ui.vertical(|ui| {
                        scrolling_label(
                            ui,
                            &asset.title,
                            THEME_ROW_WIDTH - 150.0,
                            FontId::proportional(13.0),
                            phase::text(),
                        );
                        let author = asset
                            .owner
                            .as_ref()
                            .map(display_theme_owner)
                            .unwrap_or_else(|| "Phase marketplace".to_owned());
                        scrolling_label(
                            ui,
                            &format!("{} installs · {}", asset.install_count, author),
                            THEME_ROW_WIDTH - 150.0,
                            FontId::proportional(10.5),
                            phase::text_muted(),
                        );
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let applying = self.theme_apply_rx.is_some();
                        ui.add_enabled_ui(!applying, |ui| {
                            if secondary_button(
                                ui,
                                MiniIcon::Download,
                                if applying { "Applying" } else { "Apply" },
                                Vec2::new(92.0, 34.0),
                            )
                            .clicked()
                            {
                                self.start_theme_apply(ui.ctx(), asset);
                            }
                        });
                    });
                });
            });
    }

    fn theme_search_row(&mut self, ui: &mut Ui) {
        let width = THEME_ROW_WIDTH;
        let height = 40.0;
        let (rect, response) = ui.allocate_exact_size(Vec2::new(width, height), Sense::click());
        if response.clicked() {
            response.request_focus();
        }

        let focused = response.has_focus();
        let border = if focused {
            phase::accent()
        } else if response.hovered() {
            phase::accent_dim()
        } else {
            phase::line()
        };
        ui.painter()
            .rect_filled(rect, Rounding::same(8.0), phase::surface());
        ui.painter()
            .rect_stroke(rect, Rounding::same(8.0), Stroke::new(1.0, border));

        let icon_rect = Rect::from_center_size(
            Pos2::new(rect.left() + 24.0, rect.center().y),
            Vec2::splat(15.0),
        );
        draw_icon_at(
            ui.painter(),
            icon_rect,
            MiniIcon::Search,
            phase::text_muted(),
        );

        let text_rect = Rect::from_min_max(
            Pos2::new(rect.left() + 46.0, rect.top() + 7.0),
            Pos2::new(rect.right() - 44.0, rect.bottom() - 6.0),
        );
        ui.allocate_ui_at_rect(text_rect, |ui| {
            ui.visuals_mut().widgets.inactive.bg_fill = Color32::TRANSPARENT;
            ui.visuals_mut().widgets.hovered.bg_fill = Color32::TRANSPARENT;
            ui.visuals_mut().widgets.active.bg_fill = Color32::TRANSPARENT;
            ui.visuals_mut().widgets.inactive.bg_stroke = Stroke::NONE;
            ui.visuals_mut().widgets.hovered.bg_stroke = Stroke::NONE;
            ui.visuals_mut().widgets.active.bg_stroke = Stroke::NONE;
            let response = ui.add_sized(
                text_rect.size(),
                egui::TextEdit::singleline(&mut self.theme_search)
                    .hint_text("Search marketplace themes")
                    .font(FontId::proportional(13.0))
                    .desired_width(text_rect.width())
                    .frame(false),
            );
            if response.changed() {
                self.visible_theme_count = 6;
            }
        });

        if !self.theme_search.is_empty() {
            let clear_rect = Rect::from_center_size(
                Pos2::new(rect.right() - 22.0, rect.center().y),
                Vec2::splat(24.0),
            );
            let clear_response = ui.interact(clear_rect, ui.next_auto_id(), Sense::click());
            let clear_color = if clear_response.hovered() {
                phase::text()
            } else {
                phase::text_muted()
            };
            ui.painter().text(
                clear_rect.center(),
                Align2::CENTER_CENTER,
                "x",
                FontId::proportional(14.0),
                clear_color,
            );
            if clear_response.clicked() {
                self.theme_search.clear();
                self.visible_theme_count = 6;
            }
        }
    }

    fn folder_candidates(&mut self, ui: &mut Ui) {
        let rows: Vec<_> = self.candidates.iter().cloned().collect();
        ui.add_space(2.0);
        for candidate in rows.into_iter().take(2) {
            let selected = self
                .selected_folder
                .as_ref()
                .is_some_and(|path| normalize_path(path) == normalize_path(&candidate.path));

            let width = ui.available_width();
            let (rect, response) = ui.allocate_exact_size(Vec2::new(width, 56.0), Sense::click());
            let painter = ui.painter();

            let bg_color = if selected {
                phase::input()
            } else if response.hovered() {
                phase::surface_hover()
            } else {
                phase::surface()
            };

            let border_color = if selected {
                phase::accent()
            } else {
                phase::line()
            };

            painter.rect_filled(rect, Rounding::same(8.0), bg_color);
            painter.rect_stroke(rect, Rounding::same(8.0), Stroke::new(1.0, border_color));

            let dot_color = match candidate.health {
                FolderHealth::Ready => phase::green(),
                FolderHealth::Empty => phase::warning(),
                FolderHealth::Missing => phase::red(),
            };

            let dot_center = Pos2::new(rect.left() + 23.0, rect.center().y);
            painter.circle_filled(dot_center, 5.5, dot_color);

            let path_text = candidate.path.to_string_lossy().to_string();
            let health_lbl = health_label(&candidate.health);
            let text_color = if selected {
                phase::text()
            } else {
                phase::text_secondary()
            };
            let title_rect = Rect::from_min_size(
                Pos2::new(rect.left() + 40.0, rect.top() + 8.0),
                Vec2::new(rect.width() - 52.0, 19.0),
            );
            ui.allocate_ui_at_rect(title_rect, |ui| {
                scrolling_label(
                    ui,
                    &format!("{health_lbl}  {path_text}"),
                    title_rect.width(),
                    FontId::monospace(12.0),
                    text_color,
                );
            });

            let source_rect = Rect::from_min_size(
                Pos2::new(rect.left() + 40.0, rect.bottom() - 23.0),
                Vec2::new(rect.width() - 52.0, 18.0),
            );
            ui.allocate_ui_at_rect(source_rect, |ui| {
                scrolling_label(
                    ui,
                    &format!("Source: {}", candidate.source),
                    source_rect.width(),
                    FontId::proportional(10.5),
                    phase::text_muted(),
                );
            });

            if response.clicked() {
                self.selected_folder = Some(candidate.path.clone());
                self.log(
                    phase::blue(),
                    format!("Selected {}", compact_path(&candidate.path, 30)),
                );
            }
            ui.add_space(6.0);
        }
    }

    fn activity_block(&self, ui: &mut Ui) {
        section_label(ui, "System Console");

        ui.scope(|ui| {
            ui.set_min_width(CARD_WIDTH);
            ui.set_max_width(CARD_WIDTH);

            egui::Frame::none()
                .fill(Color32::from_rgb(10, 8, 16))
                .stroke(Stroke::new(1.0, phase::line()))
                .rounding(Rounding::same(6.0))
                .inner_margin(Margin::same(8.0))
                .show(ui, |ui| {
                    ui.set_min_width(CARD_INNER_WIDTH);
                    ui.set_max_width(CARD_INNER_WIDTH);
                    ui.set_min_height(122.0);
                    ui.set_max_height(122.0);

                    ui.vertical(|ui| {
                        for (idx, line) in self.activity.iter().enumerate() {
                            ui.horizontal(|ui| {
                                let time_prefix = format!("[{:03.1}s] ", idx as f32 * 0.4);
                                ui.label(
                                    RichText::new(time_prefix)
                                        .font(FontId::monospace(11.5))
                                        .color(phase::text_muted()),
                                );

                                let text_color = if line.color == phase::red() {
                                    phase::red()
                                } else if line.color == phase::green() {
                                    Color32::from_rgb(120, 230, 150)
                                } else {
                                    phase::text_secondary()
                                };

                                scrolling_label(
                                    ui,
                                    &line.text,
                                    CARD_INNER_WIDTH - 76.0,
                                    FontId::monospace(11.5),
                                    text_color,
                                );
                            });
                        }
                    });
                });
        });
    }
}

fn configure_style(ctx: &Context) {
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        PHOSPHOR_FONT.to_owned(),
        FontData::from_static(include_bytes!("../assets/Phosphor.ttf")),
    );
    let mut icon_family = vec![PHOSPHOR_FONT.to_owned()];
    if let Some(default_fonts) = fonts.families.get(&FontFamily::Proportional) {
        icon_family.extend(default_fonts.iter().cloned());
    }
    fonts
        .families
        .insert(FontFamily::Name(PHOSPHOR_FONT.into()), icon_family);
    ctx.set_fonts(fonts);

    let mut style = (*ctx.style()).clone();
    style.visuals.dark_mode = true;
    style.visuals.panel_fill = phase::background();
    style.visuals.window_fill = phase::background();
    style.visuals.widgets.noninteractive.bg_fill = phase::surface();
    style.visuals.widgets.inactive.bg_fill = phase::input();
    style.visuals.widgets.hovered.bg_fill = phase::surface_hover();
    style.visuals.widgets.active.bg_fill = phase::surface_active();
    style.visuals.selection.bg_fill = phase::accent_dim();
    style.visuals.selection.stroke = Stroke::new(1.0, phase::accent());
    style.spacing.item_spacing = Vec2::new(8.0, 8.0);
    style.spacing.button_padding = Vec2::new(12.0, 8.0);
    ctx.set_style(style);
}

fn load_logo(ctx: &Context) -> Option<TextureHandle> {
    let bytes = include_bytes!("../assets/PhaseAnimator.png");
    let image = image::load_from_memory(bytes).ok()?.to_rgba8();
    let size = [image.width() as usize, image.height() as usize];
    let pixels = image.into_raw();
    let color_image = ColorImage::from_rgba_unmultiplied(size, &pixels);
    Some(ctx.load_texture("phase-animator-logo", color_image, TextureOptions::LINEAR))
}

fn load_window_icon() -> IconData {
    let bytes = include_bytes!("../assets/PhaseAnimator.png");
    let Ok(image) = image::load_from_memory(bytes).map(|image| image.to_rgba8()) else {
        return IconData::default();
    };

    let width = image.width();
    let height = image.height();
    let side = width.min(height).max(1);
    let offset_x = (width - side) / 2;
    let offset_y = (height - side) / 2;
    let mut square = image::RgbaImage::new(side, side);
    for y in 0..side {
        for x in 0..side {
            square.put_pixel(x, y, *image.get_pixel(offset_x + x, offset_y + y));
        }
    }

    let resized = image::imageops::resize(&square, 256, 256, image::imageops::FilterType::Lanczos3);
    IconData {
        rgba: resized.into_raw(),
        width: 256,
        height: 256,
    }
}

#[cfg(target_os = "windows")]
fn apply_windows_title_bar(frame: &eframe::Frame) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows_sys::Win32::Graphics::Dwm::{
        DWMWA_BORDER_COLOR, DWMWA_CAPTION_COLOR, DWMWA_TEXT_COLOR, DwmSetWindowAttribute,
    };

    let Ok(handle) = frame.window_handle() else {
        return;
    };
    let RawWindowHandle::Win32(window) = handle.as_raw() else {
        return;
    };

    let hwnd = window.hwnd.get() as *mut core::ffi::c_void;
    let caption = color_ref(phase::surface());
    let text = color_ref(phase::text());
    let border = color_ref(phase::accent_dim());

    unsafe {
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_CAPTION_COLOR as u32,
            &caption as *const _ as *const _,
            core::mem::size_of_val(&caption) as u32,
        );
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_TEXT_COLOR as u32,
            &text as *const _ as *const _,
            core::mem::size_of_val(&text) as u32,
        );
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_BORDER_COLOR as u32,
            &border as *const _ as *const _,
            core::mem::size_of_val(&border) as u32,
        );
    }
}

#[cfg(not(target_os = "windows"))]
fn apply_windows_title_bar(_frame: &eframe::Frame) {}

fn color_ref(color: Color32) -> u32 {
    (color.r() as u32) | ((color.g() as u32) << 8) | ((color.b() as u32) << 16)
}

fn spawn_avatar_fetch(
    tx: Sender<AvatarFetchResult>,
    kind: AvatarKind,
    key: String,
    ctx: Context,
    loader: impl FnOnce() -> Result<Vec<u8>, String> + Send + 'static,
) {
    std::thread::spawn(move || {
        let image = loader().and_then(decode_avatar_image);
        let _ = tx.send(AvatarFetchResult { kind, key, image });
        ctx.request_repaint();
    });
}

fn spawn_theme_background_fetch(
    tx: Sender<ThemeBackgroundFetchResult>,
    key: String,
    ctx: Context,
    loader: impl FnOnce() -> Result<Vec<u8>, String> + Send + 'static,
) {
    std::thread::spawn(move || {
        let image = loader().and_then(decode_texture_image);
        let _ = tx.send(ThemeBackgroundFetchResult { key, image });
        ctx.request_repaint();
    });
}

fn decode_texture_image(bytes: Vec<u8>) -> Result<ColorImage, String> {
    let image = image::load_from_memory(&bytes)
        .map_err(|error| format!("Invalid theme background image: {error}"))?
        .to_rgba8();
    let size = [image.width() as usize, image.height() as usize];
    let pixels = image.into_raw();
    Ok(ColorImage::from_rgba_unmultiplied(size, &pixels))
}

fn decode_avatar_image(bytes: Vec<u8>) -> Result<ColorImage, String> {
    let image = image::load_from_memory(&bytes)
        .map_err(|error| format!("Invalid avatar image: {error}"))?
        .to_rgba8();

    let source_width = image.width();
    let source_height = image.height();
    let side = source_width.min(source_height).max(1);
    let offset_x = (source_width - side) / 2;
    let offset_y = (source_height - side) / 2;
    let mut square = image::RgbaImage::new(side, side);
    for y in 0..side {
        for x in 0..side {
            let pixel = *image.get_pixel(offset_x + x, offset_y + y);
            square.put_pixel(x, y, pixel);
        }
    }

    let center = (side as f32 - 1.0) * 0.5;
    let radius = side as f32 * 0.5;
    let soft_edge = radius.max(1.0) - 1.5;
    for y in 0..side {
        for x in 0..side {
            let dx = x as f32 - center;
            let dy = y as f32 - center;
            let dist = (dx * dx + dy * dy).sqrt();
            let alpha_scale = if dist <= soft_edge {
                1.0
            } else if dist <= radius {
                ((radius - dist) / (radius - soft_edge)).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let pixel = square.get_pixel_mut(x, y);
            pixel.0[3] = ((pixel.0[3] as f32) * alpha_scale) as u8;
        }
    }

    let size = [side as usize, side as usize];
    let pixels = square.into_raw();
    Ok(ColorImage::from_rgba_unmultiplied(size, &pixels))
}

fn identity_card(
    ui: &mut Ui,
    avatar: Option<&TextureHandle>,
    name: &str,
    detail: &str,
    width: f32,
    accent: Color32,
) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 54.0), Sense::hover());
    {
        let painter = ui.painter();
        painter.rect_filled(rect, Rounding::same(8.0), phase::input());
        painter.rect_stroke(rect, Rounding::same(8.0), Stroke::new(1.0, phase::line()));

        let avatar_rect = Rect::from_center_size(
            Pos2::new(rect.left() + 29.0, rect.center().y),
            Vec2::splat(34.0),
        );
        painter.circle_filled(avatar_rect.center(), 17.0, accent.linear_multiply(0.22));
        if let Some(texture) = avatar {
            painter.image(
                texture.id(),
                avatar_rect.shrink(1.0),
                Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                Color32::WHITE,
            );
        } else {
            painter.text(
                avatar_rect.center(),
                Align2::CENTER_CENTER,
                initials(name),
                FontId::proportional(13.0),
                phase::text(),
            );
        }
        painter.circle_stroke(avatar_rect.center(), 17.0, Stroke::new(1.5, accent));
        let dot = Pos2::new(avatar_rect.right() - 3.0, avatar_rect.bottom() - 4.0);
        painter.circle_filled(dot, 5.0, phase::input());
        painter.circle_filled(dot, 3.4, phase::green());
    }

    let text_x = rect.left() + 60.0;
    let text_width = rect.right() - text_x - 14.0;
    let name_rect = Rect::from_min_size(
        Pos2::new(text_x, rect.top() + 6.0),
        Vec2::new(text_width, 21.0),
    );
    ui.allocate_ui_at_rect(name_rect, |ui| {
        scrolling_label(
            ui,
            name,
            text_width,
            FontId::proportional(13.5),
            phase::text(),
        );
    });
    let detail_rect = Rect::from_min_size(
        Pos2::new(text_x, rect.top() + 27.0),
        Vec2::new(text_width, 18.0),
    );
    ui.allocate_ui_at_rect(detail_rect, |ui| {
        scrolling_label(
            ui,
            detail,
            text_width,
            FontId::proportional(10.5),
            phase::text_muted(),
        );
    });
}

fn draw_card(ui: &mut Ui, height: Option<f32>, add_contents: impl FnOnce(&mut Ui)) {
    ui.scope(|ui| {
        ui.set_min_width(CARD_WIDTH);
        ui.set_max_width(CARD_WIDTH);

        let frame = egui::Frame::none()
            .fill(phase::surface())
            .stroke(Stroke::new(1.0, phase::line()))
            .rounding(Rounding::same(8.0))
            .inner_margin(Margin::symmetric(14.0, 12.0));

        frame.show(ui, |ui| {
            ui.set_min_width(CARD_INNER_WIDTH);
            ui.set_max_width(CARD_INNER_WIDTH);
            if let Some(h) = height {
                ui.set_min_height(h);
            }
            add_contents(ui);
        });
    });
}

fn draw_panel(ui: &mut Ui, add_contents: impl FnOnce(&mut Ui)) {
    draw_card(ui, None, add_contents);
}

fn section_label(ui: &mut Ui, text: &str) {
    ui.label(
        RichText::new(text.to_uppercase())
            .font(FontId::proportional(10.0))
            .color(phase::text_muted()),
    );
}

#[derive(Clone, Copy)]
enum MiniIcon {
    Bolt,
    Check,
    Download,
    External,
    Folder,
    Gear,
    Key,
    Lock,
    Refresh,
    Rocket,
    Search,
    User,
}

impl MiniIcon {
    fn glyph(self) -> &'static str {
        match self {
            MiniIcon::Bolt => "\u{E1B2}",
            MiniIcon::Check => "\u{E184}",
            MiniIcon::Download => "\u{E20A}",
            MiniIcon::External => "\u{E5DE}",
            MiniIcon::Folder => "\u{E24A}",
            MiniIcon::Gear => "\u{E272}",
            MiniIcon::Key => "\u{E2A8}",
            MiniIcon::Lock => "\u{E308}",
            MiniIcon::Refresh => "\u{E094}",
            MiniIcon::Rocket => "\u{E3FE}",
            MiniIcon::Search => "\u{E4A6}",
            MiniIcon::User => "\u{E4D6}",
        }
    }
}

fn draw_icon(ui: &mut Ui, icon: MiniIcon, size: Vec2, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    draw_icon_at(ui.painter(), rect, icon, color);
}

fn draw_icon_at(painter: &egui::Painter, rect: Rect, icon: MiniIcon, color: Color32) {
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        icon.glyph(),
        FontId::new(
            rect.height().min(rect.width()) * 0.92,
            FontFamily::Name(PHOSPHOR_FONT.into()),
        ),
        color,
    );

    // Old hand-drawn fallback. Leaving it here for a bit until we are sure the
    // bundled Phosphor font loads reliably on both Windows and macOS builds.
    return;

    #[allow(unreachable_code)]
    match icon {
        MiniIcon::Bolt => {
            let min = rect.min;
            let w = rect.width();
            let h = rect.height();
            let points = vec![
                Pos2::new(min.x + w * 0.56, min.y),
                Pos2::new(min.x + w * 0.16, min.y + h * 0.55),
                Pos2::new(min.x + w * 0.46, min.y + h * 0.55),
                Pos2::new(min.x + w * 0.34, min.y + h),
                Pos2::new(min.x + w * 0.86, min.y + h * 0.42),
                Pos2::new(min.x + w * 0.56, min.y + h * 0.42),
            ];
            painter.add(egui::Shape::convex_polygon(points, color, Stroke::NONE));
        }
        MiniIcon::Check => {
            let min = rect.min;
            let w = rect.width();
            let h = rect.height();
            painter.line_segment(
                [
                    Pos2::new(min.x + w * 0.16, min.y + h * 0.52),
                    Pos2::new(min.x + w * 0.42, min.y + h * 0.76),
                ],
                Stroke::new(2.0, color),
            );
            painter.line_segment(
                [
                    Pos2::new(min.x + w * 0.42, min.y + h * 0.76),
                    Pos2::new(min.x + w * 0.84, min.y + h * 0.24),
                ],
                Stroke::new(2.0, color),
            );
        }
        MiniIcon::Download => {
            let center = rect.center();
            let min = rect.min;
            let w = rect.width();
            let h = rect.height();
            painter.line_segment(
                [
                    Pos2::new(center.x, min.y + h * 0.14),
                    Pos2::new(center.x, min.y + h * 0.62),
                ],
                Stroke::new(2.0, color),
            );
            painter.line_segment(
                [
                    Pos2::new(min.x + w * 0.28, min.y + h * 0.44),
                    Pos2::new(center.x, min.y + h * 0.68),
                ],
                Stroke::new(2.0, color),
            );
            painter.line_segment(
                [
                    Pos2::new(center.x, min.y + h * 0.68),
                    Pos2::new(min.x + w * 0.72, min.y + h * 0.44),
                ],
                Stroke::new(2.0, color),
            );
            painter.line_segment(
                [
                    Pos2::new(min.x + w * 0.2, min.y + h * 0.84),
                    Pos2::new(min.x + w * 0.8, min.y + h * 0.84),
                ],
                Stroke::new(2.0, color),
            );
        }
        MiniIcon::External => {
            let min = rect.min;
            let w = rect.width();
            let h = rect.height();
            let box_rect = Rect::from_min_max(
                Pos2::new(min.x + w * 0.1, min.y + h * 0.28),
                Pos2::new(min.x + w * 0.72, min.y + h * 0.9),
            );
            painter.rect_stroke(box_rect, Rounding::same(2.0), Stroke::new(1.6, color));
            painter.line_segment(
                [
                    Pos2::new(min.x + w * 0.44, min.y + h * 0.18),
                    Pos2::new(min.x + w * 0.86, min.y + h * 0.18),
                ],
                Stroke::new(1.8, color),
            );
            painter.line_segment(
                [
                    Pos2::new(min.x + w * 0.86, min.y + h * 0.18),
                    Pos2::new(min.x + w * 0.86, min.y + h * 0.6),
                ],
                Stroke::new(1.8, color),
            );
            painter.line_segment(
                [
                    Pos2::new(min.x + w * 0.84, min.y + h * 0.2),
                    Pos2::new(min.x + w * 0.5, min.y + h * 0.54),
                ],
                Stroke::new(1.8, color),
            );
        }
        MiniIcon::Folder => {
            let min = rect.min;
            let w = rect.width();
            let h = rect.height();
            let body_rect = Rect::from_min_max(
                Pos2::new(min.x, min.y + h * 0.28),
                Pos2::new(min.x + w, min.y + h),
            );
            let tab_rect = Rect::from_min_max(
                Pos2::new(min.x + w * 0.04, min.y + h * 0.08),
                Pos2::new(min.x + w * 0.48, min.y + h * 0.38),
            );
            painter.rect_filled(tab_rect, Rounding::same(2.0), color);
            painter.rect_filled(body_rect, Rounding::same(3.0), color.linear_multiply(0.14));
            painter.rect_stroke(body_rect, Rounding::same(3.0), Stroke::new(1.8, color));
        }
        MiniIcon::Gear => {
            let center = rect.center();
            let r = rect.width().min(rect.height()) * 0.3;
            painter.circle_stroke(center, r, Stroke::new(1.7, color));
            painter.circle_filled(center, r * 0.38, color);
            for i in 0..8 {
                let angle = i as f32 * std::f32::consts::TAU / 8.0;
                let dir = Vec2::new(angle.cos(), angle.sin());
                painter.line_segment(
                    [center + dir * (r + 1.5), center + dir * (r + 4.0)],
                    Stroke::new(2.0, color),
                );
            }
        }
        MiniIcon::Lock => {
            let min = rect.min;
            let w = rect.width();
            let h = rect.height();
            let body_rect = Rect::from_min_max(
                Pos2::new(min.x + w * 0.12, min.y + h * 0.44),
                Pos2::new(min.x + w * 0.88, min.y + h * 0.94),
            );
            painter.rect_filled(body_rect, Rounding::same(2.0), color);
            let shackle = Rect::from_min_max(
                Pos2::new(min.x + w * 0.28, min.y + h * 0.08),
                Pos2::new(min.x + w * 0.72, min.y + h * 0.58),
            );
            painter.rect_stroke(shackle, Rounding::same(999.0), Stroke::new(1.7, color));
        }
        MiniIcon::Refresh => {
            let c = rect.center();
            let r = rect.width().min(rect.height()) * 0.34;
            let stroke = Stroke::new(1.9, color);
            let p1 = Pos2::new(c.x - r, c.y - r * 0.1);
            let p2 = Pos2::new(c.x - r * 0.36, c.y - r * 0.78);
            let p3 = Pos2::new(c.x + r * 0.54, c.y - r * 0.58);
            let p4 = Pos2::new(c.x + r, c.y + r * 0.05);
            painter.line_segment([p1, p2], stroke);
            painter.line_segment([p2, p3], stroke);
            painter.line_segment([p3, p4], stroke);
            painter.line_segment([p4, Pos2::new(p4.x - r * 0.34, p4.y - r * 0.22)], stroke);
            painter.line_segment([p4, Pos2::new(p4.x - r * 0.04, p4.y - r * 0.42)], stroke);
        }
        MiniIcon::Search => {
            let c = Pos2::new(
                rect.center().x - rect.width() * 0.06,
                rect.center().y - rect.height() * 0.06,
            );
            let r = rect.width().min(rect.height()) * 0.25;
            let stroke = Stroke::new(1.7, color);
            painter.circle_stroke(c, r, stroke);
            painter.line_segment(
                [
                    Pos2::new(c.x + r * 0.72, c.y + r * 0.72),
                    Pos2::new(c.x + r * 1.42, c.y + r * 1.42),
                ],
                stroke,
            );
        }
        MiniIcon::Rocket => {
            let center = rect.center();
            let w = rect.width();
            let h = rect.height();
            let body = Rect::from_center_size(center, Vec2::new(w * 0.28, h * 0.52));
            painter.rect_filled(body, Rounding::same(4.0), color.linear_multiply(0.12));
            painter.rect_stroke(body, Rounding::same(4.0), Stroke::new(1.8, color));
            painter.add(egui::Shape::convex_polygon(
                vec![
                    Pos2::new(center.x, center.y - h * 0.44),
                    Pos2::new(center.x - w * 0.15, center.y - h * 0.18),
                    Pos2::new(center.x + w * 0.15, center.y - h * 0.18),
                ],
                color,
                Stroke::NONE,
            ));
            painter.add(egui::Shape::convex_polygon(
                vec![
                    Pos2::new(center.x - w * 0.14, center.y + h * 0.08),
                    Pos2::new(center.x - w * 0.34, center.y + h * 0.32),
                    Pos2::new(center.x - w * 0.14, center.y + h * 0.28),
                ],
                color,
                Stroke::NONE,
            ));
            painter.add(egui::Shape::convex_polygon(
                vec![
                    Pos2::new(center.x + w * 0.14, center.y + h * 0.08),
                    Pos2::new(center.x + w * 0.34, center.y + h * 0.32),
                    Pos2::new(center.x + w * 0.14, center.y + h * 0.28),
                ],
                color,
                Stroke::NONE,
            ));
        }
        MiniIcon::Key | MiniIcon::User => {}
    }
}

fn draw_release_icon(ui: &mut Ui, icon: MiniIcon) {
    draw_icon(ui, icon, Vec2::splat(52.0), phase::accent_hover());
}

fn release_metric(ui: &mut Ui, icon: MiniIcon, label: &str, value: &str) {
    egui::Frame::none()
        .fill(phase::input())
        .stroke(Stroke::new(1.0, phase::line()))
        .rounding(Rounding::same(8.0))
        .inner_margin(Margin::symmetric(18.0, 14.0))
        .show(ui, |ui| {
            ui.set_width(CARD_INNER_WIDTH - 16.0);
            ui.horizontal(|ui| {
                ui.add_space(4.0);
                draw_release_icon(ui, icon);
                ui.add_space(18.0);
                ui.vertical(|ui| {
                    ui.label(
                        RichText::new(label.to_uppercase())
                            .font(FontId::proportional(10.5))
                            .color(phase::text_muted()),
                    );
                    scrolling_label(
                        ui,
                        value,
                        CARD_INNER_WIDTH - 96.0,
                        FontId::proportional(18.0),
                        phase::text(),
                    );
                });
            });
        });
}

fn info_row(ui: &mut Ui, label: &str, value: &str) {
    info_row_width(ui, label, value, CARD_INNER_WIDTH - 16.0);
}

fn info_row_width(ui: &mut Ui, label: &str, value: &str, width: f32) {
    ui.horizontal(|ui| {
        ui.set_width(width);
        let label_width = 96.0;
        let gap = 8.0;
        let value_width = (width - label_width - gap).max(80.0);
        ui.add_sized(
            Vec2::new(label_width, 20.0),
            egui::Label::new(
                RichText::new(label)
                    .font(FontId::proportional(13.0))
                    .color(phase::text_muted()),
            )
            .wrap(false),
        );
        ui.add_space(gap);
        scrolling_label(
            ui,
            value,
            value_width,
            FontId::proportional(13.0),
            phase::text_secondary(),
        );
    });
}

fn scrolling_label(ui: &mut Ui, text: &str, width: f32, font: FontId, color: Color32) {
    let height = font.size + 8.0;
    egui::ScrollArea::horizontal()
        .id_source(ui.next_auto_id())
        .max_width(width)
        .max_height(height)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.set_min_width(width);
            ui.add(egui::Label::new(RichText::new(text).font(font).color(color)).wrap(false));
        });
}

fn status_pill(ui: &mut Ui, text: &str, color: Color32) {
    let width = (text.chars().count() as f32 * 6.5 + 24.0).max(56.0);
    egui::Frame::none()
        .fill(phase::input())
        .stroke(Stroke::new(1.0, color))
        .rounding(Rounding::same(999.0))
        .inner_margin(Margin::symmetric(10.0, 4.0))
        .show(ui, |ui| {
            ui.add_sized(
                Vec2::new(width, 15.0),
                egui::Label::new(
                    RichText::new(text)
                        .font(FontId::proportional(11.0))
                        .color(color),
                )
                .wrap(false),
            );
        });
}

fn phase_text(phase: InstallPhase) -> &'static str {
    let _ = phase;
    match phase {
        InstallPhase::Idle => "Ready",
        InstallPhase::Checking => "Checking...",
        InstallPhase::Ready => "Update ready",
        InstallPhase::Downloading => "Downloading...",
        InstallPhase::Installing => "Installing...",
        InstallPhase::Complete => "Complete",
        InstallPhase::Error => "Needs location",
    }
}

fn phase_color(phase: InstallPhase) -> Color32 {
    match phase {
        InstallPhase::Complete => phase::green(),
        InstallPhase::Error => phase::red(),
        InstallPhase::Ready => phase::accent(),
        InstallPhase::Checking | InstallPhase::Downloading | InstallPhase::Installing => {
            phase::blue()
        }
        InstallPhase::Idle => phase::accent_dim(),
    }
}

fn health_label(health: &FolderHealth) -> &'static str {
    match health {
        FolderHealth::Ready => "Ready",
        FolderHealth::Empty => "Empty",
        FolderHealth::Missing => "Missing",
    }
}

fn progress_for(elapsed: Duration, total: Duration) -> f32 {
    (elapsed.as_secs_f32() / total.as_secs_f32()).min(1.0)
}

fn install_id() -> String {
    let fallback = || uuid::Uuid::new_v4().to_string();
    let Some(mut dir) = dirs::config_dir() else {
        return fallback();
    };

    dir.push("Phase");
    dir.push("Phase Animator Installer");
    if std::fs::create_dir_all(&dir).is_err() {
        return fallback();
    }

    // Stable local handle so reconnects and installs can be matched to this app.
    let path = dir.join("install-id");
    if let Ok(existing) = std::fs::read_to_string(&path) {
        let id = existing.trim();
        if uuid::Uuid::parse_str(id).is_ok() {
            return id.to_owned();
        }
    }

    let id = fallback();
    let _ = std::fs::write(path, &id);
    id
}

fn account_cache_path() -> Option<PathBuf> {
    let mut dir = dirs::config_dir()?;
    dir.push("Phase");
    dir.push("Phase Animator Installer");
    if std::fs::create_dir_all(&dir).is_err() {
        return None;
    }
    Some(dir.join("account-cache.json"))
}

fn load_account_cache() -> Option<AccountCache> {
    let path = account_cache_path()?;
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

fn save_account_cache(cache: &AccountCache) {
    let Some(path) = account_cache_path() else {
        return;
    };
    if cache.plugin_token.is_none()
        && cache.linked_user.is_none()
        && cache.roblox_user_id.trim().is_empty()
        && cache.activation.is_none()
        && cache.selected_theme.is_none()
    {
        let _ = std::fs::remove_file(path);
        return;
    }
    if let Ok(text) = serde_json::to_string_pretty(cache) {
        let _ = std::fs::write(path, text);
    }
}

fn sha256_file(path: &std::path::Path) -> Result<String, String> {
    let mut file = std::fs::File::open(path)
        .map_err(|error| format!("Could not read installed plugin: {error}"))?;
    let mut hash = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = std::io::Read::read(&mut file, &mut buffer)
            .map_err(|error| format!("Could not read installed plugin: {error}"))?;
        if read == 0 {
            break;
        }
        hash.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

fn show_update_notification(version: &str) {
    let title = "Phase Animator";
    let body = format!("Update available: {version}");
    show_system_notification(title, &body);
}

fn show_system_notification(title: &str, body: &str) {
    let _ = notify_rust::Notification::new()
        .appname("Phase Animator")
        .summary(title)
        .body(body)
        .show();
}

fn display_linked_user(user: &verification::LinkedUser) -> String {
    user.display_name
        .as_deref()
        .filter(|name| !name.trim().is_empty())
        .unwrap_or(&user.username)
        .to_owned()
}

fn normalize_path(path: &std::path::Path) -> String {
    path.to_string_lossy().replace('\\', "/").to_lowercase()
}

fn compact_path(path: &std::path::Path, max_chars: usize) -> String {
    let text = path.to_string_lossy().to_string();
    if text.chars().count() <= max_chars {
        return text;
    }

    let tail: String = text
        .chars()
        .rev()
        .take(max_chars.saturating_sub(3))
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    format!("...{tail}")
}

fn initials(text: &str) -> String {
    let mut letters = text
        .split(|ch: char| !ch.is_alphanumeric())
        .filter(|part| !part.is_empty())
        .filter_map(|part| part.chars().next())
        .take(2)
        .collect::<String>();
    if letters.is_empty() {
        letters = "?".to_owned();
    }
    letters.to_uppercase()
}

fn human_size(bytes: u64) -> String {
    let mb = bytes as f64 / 1024.0 / 1024.0;
    if mb >= 1.0 {
        format!("{mb:.1} MB")
    } else {
        format!("{:.0} KB", bytes as f64 / 1024.0)
    }
}

fn parse_theme_background_image_id(theme_code: &str) -> Option<String> {
    theme_code
        .split('|')
        .find_map(|part| part.strip_prefix('i'))
        .map(str::trim)
        .filter(|value| value.chars().all(|ch| ch.is_ascii_digit()) && !value.is_empty())
        .map(|value| value.to_owned())
}

fn display_theme_owner(owner: &verification::PhaseThemeOwner) -> String {
    owner
        .display_name
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            if owner.username.trim().is_empty() {
                None
            } else {
                Some(owner.username.as_str())
            }
        })
        .unwrap_or("Phase creator")
        .to_owned()
}

fn theme_matches_search(asset: &verification::PhaseThemeAsset, search: &str) -> bool {
    let search = search.trim().to_ascii_lowercase();
    if search.is_empty() {
        return true;
    }

    let owner = asset
        .owner
        .as_ref()
        .map(display_theme_owner)
        .unwrap_or_default();
    let tags = asset.tags.join(" ");
    let haystack =
        format!("{} {} {} {}", asset.title, asset.description, owner, tags).to_ascii_lowercase();
    haystack.contains(&search)
}

fn theme_background_layout(
    rect: Rect,
    texture_size: Vec2,
    mode: ThemeBackgroundMode,
) -> (Rect, Rect) {
    let full_uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
    let image_w = texture_size.x.max(1.0);
    let image_h = texture_size.y.max(1.0);
    let image_aspect = image_w / image_h;
    let rect_aspect = rect.width().max(1.0) / rect.height().max(1.0);

    match mode {
        ThemeBackgroundMode::Stretch => (rect, full_uv),
        ThemeBackgroundMode::Fit => {
            let size = if image_aspect > rect_aspect {
                Vec2::new(rect.width(), rect.width() / image_aspect)
            } else {
                Vec2::new(rect.height() * image_aspect, rect.height())
            };
            (Rect::from_center_size(rect.center(), size), full_uv)
        }
        ThemeBackgroundMode::Crop => {
            let uv = if image_aspect > rect_aspect {
                let visible_width = rect_aspect / image_aspect;
                let inset = (1.0 - visible_width) * 0.5;
                Rect::from_min_max(Pos2::new(inset, 0.0), Pos2::new(1.0 - inset, 1.0))
            } else {
                let visible_height = image_aspect / rect_aspect;
                let inset = (1.0 - visible_height) * 0.5;
                Rect::from_min_max(Pos2::new(0.0, inset), Pos2::new(1.0, 1.0 - inset))
            };
            (rect, uv)
        }
    }
}

fn primary_button(ui: &mut Ui, icon: MiniIcon, text: &str, size: Vec2) -> egui::Response {
    let (id, button_rect) = ui.allocate_space(size);
    let response = ui.interact(button_rect, id, Sense::click());

    let hovered = response.hovered();
    let active = response.clicked();

    let painter = ui.painter();

    let mut bg_color = phase::accent();
    if hovered {
        bg_color = phase::accent_hover();
    }
    if active {
        bg_color = phase::accent_dim();
    }

    painter.rect_filled(button_rect, Rounding::same(6.0), bg_color);
    painter.rect_stroke(
        button_rect,
        Rounding::same(6.0),
        Stroke::new(1.0, phase::text_on_accent()),
    );

    let icon_rect = Rect::from_center_size(
        Pos2::new(button_rect.center().x - 82.0, button_rect.center().y),
        Vec2::splat(19.0),
    );
    draw_icon_at(painter, icon_rect, icon, phase::text_on_accent());

    painter.text(
        Pos2::new(button_rect.center().x + 12.0, button_rect.center().y),
        Align2::CENTER_CENTER,
        text,
        FontId::proportional(16.0),
        phase::text_on_accent(),
    );

    response
}

fn secondary_button(ui: &mut Ui, icon: MiniIcon, text: &str, size: Vec2) -> egui::Response {
    let (id, button_rect) = ui.allocate_space(size);
    let response = ui.interact(button_rect, id, Sense::click());

    let hovered = response.hovered();
    let active = response.clicked();

    let painter = ui.painter();

    let stroke_color = if hovered {
        phase::accent()
    } else {
        phase::line()
    };

    let bg_color = if active {
        phase::surface_active()
    } else if hovered {
        phase::surface_hover()
    } else {
        phase::input()
    };

    painter.rect_filled(button_rect, Rounding::same(6.0), bg_color);
    painter.rect_stroke(
        button_rect,
        Rounding::same(6.0),
        Stroke::new(1.0, stroke_color),
    );

    let text_color = if hovered {
        phase::text()
    } else {
        phase::text_secondary()
    };
    let icon_rect = Rect::from_center_size(
        Pos2::new(button_rect.left() + 22.0, button_rect.center().y),
        Vec2::splat(15.0),
    );
    draw_icon_at(painter, icon_rect, icon, text_color);

    painter.text(
        Pos2::new(button_rect.center().x + 10.0, button_rect.center().y),
        Align2::CENTER_CENTER,
        text,
        FontId::proportional(14.0),
        text_color,
    );

    response
}

fn status_action(ui: &mut Ui, icon: MiniIcon, text: &str, size: Vec2) {
    let (id, button_rect) = ui.allocate_space(size);
    let _ = ui.interact(button_rect, id, Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(button_rect, Rounding::same(6.0), phase::surface_active());
    painter.rect_stroke(
        button_rect,
        Rounding::same(6.0),
        Stroke::new(1.0, phase::line()),
    );
    let icon_rect = Rect::from_center_size(
        Pos2::new(button_rect.left() + 22.0, button_rect.center().y),
        Vec2::splat(15.0),
    );
    draw_icon_at(painter, icon_rect, icon, phase::green());
    painter.text(
        Pos2::new(button_rect.center().x + 10.0, button_rect.center().y),
        Align2::CENTER_CENTER,
        text,
        FontId::proportional(14.0),
        phase::text_secondary(),
    );
}

mod phase {
    use eframe::egui::Color32;
    use std::sync::{OnceLock, RwLock};

    #[derive(Clone, Copy)]
    pub struct Palette {
        background: Color32,
        surface: Color32,
        surface_hover: Color32,
        surface_active: Color32,
        input: Color32,
        line: Color32,
        accent: Color32,
        accent_hover: Color32,
        accent_dim: Color32,
        blue: Color32,
        green: Color32,
        red: Color32,
        warning: Color32,
        text: Color32,
        text_secondary: Color32,
        text_muted: Color32,
        text_on_accent: Color32,
    }

    static PALETTE: OnceLock<RwLock<Palette>> = OnceLock::new();

    pub fn reset_palette() {
        set_palette(default_palette());
    }

    pub fn set_palette(palette: Palette) {
        let lock = PALETTE.get_or_init(|| RwLock::new(default_palette()));
        if let Ok(mut current) = lock.write() {
            *current = palette;
        }
    }

    pub fn palette_from_theme_code(code: &str) -> Option<Palette> {
        let colors = code
            .split('|')
            .nth(2)?
            .split('.')
            .filter_map(hex_color)
            .collect::<Vec<_>>();
        if colors.len() < 25 {
            return None;
        }

        Some(Palette {
            background: colors[0],
            surface: colors[5],
            surface_hover: colors[6],
            surface_active: colors[7],
            input: colors[23],
            line: colors[20],
            accent: colors[8],
            accent_hover: colors[9],
            accent_dim: colors[11],
            blue: colors[12],
            green: colors[13],
            red: colors[14],
            warning: colors[15],
            text: colors[16],
            text_secondary: colors[17],
            text_muted: colors[18],
            text_on_accent: colors[19],
        })
    }

    pub fn background() -> Color32 {
        current().background
    }
    pub fn surface() -> Color32 {
        current().surface
    }
    pub fn surface_hover() -> Color32 {
        current().surface_hover
    }
    pub fn surface_active() -> Color32 {
        current().surface_active
    }
    pub fn input() -> Color32 {
        current().input
    }
    pub fn line() -> Color32 {
        current().line
    }
    pub fn accent() -> Color32 {
        current().accent
    }
    pub fn accent_hover() -> Color32 {
        current().accent_hover
    }
    pub fn accent_dim() -> Color32 {
        current().accent_dim
    }
    pub fn blue() -> Color32 {
        current().blue
    }
    pub fn green() -> Color32 {
        current().green
    }
    pub fn red() -> Color32 {
        current().red
    }
    pub fn warning() -> Color32 {
        current().warning
    }
    pub fn text() -> Color32 {
        current().text
    }
    pub fn text_secondary() -> Color32 {
        current().text_secondary
    }
    pub fn text_muted() -> Color32 {
        current().text_muted
    }
    pub fn text_on_accent() -> Color32 {
        current().text_on_accent
    }

    fn current() -> Palette {
        let lock = PALETTE.get_or_init(|| RwLock::new(default_palette()));
        lock.read()
            .map(|palette| *palette)
            .unwrap_or_else(|_| default_palette())
    }

    fn default_palette() -> Palette {
        Palette {
            background: Color32::from_rgb(21, 18, 37),
            surface: Color32::from_rgb(38, 33, 63),
            surface_hover: Color32::from_rgb(58, 52, 90),
            surface_active: Color32::from_rgb(67, 59, 99),
            input: Color32::from_rgb(42, 36, 66),
            line: Color32::from_rgb(58, 52, 90),
            accent: Color32::from_rgb(216, 184, 245),
            accent_hover: Color32::from_rgb(234, 216, 255),
            accent_dim: Color32::from_rgb(122, 98, 159),
            blue: Color32::from_rgb(158, 219, 255),
            green: Color32::from_rgb(75, 198, 122),
            red: Color32::from_rgb(224, 78, 78),
            warning: Color32::from_rgb(228, 169, 64),
            text: Color32::from_rgb(243, 238, 255),
            text_secondary: Color32::from_rgb(197, 190, 221),
            text_muted: Color32::from_rgb(159, 151, 188),
            text_on_accent: Color32::from_rgb(74, 64, 102),
        }
    }

    fn hex_color(value: &str) -> Option<Color32> {
        let value = value.trim();
        if value.len() != 6 {
            return None;
        }
        let rgb = u32::from_str_radix(value, 16).ok()?;
        Some(Color32::from_rgb(
            ((rgb >> 16) & 0xff) as u8,
            ((rgb >> 8) & 0xff) as u8,
            (rgb & 0xff) as u8,
        ))
    }
}
