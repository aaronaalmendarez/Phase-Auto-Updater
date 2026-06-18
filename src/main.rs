#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod detector;
mod diagnostics;
mod motion;
mod verification;
mod video_reference;

use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;
#[cfg(target_os = "windows")]
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::SystemTime;
use std::time::{Duration, Instant};

use detector::{
    FolderHealth, PluginFile, PluginFolderCandidate, best_candidate, detect_plugin_folders,
    inspect_candidate,
};
use eframe::egui::{
    self, Align, Align2, Button, Color32, ColorImage, Context, FontData, FontFamily, FontId,
    IconData, Margin, Pos2, Rect, RichText, Rounding, Sense, Stroke, TextFormat, TextureHandle,
    TextureOptions, Ui, Vec2, WidgetText,
};
#[cfg(any(target_os = "windows", target_os = "macos"))]
use tray_icon::{
    Icon as TrayIconImage, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent,
};

const APP_NAME: &str = "Phase Companion";
const CURRENT_BUILD_ID: &str = "phase-2026-06-05-rustls-v0-19-8";
const PHOSPHOR_FONT: &str = "phosphor-icons";
const APP_WIDTH: f32 = 450.0;
// Responsive content column. The UI lays everything out in a single centered
// column whose width tracks the window but is clamped to this range, so wide
// windows get balanced margins instead of stretched rows and narrow windows
// shrink to fit instead of clipping.
const MAX_CONTENT_WIDTH: f32 = 560.0;
const MIN_CONTENT_WIDTH: f32 = 296.0;
const CARD_H_MARGIN: f32 = 14.0;
const THEME_ROW_MARGIN: f32 = 12.0;
const TRAY_PANEL_WIDTH: f32 = 302.0;
const TRAY_PANEL_HEIGHT: f32 = 286.0;
const TRAY_VIEWPORT_KEY: &str = "phase-tray-controls";
const DIAGNOSTICS_VIEWPORT_KEY: &str = "phase-connection-diagnostics";
const PARKED_WINDOW_POS: f32 = -32_000.0;
const PARKED_WINDOW_SIZE: f32 = 1.0;
#[cfg(target_os = "windows")]
static MAIN_HWND: AtomicIsize = AtomicIsize::new(0);

thread_local! {
    // The centered content-column width for the current frame. Set once per
    // frame in `update` and read by the responsive layout helpers below, so the
    // whole UI flexes with the window instead of relying on baked-in widths.
    // eframe runs the UI on a single thread, so a thread-local Cell is enough.
    static CONTENT_W: std::cell::Cell<f32> = std::cell::Cell::new(410.0);
}

fn set_content_width(w: f32) {
    CONTENT_W.with(|c| c.set(w));
}

/// Width of the centered content column for this frame.
fn content_w() -> f32 {
    CONTENT_W.with(|c| c.get())
}

/// Inner width inside a standard card (column width minus the card's horizontal
/// margins). Every card shares the same margin, so this is correct regardless
/// of nesting or draw order.
fn card_inner() -> f32 {
    (content_w() - CARD_H_MARGIN * 2.0).max(1.0)
}

fn main() -> eframe::Result<()> {
    if std::env::args().any(|arg| arg == "--smoke-test") {
        if run_smoke_test().is_err() {
            std::process::exit(1);
        }
        return Ok(());
    }
    if let Some(path) = popup_arg_path() {
        video_reference::install_popup_panic_logger();
        if let Err(error) = video_reference::run_popup_window(&path) {
            video_reference::append_popup_log(format!("popup failed before event loop: {error}"));
            eprintln!("Phase video popup failed: {error}");
        }
        return Ok(());
    }

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([APP_WIDTH, 620.0])
            .with_min_inner_size([320.0, 520.0])
            .with_transparent(true)
            .with_title(APP_NAME)
            .with_icon(load_window_icon()),
        persist_window: false,
        ..Default::default()
    };

    eframe::run_native(
        APP_NAME,
        native_options,
        Box::new(|cc| Box::new(PhaseInstallerApp::new(cc))),
    )
}

fn popup_arg_path() -> Option<PathBuf> {
    let mut args = std::env::args_os();
    while let Some(arg) = args.next() {
        if arg == "--video-popup" {
            return args.next().map(PathBuf::from);
        }
    }
    None
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

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum ViewTab {
    Install,
    Account,
    Folders,
    Video,
    Options,
}

impl ViewTab {
    fn index(self) -> usize {
        match self {
            ViewTab::Install => 0,
            ViewTab::Account => 1,
            ViewTab::Folders => 2,
            ViewTab::Video => 3,
            ViewTab::Options => 4,
        }
    }
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

struct ThemePreviewFetchResult {
    asset_id: String,
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

#[cfg(any(target_os = "windows", target_os = "macos"))]
struct TrayController {
    _icon: TrayIcon,
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
enum TraySignal {
    ShowWindow,
    ShowPanel { x: f32, y: f32 },
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
    theme_preview_tx: Sender<ThemePreviewFetchResult>,
    theme_preview_rx: Receiver<ThemePreviewFetchResult>,
    theme_preview_textures: HashMap<String, TextureHandle>,
    theme_preview_loading: HashSet<String>,
    candidates: Vec<PluginFolderCandidate>,
    selected_folder: Option<PathBuf>,
    release: Option<verification::VersionResponse>,
    local_release_current: bool,
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
    diagnostics_rx: Option<Receiver<diagnostics::DiagnosticReport>>,
    diagnostics_report: Option<diagnostics::DiagnosticReport>,
    diagnostics_open: bool,
    diagnostics_started_at: Option<Instant>,
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
    plugin_settings_reset_themes: bool,
    plugin_settings_reset_keybinds: bool,
    plugin_settings_inventory: PluginSettingsInventory,
    plugin_data_reset_confirm: bool,
    plugin_data_reset_status: Option<String>,
    video_bridge: video_reference::VideoReferenceBridge,
    video_bridge_config: video_reference::BridgeConfig,
    video_bridge_listening: bool,
    video_bridge_connected: bool,
    video_bridge_status: String,
    video_source: String,
    video_title: String,
    video_duration_seconds: String,
    video_fps: String,
    video_start_frame: String,
    video_offset_seconds: String,
    video_playback_rate: String,
    video_position_seconds: f64,
    video_position_input: String,
    video_sync_enabled: bool,
    video_playing: bool,
    video_play_last_tick: Option<Instant>,
    video_last_sync_sent: Option<Instant>,
    video_seq: u64,
    video_last_plugin_state: String,
    video_last_reference_status: String,
    phase: InstallPhase,
    active_tab: ViewTab,
    progress: f32,
    phase_started_at: Option<Instant>,
    activity: Vec<ActivityLine>,

    tab_indicator: motion::SpringValue,
    tab_page_motion: motion::PagerMotion<ViewTab>,
    milestone: u32,
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    tray: Option<TrayController>,
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    tray_rx: Receiver<TraySignal>,
    close_dialog_open: bool,
    allow_quit: bool,
    hidden_to_tray: bool,
    tray_notice_shown: bool,
    tray_panel_open: bool,
    tray_panel_pos: Pos2,
    main_window_pos: Option<Pos2>,
    tray_anim_nonce: u64,
    dialog_anim_nonce: u64,
}

impl PhaseInstallerApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        configure_style(&cc.egui_ctx);

        let candidates = detect_plugin_folders();
        let selected_folder = best_candidate(&candidates).map(|candidate| candidate.path);
        let (avatar_tx, avatar_rx) = mpsc::channel();
        let (theme_background_tx, theme_background_rx) = mpsc::channel();
        let (theme_preview_tx, theme_preview_rx) = mpsc::channel();
        let video_bridge_config = video_reference::BridgeConfig::default_local();
        let video_bridge =
            video_reference::VideoReferenceBridge::start(video_bridge_config.clone());
        #[cfg(any(target_os = "windows", target_os = "macos"))]
        let (tray_tx, tray_rx) = mpsc::channel();

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
            theme_preview_tx,
            theme_preview_rx,
            theme_preview_textures: HashMap::new(),
            theme_preview_loading: HashSet::new(),
            candidates,
            selected_folder,
            release: None,
            local_release_current: false,
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
            diagnostics_rx: None,
            diagnostics_report: None,
            diagnostics_open: false,
            diagnostics_started_at: None,
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
            plugin_settings_reset_themes: true,
            plugin_settings_reset_keybinds: true,
            plugin_settings_inventory: phase_plugin_settings_inventory(),
            plugin_data_reset_confirm: false,
            plugin_data_reset_status: None,
            video_bridge,
            video_bridge_config,
            video_bridge_listening: false,
            video_bridge_connected: false,
            video_bridge_status: "Starting video bridge.".to_owned(),
            video_source: String::new(),
            video_title: String::new(),
            video_duration_seconds: String::new(),
            video_fps: "60".to_owned(),
            video_start_frame: "0".to_owned(),
            video_offset_seconds: "0".to_owned(),
            video_playback_rate: "1".to_owned(),
            video_position_seconds: 0.0,
            video_position_input: "0".to_owned(),
            video_sync_enabled: false,
            video_playing: false,
            video_play_last_tick: None,
            video_last_sync_sent: None,
            video_seq: 0,
            video_last_plugin_state: "No Studio timeline state yet.".to_owned(),
            video_last_reference_status: "No reference sent.".to_owned(),
            phase: InstallPhase::Idle,
            active_tab: ViewTab::Install,
            progress: 0.0,
            phase_started_at: None,
            activity: Vec::new(),
            tab_indicator: motion::SpringValue::new(0.0),
            tab_page_motion: motion::PagerMotion::new(ViewTab::Install),
            milestone: 0,
            #[cfg(any(target_os = "windows", target_os = "macos"))]
            tray: None,
            #[cfg(any(target_os = "windows", target_os = "macos"))]
            tray_rx,
            close_dialog_open: false,
            allow_quit: false,
            hidden_to_tray: false,
            tray_notice_shown: false,
            tray_panel_open: false,
            tray_panel_pos: Pos2::new(64.0, 64.0),
            main_window_pos: None,
            tray_anim_nonce: 0,
            dialog_anim_nonce: 0,
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

        #[cfg(any(target_os = "windows", target_os = "macos"))]
        match TrayController::new(tray_tx, cc.egui_ctx.clone()) {
            Ok(tray) => app.tray = Some(tray),
            Err(error) => app.log(phase::warning(), format!("Tray unavailable: {error}")),
        }

        app.begin_version_check(Some(cc.egui_ctx.clone()));
        app.begin_update_stream(&cc.egui_ctx);
        app.begin_phase_account_refresh(&cc.egui_ctx);
        app.begin_app_update_check(&cc.egui_ctx);
        app.begin_theme_fetch(&cc.egui_ctx);

        app
    }

    fn select_tab(&mut self, tab: ViewTab) {
        if self.active_tab == tab {
            return;
        }
        let direction = tab.index() as f32 - self.active_tab.index() as f32;
        self.tab_page_motion.set_target(tab, direction);
        self.active_tab = tab;
    }

    fn refresh_local_release_status(&mut self) {
        self.local_release_current = self
            .release
            .as_ref()
            .is_some_and(|release| self.local_matches_latest(release));
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
        self.poll_connection_diagnostics(ctx);
        self.poll_avatar_fetches(ctx);
        self.ensure_avatar_fetches(ctx);
        self.poll_theme_background_fetches(ctx);
        self.ensure_theme_background_fetch(ctx);
        self.poll_theme_preview_fetches(ctx);
        self.poll_tray(ctx);
        self.poll_video_bridge(ctx);
        self.tick_video_playback(ctx);

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

    fn is_busy(&self) -> bool {
        matches!(
            self.phase,
            InstallPhase::Checking | InstallPhase::Downloading | InstallPhase::Installing
        )
    }

    #[cfg(any(target_os = "windows", target_os = "macos"))]
    fn poll_tray(&mut self, ctx: &Context) {
        let signals: Vec<TraySignal> = self.tray_rx.try_iter().collect();
        for signal in signals {
            match signal {
                TraySignal::ShowWindow => {
                    log_tray_debug("app received show window");
                    self.show_main_window(ctx);
                }
                TraySignal::ShowPanel { x, y } => {
                    log_tray_debug(format!("app received show panel at {x:.0},{y:.0}"));
                    self.show_tray_panel(ctx, x, y);
                }
            }
        }
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    fn poll_tray(&mut self, _ctx: &Context) {}

    fn tray_available(&self) -> bool {
        #[cfg(any(target_os = "windows", target_os = "macos"))]
        {
            self.tray.is_some()
        }
        #[cfg(not(any(target_os = "windows", target_os = "macos")))]
        {
            false
        }
    }

    fn show_main_window(&mut self, ctx: &Context) {
        self.hidden_to_tray = false;
        self.close_tray_popup(ctx);
        ctx.send_viewport_cmd_to(
            egui::ViewportId::ROOT,
            egui::ViewportCommand::MousePassthrough(false),
        );
        ctx.send_viewport_cmd_to(
            egui::ViewportId::ROOT,
            egui::ViewportCommand::Transparent(false),
        );
        ctx.send_viewport_cmd_to(
            egui::ViewportId::ROOT,
            egui::ViewportCommand::Decorations(true),
        );
        ctx.send_viewport_cmd_to(
            egui::ViewportId::ROOT,
            egui::ViewportCommand::Resizable(true),
        );
        ctx.send_viewport_cmd_to(
            egui::ViewportId::ROOT,
            egui::ViewportCommand::MinInnerSize(Vec2::new(320.0, 520.0)),
        );
        ctx.send_viewport_cmd_to(
            egui::ViewportId::ROOT,
            egui::ViewportCommand::InnerSize(Vec2::new(APP_WIDTH, 620.0)),
        );
        if let Some(pos) = self.main_window_pos {
            ctx.send_viewport_cmd_to(
                egui::ViewportId::ROOT,
                egui::ViewportCommand::OuterPosition(pos),
            );
        }
        ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd_to(
            egui::ViewportId::ROOT,
            egui::ViewportCommand::Minimized(false),
        );
        ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Focus);
        ctx.request_repaint();
    }

    #[cfg(any(target_os = "windows", target_os = "macos"))]
    fn show_tray_panel(&mut self, ctx: &Context, x: f32, y: f32) {
        if !self.tray_panel_open {
            self.tray_anim_nonce = self.tray_anim_nonce.wrapping_add(1);
        }
        self.tray_panel_open = true;
        self.close_dialog_open = false;

        let panel_x = (x - TRAY_PANEL_WIDTH + 14.0).max(8.0);
        let panel_y = (y - TRAY_PANEL_HEIGHT - 8.0).max(8.0);
        self.tray_panel_pos = Pos2::new(panel_x, panel_y);
        if self.hidden_to_tray {
            self.show_tray_panel_on_root(ctx);
        } else {
            ctx.send_viewport_cmd_to(tray_viewport_id(), egui::ViewportCommand::Focus);
        }
        ctx.request_repaint();
    }

    fn close_tray_popup(&mut self, ctx: &Context) {
        self.tray_panel_open = false;
        if self.hidden_to_tray {
            self.park_root_for_tray(ctx);
        } else {
            ctx.send_viewport_cmd_to(tray_viewport_id(), egui::ViewportCommand::Close);
        }
        ctx.request_repaint();
    }

    fn minimize_to_tray(&mut self, ctx: &Context) {
        self.remember_main_window_position(ctx);
        self.hidden_to_tray = true;
        self.close_dialog_open = false;
        self.close_tray_popup(ctx);
        if !self.tray_notice_shown {
            self.tray_notice_shown = true;
            show_system_notification(
                "Phase Animator",
                "Still running in the tray. Right-click for install actions.",
            );
        }
    }

    fn show_tray_panel_on_root(&self, ctx: &Context) {
        ctx.send_viewport_cmd_to(
            egui::ViewportId::ROOT,
            egui::ViewportCommand::MousePassthrough(false),
        );
        ctx.send_viewport_cmd_to(
            egui::ViewportId::ROOT,
            egui::ViewportCommand::Transparent(false),
        );
        ctx.send_viewport_cmd_to(
            egui::ViewportId::ROOT,
            egui::ViewportCommand::Decorations(false),
        );
        ctx.send_viewport_cmd_to(
            egui::ViewportId::ROOT,
            egui::ViewportCommand::Resizable(false),
        );
        ctx.send_viewport_cmd_to(
            egui::ViewportId::ROOT,
            egui::ViewportCommand::MinInnerSize(Vec2::new(TRAY_PANEL_WIDTH, 180.0)),
        );
        ctx.send_viewport_cmd_to(
            egui::ViewportId::ROOT,
            egui::ViewportCommand::InnerSize(Vec2::new(TRAY_PANEL_WIDTH, TRAY_PANEL_HEIGHT)),
        );
        ctx.send_viewport_cmd_to(
            egui::ViewportId::ROOT,
            egui::ViewportCommand::OuterPosition(self.tray_panel_pos),
        );
        ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Focus);
    }

    fn park_root_for_tray(&self, ctx: &Context) {
        ctx.send_viewport_cmd_to(
            egui::ViewportId::ROOT,
            egui::ViewportCommand::Transparent(true),
        );
        ctx.send_viewport_cmd_to(
            egui::ViewportId::ROOT,
            egui::ViewportCommand::Decorations(false),
        );
        ctx.send_viewport_cmd_to(
            egui::ViewportId::ROOT,
            egui::ViewportCommand::Resizable(false),
        );
        ctx.send_viewport_cmd_to(
            egui::ViewportId::ROOT,
            egui::ViewportCommand::MousePassthrough(true),
        );
        ctx.send_viewport_cmd_to(
            egui::ViewportId::ROOT,
            egui::ViewportCommand::MinInnerSize(Vec2::splat(PARKED_WINDOW_SIZE)),
        );
        ctx.send_viewport_cmd_to(
            egui::ViewportId::ROOT,
            egui::ViewportCommand::InnerSize(Vec2::splat(PARKED_WINDOW_SIZE)),
        );
        ctx.send_viewport_cmd_to(
            egui::ViewportId::ROOT,
            egui::ViewportCommand::OuterPosition(Pos2::new(PARKED_WINDOW_POS, PARKED_WINDOW_POS)),
        );
        ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Visible(true));
    }

    fn remember_main_window_position(&mut self, ctx: &Context) {
        if self.hidden_to_tray {
            return;
        }

        let position = ctx.input(|input| input.viewport().outer_rect.map(|rect| rect.min));
        if let Some(position) = position {
            if position.x > -1_000.0 && position.y > -1_000.0 {
                self.main_window_pos = Some(position);
            }
        }
    }

    fn open_selected_folder(&mut self) {
        let Some(path) = self.selected_folder.clone() else {
            self.log(phase::warning(), "Choose an install location first.");
            return;
        };
        if let Err(error) = open::that(&path) {
            self.log(phase::warning(), format!("Could not open folder: {error}"));
        }
    }

    fn request_quit(&mut self, ctx: &Context) {
        self.allow_quit = true;
        self.close_dialog_open = false;
        self.close_tray_popup(ctx);
        self.cleanup_tray();
        ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Close);
    }

    fn cleanup_tray(&mut self) {
        #[cfg(any(target_os = "windows", target_os = "macos"))]
        if let Some(tray) = self.tray.take() {
            tray.hide();
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
            self.select_tab(ViewTab::Account);
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
                self.local_release_current = local_current;
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
                self.local_release_current = false;
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

    fn start_connection_diagnostics(&mut self, ctx: &Context) {
        self.diagnostics_open = true;
        if self.diagnostics_rx.is_some() {
            return;
        }

        let selected_folder = self.selected_folder.clone();
        let (tx, rx) = mpsc::channel();
        let repaint = ctx.clone();
        self.diagnostics_rx = Some(rx);
        self.diagnostics_started_at = Some(Instant::now());
        self.log(phase::blue(), "Running connection diagnostics.");

        std::thread::spawn(move || {
            let report = diagnostics::run(CURRENT_BUILD_ID, selected_folder);
            let _ = tx.send(report);
            repaint.request_repaint();
        });
    }

    fn poll_connection_diagnostics(&mut self, ctx: &Context) {
        let Some(report) = self
            .diagnostics_rx
            .as_ref()
            .and_then(|rx| rx.try_recv().ok())
        else {
            return;
        };

        self.diagnostics_rx = None;
        self.diagnostics_started_at = None;
        let status = report.overall_status();
        let summary = report.summary.clone();
        self.diagnostics_report = Some(report);
        match status {
            diagnostics::DiagnosticStatus::Good => self.log(phase::green(), summary),
            diagnostics::DiagnosticStatus::Warning => self.log(phase::warning(), summary),
            diagnostics::DiagnosticStatus::Problem => self.log(phase::red(), summary),
        }
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

    fn has_local_phase_install(&self) -> bool {
        let Some(folder) = self.selected_folder.as_ref() else {
            return false;
        };
        let plugin_files = self
            .selected_candidate()
            .map(|candidate| candidate.plugin_files.clone())
            .unwrap_or_default();
        choose_install_target(folder, &plugin_files).exists()
    }

    fn account_summary(&self) -> String {
        if let Some(user) = self.linked_user.as_ref().map(display_linked_user) {
            return user;
        }
        if let Some(name) = self
            .roblox_username
            .as_deref()
            .filter(|name| !name.trim().is_empty())
        {
            return name.to_owned();
        }
        if !self.roblox_user_id.trim().is_empty() {
            return format!("Roblox {}", self.roblox_user_id.trim());
        }
        "Not connected".to_owned()
    }

    fn release_summary(&self) -> String {
        self.release
            .as_ref()
            .map(|release| release.latest_version.clone())
            .unwrap_or_else(|| "Checking".to_owned())
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
                    let activation_mode = status
                        .activation_mode
                        .clone()
                        .unwrap_or_else(|| "licenseKey".to_owned());
                    let user_id = user_id_text
                        .parse::<u64>()
                        .ok()
                        .or_else(|| (activation_mode == "phaseAccount").then_some(0_u64));
                    if let Some(user_id) = user_id {
                        if !user_id_text.trim().is_empty() {
                            self.roblox_user_id = user_id_text;
                        }
                        self.activation_error = None;
                        self.activation = Some(verification::ActivationResponse {
                            ok: true,
                            active: true,
                            activation_mode,
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
                let session = me.plugin_session;
                self.linked_user = Some(me.user);
                if let Some(session) = session {
                    if let Some(user_id_text) = session
                        .roblox_user_id
                        .as_deref()
                        .filter(|value| !value.trim().is_empty())
                    {
                        self.roblox_user_id = user_id_text.to_owned();
                    }
                    if let Some(token) = session
                        .activation_token
                        .as_deref()
                        .filter(|value| !value.trim().is_empty())
                    {
                        let activation_mode = session
                            .activation_mode
                            .clone()
                            .unwrap_or_else(|| "licenseKey".to_owned());
                        let user_id = self
                            .roblox_user_id
                            .trim()
                            .parse::<u64>()
                            .ok()
                            .or_else(|| (activation_mode == "phaseAccount").then_some(0_u64));
                        if let Some(user_id) = user_id {
                            self.activation_error = None;
                            self.activation = Some(verification::ActivationResponse {
                                ok: true,
                                active: true,
                                activation_mode,
                                product: "Phase Animator".to_owned(),
                                user_id,
                                install_id: session.install_id.clone().unwrap_or_else(install_id),
                                asset_id: Some(verification::ROBLOX_PLUGIN_ASSET_ID),
                                token: token.to_owned(),
                                expires_at: 0,
                                licensee: session
                                    .licensee
                                    .clone()
                                    .unwrap_or_else(|| format!("Phase account {user_id}")),
                                message: session
                                    .message
                                    .clone()
                                    .unwrap_or_else(|| "Phase account access verified.".to_owned()),
                            });
                        }
                    }
                }
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
                self.log(
                    phase::warning(),
                    format!("{error}. Saved account connection kept."),
                );
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
                    "Phase Companion",
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

    fn reset_phase_plugin_data(&mut self) {
        let categories = self.selected_plugin_settings_categories();
        if categories.is_empty() {
            self.plugin_data_reset_status =
                Some("Choose at least one settings category.".to_owned());
            return;
        }

        match reset_phase_plugin_settings(&categories) {
            Ok(summary) => {
                self.plugin_data_reset_confirm = false;
                let message = if summary.removed_keys == 0 {
                    "No matching Phase Animator plugin settings were found.".to_owned()
                } else {
                    format!(
                        "Backed up and deleted {} Phase setting{} across {} Roblox settings file{}.",
                        summary.removed_keys,
                        plural(summary.removed_keys),
                        summary.files_changed,
                        plural(summary.files_changed)
                    )
                };
                self.plugin_data_reset_status = Some(message.clone());
                self.log(phase::green(), message);
                self.plugin_settings_inventory = phase_plugin_settings_inventory();
            }
            Err(error) => {
                self.plugin_data_reset_status = Some(error.clone());
                self.log(phase::red(), error);
            }
        }
    }

    fn selected_plugin_settings_categories(&self) -> Vec<PluginSettingsCategory> {
        let mut categories = Vec::new();
        if self.plugin_settings_reset_themes {
            categories.push(PluginSettingsCategory::Themes);
        }
        if self.plugin_settings_reset_keybinds {
            categories.push(PluginSettingsCategory::Keybinds);
        }
        categories
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

    fn ensure_theme_preview_fetch(&mut self, ctx: &Context, asset: &verification::PhaseThemeAsset) {
        let background_image = asset.theme_preview.background_image.trim();
        if background_image.is_empty()
            || self.theme_preview_textures.contains_key(&asset.id)
            || self.theme_preview_loading.contains(&asset.id)
        {
            return;
        }

        let asset_id = asset.id.clone();
        let key = background_image.to_owned();
        self.theme_preview_loading.insert(asset_id.clone());
        spawn_theme_preview_fetch(
            self.theme_preview_tx.clone(),
            asset_id,
            key.clone(),
            ctx.clone(),
            move || verification::fetch_roblox_asset_thumbnail_image(&key),
        );
    }

    fn poll_theme_preview_fetches(&mut self, ctx: &Context) {
        while let Ok(result) = self.theme_preview_rx.try_recv() {
            let Ok(image) = result.image else {
                continue;
            };
            self.theme_preview_loading.remove(&result.asset_id);
            let texture = ctx.load_texture(
                format!("theme-preview-{}-{}", result.asset_id, result.key),
                image,
                TextureOptions::LINEAR,
            );
            self.theme_preview_textures.insert(result.asset_id, texture);
            ctx.request_repaint();
        }
    }

    fn refresh_detection(&mut self) {
        let previous = self.selected_folder.clone();
        self.candidates = detect_plugin_folders();
        self.selected_folder = previous
            .filter(|path| path.exists())
            .or_else(|| best_candidate(&self.candidates).map(|candidate| candidate.path));
        self.refresh_local_release_status();
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
            self.refresh_local_release_status();
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

    fn poll_video_bridge(&mut self, ctx: &Context) {
        for event in self.video_bridge.poll() {
            match event {
                video_reference::BridgeEvent::Listening { url } => {
                    self.video_bridge_listening = true;
                    self.video_bridge_status = format!("Listening on {url}");
                    self.log(phase::green(), "Video reference bridge is listening.");
                }
                video_reference::BridgeEvent::ClientConnected => {
                    self.video_bridge_connected = true;
                    self.video_bridge_status = "Studio connected to video bridge.".to_owned();
                    self.log(phase::green(), "Studio connected to video bridge.");
                }
                video_reference::BridgeEvent::ClientDisconnected => {
                    self.video_bridge_connected = false;
                    self.video_bridge_status = "Studio disconnected from video bridge.".to_owned();
                    self.video_playing = false;
                    self.video_play_last_tick = None;
                    self.log(phase::warning(), "Studio disconnected from video bridge.");
                }
                video_reference::BridgeEvent::PacketReceived(packet) => {
                    self.handle_video_packet(packet);
                }
                video_reference::BridgeEvent::PacketSent { op } => {
                    self.video_bridge_status = format!("Sent {op} to Studio.");
                }
                video_reference::BridgeEvent::SendFailed { op, message } => {
                    self.video_bridge_status = format!("{op} failed: {message}");
                    self.log(phase::warning(), self.video_bridge_status.clone());
                }
                video_reference::BridgeEvent::Error(error) => {
                    self.video_bridge_status = error.clone();
                    self.log(phase::red(), error);
                }
                video_reference::BridgeEvent::Stopped => {
                    self.video_bridge_listening = false;
                    self.video_bridge_connected = false;
                    self.video_bridge_status = "Video bridge stopped.".to_owned();
                }
            }
            ctx.request_repaint();
        }
    }

    fn handle_video_packet(&mut self, packet: video_reference::VideoPacket) {
        let payload = video_reference::packet_payload(&packet);
        match packet.op.as_str() {
            "hello" => {
                self.video_last_plugin_state = "Studio hello received.".to_owned();
            }
            "ping" => {
                self.video_bridge_status = "Ping received from Studio.".to_owned();
            }
            "ack" | "hello.ok" => {
                self.video_last_reference_status = payload
                    .get("video_reference")
                    .and_then(reference_summary)
                    .unwrap_or_else(|| "Studio acknowledged video bridge packet.".to_owned());
                self.video_bridge_status = "Studio acknowledged video bridge packet.".to_owned();
            }
            "error" => {
                let message = payload
                    .get("message")
                    .and_then(|value| value.as_str())
                    .unwrap_or("Studio reported a video bridge error.");
                self.video_bridge_status = message.to_owned();
                self.log(phase::red(), message);
            }
            "reference.status" => {
                self.video_last_reference_status = payload
                    .get("video_reference")
                    .or_else(|| payload.get("reference"))
                    .and_then(reference_summary)
                    .or_else(|| {
                        payload
                            .get("status")
                            .or_else(|| payload.get("message"))
                            .and_then(|value| value.as_str())
                            .map(str::to_owned)
                    })
                    .unwrap_or_else(|| "Studio reference state received.".to_owned());
            }
            "sync.enabled" => {
                self.video_sync_enabled = payload
                    .get("enabled")
                    .or_else(|| payload.get("sync_enabled"))
                    .and_then(|value| value.as_bool())
                    .unwrap_or(self.video_sync_enabled);
                self.video_last_plugin_state = if self.video_sync_enabled {
                    "Studio video sync enabled.".to_owned()
                } else {
                    "Studio video sync disabled.".to_owned()
                };
            }
            "sync.timeline" | "sync.seek" | "sync.playback" => {
                self.apply_video_timeline_payload(packet.op.as_str(), payload);
            }
            _ => {}
        }
    }

    fn apply_video_timeline_payload(&mut self, op: &str, payload: &serde_json::Value) {
        let seconds = payload
            .get("seconds")
            .or_else(|| payload.get("video_seconds"))
            .or_else(|| payload.get("position_seconds"))
            .and_then(|value| value.as_f64());
        let frame = payload.get("frame").and_then(|value| value.as_i64());
        let fps = payload.get("fps").and_then(|value| value.as_f64());
        let playback_rate = payload
            .get("playback_rate")
            .or_else(|| payload.get("PlaybackRate"))
            .or_else(|| payload.get("rate"))
            .and_then(|value| value.as_f64());
        let playing = payload.get("playing").and_then(|value| value.as_bool());
        if let Some(sync_enabled) = payload
            .get("sync_enabled")
            .or_else(|| payload.get("enabled"))
            .and_then(|value| value.as_bool())
        {
            self.video_sync_enabled = sync_enabled;
        }
        if op == "sync.playback" {
            if let Some(playing) = playing {
                self.video_playing = playing;
                self.video_play_last_tick = playing.then(Instant::now);
            }
        }
        if let Some(seconds) = seconds {
            self.video_position_seconds = seconds.max(0.0);
            self.video_position_input = format_seconds(self.video_position_seconds);
        }
        if let Some(fps) = fps {
            self.video_fps = format_seconds(fps.max(1.0));
        }
        if let Some(playback_rate) = playback_rate {
            self.video_playback_rate = format_seconds(playback_rate.clamp(0.05, 8.0));
        }
        let frame_text = frame
            .map(|frame| format!("frame {frame}"))
            .unwrap_or_else(|| "frame unknown".to_owned());
        let seconds_text = seconds
            .map(|seconds| format!("{seconds:.3}s"))
            .unwrap_or_else(|| "seconds unknown".to_owned());
        self.video_last_plugin_state = format!("{op}: {frame_text}, {seconds_text}");
    }

    fn tick_video_playback(&mut self, ctx: &Context) {
        if !self.video_playing {
            self.video_play_last_tick = None;
            return;
        }

        let now = Instant::now();
        if let Some(previous) = self.video_play_last_tick {
            let rate = parse_f64_or(&self.video_playback_rate, 1.0).clamp(0.05, 8.0);
            self.video_position_seconds += previous.elapsed().as_secs_f64() * rate;
            self.video_position_input = format_seconds(self.video_position_seconds);
        }
        self.video_play_last_tick = Some(now);

        let should_send = self
            .video_last_sync_sent
            .is_none_or(|sent| sent.elapsed() >= Duration::from_millis(100));
        if should_send {
            self.send_video_timeline("sync.timeline");
            self.video_last_sync_sent = Some(now);
        }
        ctx.request_repaint_after(Duration::from_millis(33));
    }

    fn restart_video_bridge(&mut self) {
        self.video_bridge.stop();
        std::thread::sleep(Duration::from_millis(80));
        self.video_bridge =
            video_reference::VideoReferenceBridge::start(self.video_bridge_config.clone());
        self.video_bridge_listening = false;
        self.video_bridge_connected = false;
        self.video_bridge_status = "Restarting video bridge.".to_owned();
        self.log(phase::blue(), "Restarting video reference bridge.");
    }

    fn pick_video_file(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Video", &["mp4", "mov", "m4v", "webm"])
            .pick_file()
        else {
            return;
        };
        self.video_source = path.to_string_lossy().to_string();
        if self.video_title.trim().is_empty() {
            self.video_title = video_reference::default_title_for(&self.video_source);
        }
        self.video_last_reference_status = "Local video selected.".to_owned();
    }

    fn send_video_reference(&mut self) {
        let Ok(draft) = self.video_reference_draft() else {
            self.video_bridge_status = "Choose a YouTube URL or local MP4 first.".to_owned();
            self.log(phase::warning(), self.video_bridge_status.clone());
            return;
        };

        self.video_reference_source_to_title();
        self.video_bridge
            .send("reference.set", draft.payload(), None);
        self.video_last_reference_status = format!("Queued reference: {}", draft.title);
    }

    fn video_reference_draft(&self) -> Result<video_reference::ReferenceDraft, String> {
        let source = self.video_source.trim().to_owned();
        if source.is_empty() {
            return Err("Choose a YouTube URL or local MP4 first.".to_owned());
        }

        let title = self.video_title.trim();
        Ok(video_reference::ReferenceDraft {
            source_kind: video_reference::source_kind_for(&source),
            source: source.clone(),
            title: if title.is_empty() {
                video_reference::default_title_for(&source)
            } else {
                title.to_owned()
            },
            duration_seconds: parse_f64_or(&self.video_duration_seconds, 0.0),
            fps: parse_f64_or(&self.video_fps, 60.0),
            start_frame: parse_i64_or(&self.video_start_frame, 0),
            offset_seconds: parse_f64_or(&self.video_offset_seconds, 0.0),
            playback_rate: parse_f64_or(&self.video_playback_rate, 1.0),
        })
    }

    fn open_video_popup(&mut self) {
        let draft = match self.video_reference_draft() {
            Ok(draft) => draft,
            Err(error) => {
                self.video_bridge_status = error.clone();
                self.log(phase::warning(), error);
                return;
            }
        };
        match video_reference::open_reference_popup(&draft) {
            Ok(_) => {
                self.video_bridge
                    .send("reference.set", draft.payload(), None);
                self.video_last_reference_status = format!("Opened popup: {}", draft.title);
                self.video_bridge_status = "Video popup opened.".to_owned();
            }
            Err(error) => {
                self.video_bridge_status = error.clone();
                self.log(phase::red(), error);
            }
        }
    }

    fn video_reference_source_to_title(&mut self) {
        if self.video_title.trim().is_empty() && !self.video_source.trim().is_empty() {
            self.video_title = video_reference::default_title_for(&self.video_source);
        }
    }

    fn clear_video_reference(&mut self) {
        self.video_bridge.send("reference.clear", json!({}), None);
        self.video_last_reference_status = "Clear request sent.".to_owned();
    }

    fn send_video_sync_enabled(&mut self) {
        self.video_bridge.send(
            "sync.enabled",
            json!({
                "enabled": self.video_sync_enabled,
            }),
            None,
        );
    }

    fn send_video_ping(&mut self) {
        self.video_bridge.send(
            "ping",
            json!({
                "side": "phase-rust-companion",
            }),
            None,
        );
    }

    fn send_video_timeline(&mut self, op: &str) {
        self.video_seq = self.video_seq.saturating_add(1);
        let fps = parse_f64_or(&self.video_fps, 60.0).max(1.0);
        let start_frame = parse_i64_or(&self.video_start_frame, 0).max(0);
        let offset = parse_f64_or(&self.video_offset_seconds, 0.0);
        let playback_rate = parse_f64_or(&self.video_playback_rate, 1.0).clamp(0.05, 8.0);
        let frame = start_frame + ((self.video_position_seconds - offset) * fps).round() as i64;
        self.video_bridge.send(
            op,
            json!({
                "seq": self.video_seq,
                "frame": frame.max(0),
                "seconds": self.video_position_seconds,
                "fps": fps,
                "playing": self.video_playing,
                "playback_rate": playback_rate,
            }),
            None,
        );
    }

    fn seek_video_sync(&mut self) {
        self.video_position_seconds = parse_f64_or(&self.video_position_input, 0.0).max(0.0);
        self.video_position_input = format_seconds(self.video_position_seconds);
        self.send_video_timeline("sync.seek");
    }

    fn set_video_playing(&mut self, playing: bool) {
        self.video_position_seconds =
            parse_f64_or(&self.video_position_input, self.video_position_seconds).max(0.0);
        self.video_position_input = format_seconds(self.video_position_seconds);
        self.video_playing = playing;
        self.video_play_last_tick = playing.then(Instant::now);
        self.send_video_timeline("sync.playback");
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
    let file_name = if update.asset_name.trim().is_empty() {
        format!(
            "PhaseAutoUpdater-{}.msi",
            safe_file_fragment(&update.version)
        )
    } else {
        safe_file_fragment(&update.asset_name)
    };
    path.push(file_name);
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
        "Refreshing install access.",
        0.12,
    );

    let plan = verification::VerificationPlan::new(CURRENT_BUILD_ID);
    let activation = refresh_activation_for_download(&plan, &activation, &license_key);
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

fn refresh_activation_for_download(
    plan: &verification::VerificationPlan,
    activation: &verification::ActivationResponse,
    license_key: &str,
) -> verification::ActivationResponse {
    let request = match activation.activation_mode.as_str() {
        "robloxPurchase" => Some(verification::ActivationRequest {
            activation_mode: "robloxPurchase".to_owned(),
            license_key: None,
            user_id: activation.user_id,
            install_id: install_id(),
            asset_id: activation
                .asset_id
                .or(Some(verification::ROBLOX_PLUGIN_ASSET_ID)),
        }),
        "licenseKey" if !license_key.trim().is_empty() => Some(verification::ActivationRequest {
            activation_mode: "licenseKey".to_owned(),
            license_key: Some(license_key.trim().to_owned()),
            user_id: activation.user_id,
            install_id: install_id(),
            asset_id: None,
        }),
        _ => None,
    };

    request
        .as_ref()
        .and_then(|request| verification::activate_install(plan, request).ok())
        .unwrap_or_else(|| activation.clone())
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

#[cfg(any(target_os = "windows", target_os = "macos"))]
impl TrayController {
    fn new(tx: Sender<TraySignal>, ctx: Context) -> Result<Self, String> {
        let icon = load_tray_icon().ok_or_else(|| "Could not load tray icon.".to_owned())?;
        let icon = TrayIconBuilder::new()
            .with_tooltip("Phase Animator Installer")
            .with_menu_on_left_click(false)
            .with_menu_on_right_click(false)
            .with_icon(icon)
            .build()
            .map_err(|error| format!("Could not create tray icon: {error}"))?;
        log_tray_debug("tray icon created");

        TrayIconEvent::set_event_handler(Some(move |event| {
            log_tray_debug(format!("event {event:?}"));
            let signal = match event {
                TrayIconEvent::Click {
                    button: MouseButton::Right,
                    button_state: MouseButtonState::Down,
                    position,
                    ..
                } => {
                    log_tray_debug("right down");
                    Some(TraySignal::ShowPanel {
                        x: position.x as f32,
                        y: position.y as f32,
                    })
                }
                TrayIconEvent::Click {
                    button: MouseButton::Right,
                    button_state: MouseButtonState::Up,
                    position,
                    ..
                } => {
                    log_tray_debug("right up");
                    Some(TraySignal::ShowPanel {
                        x: position.x as f32,
                        y: position.y as f32,
                    })
                }
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    position,
                    ..
                } => {
                    log_tray_debug("left up");
                    Some(TraySignal::ShowPanel {
                        x: position.x as f32,
                        y: position.y as f32,
                    })
                }
                TrayIconEvent::DoubleClick {
                    button: MouseButton::Left,
                    ..
                } => {
                    log_tray_debug("left double");
                    reveal_window_for_tray_signal();
                    Some(TraySignal::ShowWindow)
                }
                _ => None,
            };
            if let Some(signal) = signal {
                let _ = tx.send(signal);
                ctx.request_repaint();
            }
        }));

        Ok(Self { _icon: icon })
    }

    fn hide(&self) {
        let _ = self._icon.set_visible(false);
        log_tray_debug("tray icon hidden");
    }
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
impl Drop for TrayController {
    fn drop(&mut self) {
        self.hide();
    }
}

impl eframe::App for PhaseInstallerApp {
    fn update(&mut self, ctx: &Context, frame: &mut eframe::Frame) {
        apply_windows_title_bar(frame);
        self.remember_main_window_position(ctx);
        self.handle_close_request(ctx);
        self.tick(ctx);

        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(if self.hidden_to_tray {
                if self.tray_panel_open {
                    phase::background()
                } else {
                    Color32::TRANSPARENT
                }
            } else {
                phase::background()
            }))
            .show(ctx, |ui| {
                if self.hidden_to_tray {
                    if self.tray_panel_open {
                        self.tray_panel(ui);
                    } else {
                        ui.allocate_space(ui.available_size());
                    }
                    return;
                }

                self.paint_theme_background(ui);
                // Responsive column: track the window width, reserve a small
                // scrollbar gutter, then clamp to a comfortable reading range so
                // the layout never sprawls on wide windows or clips on narrow.
                let column =
                    (ui.available_width() - 12.0).clamp(MIN_CONTENT_WIDTH, MAX_CONTENT_WIDTH);
                set_content_width(column);
                ui.vertical_centered(|ui| {
                    ui.set_width(content_w() + 12.0);
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
                                ui.set_width(content_w());
                                self.current_tab(ui);
                                ui.add_space(8.0);
                                self.activity_block(ui);
                            });
                        });
                });
            });
        if !self.hidden_to_tray {
            self.draw_close_dialog(ctx);
            self.show_tray_popup_viewport(ctx);
            self.show_diagnostics_viewport(ctx);
        }

        // Keep a low-frequency idle tick so async receivers are polled even if
        // the user is not moving the mouse. High-frequency repainting is
        // requested locally by active animations/progress/video playback.
        let repaint_after = if self.hidden_to_tray && !self.tray_panel_open {
            Duration::from_millis(500)
        } else {
            Duration::from_millis(250)
        };
        ctx.request_repaint_after(repaint_after);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.video_bridge.stop();
        self.cleanup_tray();
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        Color32::TRANSPARENT.to_normalized_gamma_f32()
    }
}

impl PhaseInstallerApp {
    fn show_diagnostics_viewport(&mut self, ctx: &Context) {
        if !self.diagnostics_open {
            return;
        }

        let builder = egui::ViewportBuilder::default()
            .with_title("Phase Connection Diagnostics")
            .with_inner_size(Vec2::new(520.0, 540.0))
            .with_min_inner_size(Vec2::new(420.0, 420.0))
            .with_transparent(false)
            .with_resizable(true)
            .with_taskbar(true)
            .with_visible(true);

        ctx.show_viewport_immediate(diagnostics_viewport_id(), builder, |diag_ctx, _class| {
            if diag_ctx.input(|input| input.viewport().close_requested()) {
                self.diagnostics_open = false;
                return;
            }

            if diag_ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
                self.diagnostics_open = false;
                return;
            }

            egui::CentralPanel::default()
                .frame(egui::Frame::none().fill(phase::background()))
                .show(diag_ctx, |ui| {
                    self.diagnostics_panel(ui);
                });
        });
    }

    fn diagnostics_panel(&mut self, ui: &mut Ui) {
        ui.add_space(14.0);
        ui.vertical_centered(|ui| {
            ui.set_width((ui.available_width() - 28.0).clamp(360.0, 472.0));
            ui.horizontal(|ui| {
                if let Some(logo) = &self.logo {
                    let image = egui::Image::new(logo).fit_to_exact_size(Vec2::splat(38.0));
                    ui.add(image);
                }
                ui.add_space(8.0);
                ui.vertical(|ui| {
                    ui.label(
                        RichText::new("Connection Diagnostics")
                            .font(FontId::proportional(18.0))
                            .strong()
                            .color(phase::text()),
                    );
                    ui.label(
                        RichText::new("Checks Phase servers and local install access.")
                            .font(FontId::proportional(11.5))
                            .color(phase::text_secondary()),
                    );
                });
            });

            ui.add_space(14.0);
            let running = self.diagnostics_rx.is_some();
            let status = self
                .diagnostics_report
                .as_ref()
                .map(diagnostics::DiagnosticReport::overall_status);
            let summary = if running {
                self.diagnostics_started_at
                    .map(|started| format!("Checking... {}s", started.elapsed().as_secs()))
                    .unwrap_or_else(|| "Checking...".to_owned())
            } else {
                self.diagnostics_report
                    .as_ref()
                    .map(|report| report.summary.clone())
                    .unwrap_or_else(|| "Run a check to see what is blocking connection.".to_owned())
            };
            let summary_color = match status {
                Some(diagnostics::DiagnosticStatus::Good) => phase::green(),
                Some(diagnostics::DiagnosticStatus::Warning) => phase::warning(),
                Some(diagnostics::DiagnosticStatus::Problem) => phase::red(),
                None => phase::blue(),
            };

            egui::Frame::none()
                .fill(phase::surface())
                .stroke(Stroke::new(1.0, phase::line()))
                .rounding(Rounding::same(10.0))
                .inner_margin(Margin::symmetric(14.0, 12.0))
                .show(ui, |ui| {
                    let width = (ui.available_width() - 8.0).max(300.0);
                    ui.set_width(width);
                    ui.horizontal(|ui| {
                        status_pill(
                            ui,
                            status
                                .map(diagnostics::DiagnosticStatus::label)
                                .unwrap_or("Ready"),
                            summary_color,
                        );
                        ui.add_space(8.0);
                        scrolling_label(
                            ui,
                            &summary,
                            width - 92.0,
                            FontId::proportional(13.0),
                            phase::text_secondary(),
                        );
                    });
                });

            ui.add_space(10.0);
            ui.horizontal(|ui| {
                let width = (ui.available_width() - 18.0) / 3.0;
                ui.add_enabled_ui(!running, |ui| {
                    let label = if running { "Checking" } else { "Run Check" };
                    if primary_button(ui, MiniIcon::Search, label, Vec2::new(width, 34.0)).clicked()
                    {
                        self.start_connection_diagnostics(ui.ctx());
                    }
                });
                if secondary_button(ui, MiniIcon::Refresh, "Retry", Vec2::new(width, 34.0))
                    .clicked()
                {
                    self.start_connection_diagnostics(ui.ctx());
                }
                ui.add_enabled_ui(self.diagnostics_report.is_some(), |ui| {
                    let copy_label =
                        if matches!(status, Some(diagnostics::DiagnosticStatus::Problem)) {
                            "Copy Error"
                        } else {
                            "Copy Report"
                        };
                    if secondary_button(ui, MiniIcon::External, copy_label, Vec2::new(width, 34.0))
                        .clicked()
                    {
                        if let Some(report) = &self.diagnostics_report {
                            ui.output_mut(|output| output.copied_text = report.to_plain_text());
                            self.log(phase::blue(), "Copied diagnostic report.");
                        }
                    }
                });
            });

            ui.add_space(12.0);
            egui::ScrollArea::vertical()
                .id_source("phase-diagnostics-report")
                .max_height((ui.available_height() - 12.0).max(180.0))
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    if running && self.diagnostics_report.is_none() {
                        diagnostics_waiting_card(ui);
                    }
                    if let Some(report) = &self.diagnostics_report {
                        for check in &report.checks {
                            diagnostics_check_card(ui, check);
                            ui.add_space(8.0);
                        }
                    }
                });
        });
    }

    fn show_tray_popup_viewport(&mut self, ctx: &Context) {
        if !self.tray_panel_open {
            return;
        }

        let builder = egui::ViewportBuilder::default()
            .with_title("Phase Animator Controls")
            .with_position(self.tray_panel_pos)
            .with_inner_size(Vec2::new(TRAY_PANEL_WIDTH, TRAY_PANEL_HEIGHT))
            .with_min_inner_size(Vec2::new(TRAY_PANEL_WIDTH, 180.0))
            .with_transparent(false)
            .with_decorations(false)
            .with_resizable(false)
            .with_taskbar(false)
            .with_always_on_top()
            .with_active(true)
            .with_visible(true);

        ctx.show_viewport_immediate(tray_viewport_id(), builder, |tray_ctx, _class| {
            if tray_ctx.input(|input| input.viewport().close_requested()) {
                self.close_tray_popup(tray_ctx);
                return;
            }

            if tray_ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
                self.close_tray_popup(tray_ctx);
                return;
            }

            egui::CentralPanel::default()
                .frame(egui::Frame::none().fill(phase::background()))
                .show(tray_ctx, |ui| {
                    self.tray_panel(ui);
                });
        });
    }

    fn tray_panel(&mut self, ui: &mut Ui) {
        ui.set_min_size(Vec2::new(TRAY_PANEL_WIDTH, TRAY_PANEL_HEIGHT));

        let raw_t = ui.ctx().animate_bool_with_time(
            egui::Id::new(("phase-tray-pop", self.tray_anim_nonce)),
            true,
            0.16,
        );
        let fade_t = ease_out_cubic(raw_t).clamp(0.0, 1.0);
        let panel_width = TRAY_PANEL_WIDTH;
        let panel_height = TRAY_PANEL_HEIGHT;

        ui.horizontal_centered(|ui| {
            egui::Frame::none()
                .fill(color_with_alpha(phase::background(), fade_t))
                .stroke(Stroke::new(1.0, color_with_alpha(phase::line(), fade_t)))
                .rounding(Rounding::same(14.0))
                .inner_margin(Margin::same(10.0))
                .show(ui, |ui| {
                    let content_width = (panel_width - 20.0).max(180.0);
                    ui.set_width(content_width);
                    egui::ScrollArea::vertical()
                        .id_source("phase-tray-panel-scroll")
                        .max_height(panel_height - 20.0)
                        .auto_shrink([false, false])
                        .show(ui, |ui| self.tray_panel_contents(ui, content_width));
                });
        });
    }

    fn tray_panel_contents(&mut self, ui: &mut Ui, content_width: f32) {
        ui.vertical_centered(|ui| {
            ui.set_width(content_width);
            ui.horizontal(|ui| {
                if let Some(logo) = &self.logo {
                    let image = egui::Image::new(logo).fit_to_exact_size(Vec2::splat(34.0));
                    ui.add(image);
                }
                ui.add_space(8.0);
                ui.vertical(|ui| {
                    ui.label(
                        RichText::new("Phase Animator")
                            .font(FontId::proportional(15.0))
                            .strong()
                            .color(phase::text()),
                    );
                    ui.label(
                        RichText::new("Tray controls")
                            .font(FontId::proportional(11.5))
                            .color(phase::text_secondary()),
                    );
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if secondary_button(ui, MiniIcon::Download, "Hide", Vec2::new(78.0, 30.0))
                        .clicked()
                    {
                        self.close_tray_popup(ui.ctx());
                    }
                });
            });

            ui.add_space(12.0);
            egui::Frame::none()
                .fill(phase::surface())
                .stroke(Stroke::new(1.0, phase::line()))
                .rounding(Rounding::same(8.0))
                .inner_margin(Margin::same(10.0))
                .show(ui, |ui| {
                    let info_width = (content_width - 10.0).max(160.0);
                    ui.set_width(info_width);
                    compact_info_row(ui, "Status", phase_text(self.phase), info_width);
                    compact_info_row(ui, "Account", &self.account_summary(), info_width);
                    compact_info_row(ui, "Latest", &self.release_summary(), info_width);
                });

            ui.add_space(10.0);
            let action_width = content_width;
            if primary_button(
                ui,
                MiniIcon::External,
                "Open Window",
                Vec2::new(action_width, 34.0),
            )
            .clicked()
            {
                self.show_main_window(ui.ctx());
            }

            if secondary_button(
                ui,
                MiniIcon::Refresh,
                "Check Updates",
                Vec2::new(action_width, 34.0),
            )
            .clicked()
                && !self.is_busy()
            {
                self.select_tab(ViewTab::Install);
                self.start_check();
            }

            ui.add_enabled_ui(self.phase == InstallPhase::Ready && !self.is_busy(), |ui| {
                let label = if self.has_local_phase_install() {
                    "Install Update"
                } else {
                    "Install Plugin"
                };
                if secondary_button(ui, MiniIcon::Bolt, label, Vec2::new(action_width, 34.0))
                    .clicked()
                {
                    self.select_tab(ViewTab::Install);
                    self.start_install();
                }
            });

            ui.add_enabled_ui(self.selected_folder.is_some(), |ui| {
                if secondary_button(
                    ui,
                    MiniIcon::Folder,
                    "Open Plugin Folder",
                    Vec2::new(action_width, 34.0),
                )
                .clicked()
                {
                    self.open_selected_folder();
                }
            });

            if secondary_button(
                ui,
                MiniIcon::External,
                "Quit",
                Vec2::new(action_width, 34.0),
            )
            .clicked()
            {
                self.request_quit(ui.ctx());
            }

            ui.add_space(8.0);
        });
    }

    fn handle_close_request(&mut self, ctx: &Context) {
        if !ctx.input(|input| input.viewport().close_requested()) {
            return;
        }

        if self.allow_quit {
            self.cleanup_tray();
            return;
        }

        ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        if self.tray_panel_open {
            self.close_tray_popup(ctx);
        }
        if !self.close_dialog_open {
            self.dialog_anim_nonce = self.dialog_anim_nonce.wrapping_add(1);
        }
        self.close_dialog_open = true;
    }

    fn draw_close_dialog(&mut self, ctx: &Context) {
        if !self.close_dialog_open {
            return;
        }

        let raw_t = ctx.animate_bool_with_time(
            egui::Id::new(("phase-close-dialog-pop", self.dialog_anim_nonce)),
            true,
            0.18,
        );
        let pop_t = ease_out_back(raw_t).clamp(0.0, 1.0);
        let fade_t = ease_out_cubic(raw_t).clamp(0.0, 1.0);
        let scale = 0.94 + 0.06 * pop_t;
        let width = 378.0 * scale;
        let title_color = color_with_alpha(phase::text(), fade_t);
        let body_color = color_with_alpha(phase::text_secondary(), fade_t);

        egui::Window::new("Keep Phase Animator running?")
            .anchor(Align2::CENTER_CENTER, Vec2::new(0.0, (1.0 - pop_t) * 12.0))
            .collapsible(false)
            .resizable(false)
            .title_bar(false)
            .frame(
                egui::Frame::none()
                    .fill(color_with_alpha(phase::surface(), fade_t))
                    .stroke(Stroke::new(1.0, color_with_alpha(phase::line(), fade_t)))
                    .rounding(Rounding::same(10.0))
                    .inner_margin(Margin::same(18.0 * scale)),
            )
            .show(ctx, |ui| {
                ui.set_width(width);
                ui.label(
                    RichText::new("Keep Phase Animator running?")
                        .font(FontId::proportional(18.0))
                        .strong()
                        .color(title_color),
                );
                ui.add_space(6.0);
                ui.label(
                    RichText::new("Minimize it to the tray to keep update checks and quick install actions available.")
                        .font(FontId::proportional(13.0))
                        .color(body_color),
                );
                if !self.tray_available() {
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new("Tray icon is not available in this build.")
                            .font(FontId::proportional(12.5))
                            .color(color_with_alpha(phase::warning(), fade_t)),
                    );
                }
                ui.add_space(16.0);
                ui.horizontal(|ui| {
                    if secondary_button(ui, MiniIcon::External, "Quit", Vec2::new(104.0, 36.0))
                        .clicked()
                    {
                        self.request_quit(ctx);
                    }
                    if secondary_button(ui, MiniIcon::Gear, "Cancel", Vec2::new(112.0, 36.0))
                        .clicked()
                    {
                        self.close_dialog_open = false;
                    }
                    ui.add_enabled_ui(self.tray_available(), |ui| {
                        if primary_button(
                            ui,
                            MiniIcon::Download,
                            "Minimize",
                            Vec2::new(132.0, 36.0),
                        )
                        .clicked()
                        {
                            self.minimize_to_tray(ctx);
                        }
                    });
                });
            });
    }

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
        let target_x = self.active_tab.index() as f32;
        let dt = ui.input(|i| i.stable_dt).clamp(0.0, 1.0 / 30.0);
        if self
            .tab_indicator
            .step(target_x, dt, motion::Spring::expressive())
        {
            ui.ctx().request_repaint();
        }
        let indicator_x = self.tab_indicator.value();

        let width = content_w();
        let height = 42.0;

        let (rect, response) = ui.allocate_exact_size(Vec2::new(width, height), Sense::click());
        let tab_width = width / 5.0;
        let hovered_idx = response
            .hover_pos()
            .map(|pos| (((pos.x - rect.left()) / tab_width).floor() as i32).clamp(0, 4));
        let painter = ui.painter();

        painter.rect_filled(rect, Rounding::same(8.0), phase::input());
        painter.rect_stroke(rect, Rounding::same(8.0), Stroke::new(1.0, phase::line()));

        let highlight_rect = Rect::from_min_max(
            Pos2::new(
                rect.left() + indicator_x * tab_width + 2.0,
                rect.top() + 2.0,
            ),
            Pos2::new(
                rect.left() + (indicator_x + 1.0) * tab_width - 2.0,
                rect.bottom() - 2.0,
            ),
        );

        painter.rect_filled(highlight_rect, Rounding::same(6.0), phase::surface());
        painter.rect_stroke(
            highlight_rect,
            Rounding::same(6.0),
            Stroke::new(1.0, color_with_alpha(phase::accent(), 0.55)),
        );

        if response.clicked() {
            if let Some(pos) = response.interact_pointer_pos() {
                let rel_x = pos.x - rect.left();
                let tab_idx = (rel_x / tab_width).floor() as i32;
                if let Some(tab) = match tab_idx {
                    0 => Some(ViewTab::Install),
                    1 => Some(ViewTab::Account),
                    2 => Some(ViewTab::Folders),
                    3 => Some(ViewTab::Video),
                    4 => Some(ViewTab::Options),
                    _ => None,
                } {
                    self.select_tab(tab);
                }
            }
        }

        let labels = ["Install", "Account", "Folders", "Video", "Options"];
        let icons = [
            MiniIcon::Bolt,
            MiniIcon::User,
            MiniIcon::Folder,
            MiniIcon::External,
            MiniIcon::Gear,
        ];
        for i in 0..5 {
            let x_center = rect.left() + (i as f32 + 0.5) * tab_width;
            let y_center = rect.center().y;

            let is_active = match (self.active_tab, i) {
                (ViewTab::Install, 0) => true,
                (ViewTab::Account, 1) => true,
                (ViewTab::Folders, 2) => true,
                (ViewTab::Video, 3) => true,
                (ViewTab::Options, 4) => true,
                _ => false,
            };

            // Soft hover wash on inactive tabs so the bar feels responsive
            // before a click commits. The active tab already owns the slider.
            let is_hovered = !is_active && hovered_idx == Some(i as i32);
            let hover_t = ui.ctx().animate_bool_with_time(
                response.id.with(("tab_hover", i)),
                is_hovered,
                0.12,
            );
            if hover_t > 0.0 {
                let cell = Rect::from_min_max(
                    Pos2::new(rect.left() + i as f32 * tab_width + 2.0, rect.top() + 2.0),
                    Pos2::new(
                        rect.left() + (i as f32 + 1.0) * tab_width - 2.0,
                        rect.bottom() - 2.0,
                    ),
                );
                painter.rect_filled(
                    cell,
                    Rounding::same(6.0),
                    color_with_alpha(phase::surface(), 0.5 * hover_t),
                );
            }

            let base_color = if is_active {
                phase::text()
            } else {
                phase::text_muted()
            };
            // Inactive labels brighten toward the primary text color on hover.
            let color = lerp_color(base_color, phase::text(), hover_t);

            // Center the icon+label as a group, and collapse to an icon when the
            // cell is too narrow for the label — so the bar stays legible from the
            // 296px floor up to the capped width instead of overlapping.
            let icon_size = 15.0;
            let gap = 7.0;
            let galley =
                painter.layout_no_wrap(labels[i].to_string(), FontId::proportional(13.0), color);
            let label_w = galley.size().x;
            let group_w = icon_size + gap + label_w;
            if tab_width >= group_w + 16.0 {
                let left = x_center - group_w / 2.0;
                let icon_rect = Rect::from_center_size(
                    Pos2::new(left + icon_size / 2.0, y_center),
                    Vec2::splat(icon_size),
                );
                draw_icon_at(painter, icon_rect, icons[i], color);
                let text_pos = Pos2::new(left + icon_size + gap, y_center - galley.size().y / 2.0);
                painter.galley(text_pos, galley, color);
            } else {
                let icon_rect = Rect::from_center_size(
                    Pos2::new(x_center, y_center),
                    Vec2::splat(icon_size + 3.0),
                );
                draw_icon_at(painter, icon_rect, icons[i], color);
            }
        }

        // Pointer affordance: the whole strip is clickable.
        if hovered_idx.is_some() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
    }

    fn draw_progress(&self, ui: &mut Ui) {
        let width = card_inner();
        let height = 22.0;

        let (rect, response) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
        let painter = ui.painter();

        painter.rect_filled(rect, Rounding::same(6.0), phase::input());
        painter.rect_stroke(rect, Rounding::same(6.0), Stroke::new(1.0, phase::line()));

        // Glide toward the real value so streamed install ticks ramp instead of
        // snapping. egui handles the dt internally, so this stays smooth at any
        // refresh rate and repaints itself while in motion.
        let progress = ui.ctx().animate_value_with_time(
            response.id.with("progress"),
            self.progress.clamp(0.0, 1.0),
            0.25,
        );
        if progress > 0.001 {
            // Left corners stay rounded from the first pixel; the right corners
            // stay square mid-fill and only round once the bar is nearly full,
            // so the leading edge reads as a crisp wipe rather than a pill.
            let fill_width = (width * progress).clamp(12.0, width);
            let fill_rect =
                Rect::from_min_max(rect.min, Pos2::new(rect.min.x + fill_width, rect.max.y));
            let right = ((progress - 0.92) / 0.08).clamp(0.0, 1.0) * 6.0;
            let rounding = Rounding {
                nw: 6.0,
                sw: 6.0,
                ne: right,
                se: right,
            };
            painter.rect_filled(fill_rect, rounding, phase_color(self.phase));
        }

        let pct_text = format!("{}%", (progress * 100.0).round() as i32);
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
            (content_w() - gap) / 2.0
        };
        let row_size = Vec2::new(content_w(), 60.0);
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
        let dt = ui.input(|i| i.stable_dt).clamp(0.0, 1.0 / 30.0);
        let frame = self.tab_page_motion.step(dt, 0.22);
        if frame.running {
            ui.ctx().request_repaint();
        }

        ui.scope(|ui| {
            ui.add_space(frame.offset.abs() * 0.5);
            ui.set_opacity(frame.opacity);
            match self.active_tab {
                ViewTab::Install => self.install_tab(ui),
                ViewTab::Account => self.account_tab(ui),
                ViewTab::Folders => self.folders_tab(ui),
                ViewTab::Video => self.video_tab(ui),
                ViewTab::Options => self.options_tab(ui),
            }
        });
    }

    fn install_tab(&mut self, ui: &mut Ui) {
        let _time = ui.input(|i| i.time);
        draw_panel(ui, |ui| {
            section_label(ui, "Release");
            ui.add_space(6.0);

            // Single status hero replaces the old stacked metric cards + the
            // redundant "Update status / Available version" frame (the version
            // used to appear three times). One read: what state are we in, and
            // which version is current.
            let latest = self
                .release
                .as_ref()
                .map(|release| release.latest_version.clone())
                .unwrap_or_else(|| "—".to_owned());
            let has_local = self.has_local_phase_install();
            let checking = self.release.is_none() && self.release_error.is_none();
            let available = self
                .release
                .as_ref()
                .map(|release| {
                    release.download_available && !release.blocked && !self.local_release_current
                })
                .unwrap_or(false);

            let (hero_icon, hero_color, hero_state, status_short): (MiniIcon, Color32, &str, &str) =
                if self.release_error.is_some() {
                    (MiniIcon::Info, phase::red(), "Update check failed", "Error")
                } else if checking {
                    (
                        MiniIcon::Clock,
                        phase::blue(),
                        "Checking for updates",
                        "Checking",
                    )
                } else if available {
                    (
                        MiniIcon::Download,
                        phase::accent(),
                        "Update available",
                        "Ready",
                    )
                } else {
                    (
                        MiniIcon::ShieldCheck,
                        phase::green(),
                        "Up to date",
                        "Current",
                    )
                };
            let hero_detail = if checking {
                "Contacting Phase servers".to_owned()
            } else if self.release_error.is_some() {
                "Retry the check below".to_owned()
            } else {
                format!("Latest release v{latest}")
            };

            egui::Frame::none()
                .fill(phase::input())
                .stroke(Stroke::new(1.0, phase::line()))
                .rounding(Rounding::same(8.0))
                .inner_margin(Margin::symmetric(14.0, 12.0))
                .show(ui, |ui| {
                    ui.set_width(card_inner() - 16.0);
                    ui.horizontal(|ui| {
                        draw_icon(ui, hero_icon, Vec2::splat(30.0), hero_color);
                        ui.add_space(12.0);
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 2.0;
                            ui.label(
                                RichText::new(hero_state)
                                    .font(FontId::proportional(16.5))
                                    .strong()
                                    .color(phase::text()),
                            );
                            ui.label(
                                RichText::new(hero_detail)
                                    .font(FontId::proportional(11.5))
                                    .color(phase::text_muted()),
                            );
                        });
                    });
                });

            ui.add_space(8.0);
            egui::Frame::none()
                .fill(phase::input())
                .stroke(Stroke::new(1.0, phase::line()))
                .rounding(Rounding::same(8.0))
                .inner_margin(Margin::symmetric(0.0, 0.0))
                .show(ui, |ui| {
                    ui.set_width(card_inner());
                    stat_grid_flat(
                        ui,
                        &[
                            (
                                MiniIcon::Folder,
                                "Installed",
                                if has_local { "Local build" } else { "Not yet" }.to_owned(),
                            ),
                            (
                                MiniIcon::Rocket,
                                "Latest",
                                if latest == "—" { "Checking" } else { &latest }.to_owned(),
                            ),
                            (MiniIcon::Stack, "Status", status_short.to_owned()),
                        ],
                        3,
                    );
                });

            ui.add_space(12.0);
            let install_ready_text = if self.has_local_phase_install() {
                "Install Update"
            } else {
                "Install Plugin"
            };
            let button_text = match self.phase {
                InstallPhase::Ready => install_ready_text,
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
                    if primary_button(ui, button_icon, button_text, Vec2::new(card_inner(), 48.0))
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
            section_label(ui, "Phase Account");
            ui.add_space(6.0);

            egui::Frame::none()
                .fill(phase::input())
                .stroke(Stroke::new(1.0, phase::line()))
                .rounding(Rounding::same(8.0))
                .inner_margin(Margin::symmetric(14.0, 12.0))
                .show(ui, |ui| {
                    ui.set_width(card_inner() - 28.0);
                    let width = ui.available_width();
                    let busy = self.link_rx.is_some() || self.link_status_rx.is_some();
                    let disconnecting = self.phase_disconnect_rx.is_some();
                    let phase_linked = self.plugin_token.is_some();
                    let link_url = self.link_url.clone();

                    ui.horizontal(|ui| {
                        draw_icon(ui, MiniIcon::User, Vec2::splat(34.0), phase::accent());
                        ui.add_space(10.0);
                        ui.vertical(|ui| {
                            ui.set_width((width - 164.0).max(80.0));
                            let title = self
                                .linked_user
                                .as_ref()
                                .map(display_linked_user)
                                .unwrap_or_else(|| "Phase account".to_owned());
                            scrolling_label(
                                ui,
                                &title,
                                (width - 164.0).max(80.0),
                                FontId::proportional(15.0),
                                phase::text(),
                            );
                            let detail = if self.plugin_token.is_some() {
                                "Connected to this installer"
                            } else if let Some(code) = &self.link_code {
                                code.as_str()
                            } else {
                                "Open browser to sign in and approve this install"
                            };
                            scrolling_label(
                                ui,
                                detail,
                                (width - 164.0).max(80.0),
                                FontId::proportional(11.0),
                                phase::text_muted(),
                            );
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            status_pill(
                                ui,
                                if self.plugin_token.is_some() {
                                    "Connected"
                                } else {
                                    "Not linked"
                                },
                                if self.plugin_token.is_some() {
                                    phase::green()
                                } else {
                                    phase::text_muted()
                                },
                            );
                        });
                    });

                    ui.add_space(12.0);
                    action_grid(ui, 3, |ui, index, size| match index {
                        0 => {
                            if phase_linked {
                                status_action(ui, MiniIcon::Check, "Connected", size);
                            } else {
                                let connect_label = if busy { "Waiting" } else { "Connect" };
                                ui.add_enabled_ui(!busy, |ui| {
                                    if secondary_button(ui, MiniIcon::Link, connect_label, size)
                                        .clicked()
                                    {
                                        self.start_phase_account_link(ui.ctx());
                                    }
                                });
                            }
                        }
                        1 => {
                            if secondary_button(ui, MiniIcon::Refresh, "Check", size).clicked() {
                                if self.plugin_token.is_some() {
                                    self.begin_phase_account_refresh(ui.ctx());
                                } else {
                                    self.begin_link_status_check(ui.ctx());
                                }
                            }
                        }
                        2 => {
                            if phase_linked {
                                ui.add_enabled_ui(!disconnecting, |ui| {
                                    if secondary_button(ui, MiniIcon::Trash, "Disconnect", size)
                                        .clicked()
                                    {
                                        self.start_phase_disconnect(ui.ctx());
                                    }
                                });
                            } else {
                                ui.add_enabled_ui(link_url.is_some(), |ui| {
                                    if secondary_button(ui, MiniIcon::External, "Open", size)
                                        .clicked()
                                    {
                                        if let Some(url) = link_url.clone() {
                                            if let Err(error) = open::that(url) {
                                                self.log(
                                                    phase::warning(),
                                                    format!("Open browser failed: {error}"),
                                                );
                                            }
                                        }
                                    }
                                });
                            }
                        }
                        _ => {}
                    });
                });

            ui.add_space(14.0);
            section_label(ui, "Verified Access");
            ui.add_space(6.0);
            egui::Frame::none()
                .fill(phase::input())
                .stroke(Stroke::new(1.0, phase::line()))
                .rounding(Rounding::same(8.0))
                .inner_margin(Margin::symmetric(14.0, 12.0))
                .show(ui, |ui| {
                    ui.set_width(card_inner() - 28.0);
                    let width = ui.available_width();
                    let oauth_busy =
                        self.roblox_oauth_rx.is_some() || self.roblox_oauth_status_rx.is_some();
                    let activation_busy = self.activation_rx.is_some();
                    let verified_roblox = !self.roblox_user_id.trim().is_empty();
                    let roblox_url = self.roblox_oauth_url.clone();

                    ui.horizontal(|ui| {
                        draw_icon(
                            ui,
                            MiniIcon::ShieldCheck,
                            Vec2::splat(24.0),
                            phase::accent(),
                        );
                        ui.add_space(8.0);
                        ui.vertical(|ui| {
                            ui.set_width((width - 154.0).max(80.0));
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
                                (width - 154.0).max(80.0),
                                FontId::proportional(11.0),
                                phase::text_muted(),
                            );
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            status_pill(
                                ui,
                                if verified_roblox {
                                    "Verified"
                                } else {
                                    "Required"
                                },
                                if verified_roblox {
                                    phase::green()
                                } else {
                                    phase::warning()
                                },
                            );
                        });
                    });

                    ui.add_space(12.0);
                    ui.label(
                        RichText::new("Phase license key")
                            .font(FontId::proportional(11.0))
                            .color(phase::text_muted()),
                    );
                    ui.add(
                        egui::TextEdit::singleline(&mut self.license_key)
                            .desired_width(width)
                            .password(true)
                            .hint_text("Optional if Roblox ownership verifies"),
                    );

                    ui.add_space(12.0);
                    action_grid(ui, 3, |ui, index, size| match index {
                        0 => {
                            if verified_roblox {
                                status_action(ui, MiniIcon::Check, "Verified", size);
                            } else {
                                let label = if oauth_busy { "Waiting" } else { "Roblox" };
                                ui.add_enabled_ui(!oauth_busy, |ui| {
                                    if secondary_button(ui, MiniIcon::ShieldCheck, label, size)
                                        .clicked()
                                    {
                                        self.start_roblox_oauth(ui.ctx());
                                    }
                                });
                            }
                        }
                        1 => {
                            if verified_roblox {
                                ui.add_enabled_ui(!activation_busy, |ui| {
                                    if secondary_button(ui, MiniIcon::Key, "License", size)
                                        .clicked()
                                    {
                                        self.start_activation(ui.ctx());
                                    }
                                });
                            } else {
                                ui.add_enabled_ui(roblox_url.is_some(), |ui| {
                                    if secondary_button(ui, MiniIcon::External, "Open", size)
                                        .clicked()
                                    {
                                        if let Some(url) = roblox_url.clone() {
                                            if let Err(error) = open::that(url) {
                                                self.log(
                                                    phase::warning(),
                                                    format!("Open browser failed: {error}"),
                                                );
                                            }
                                        }
                                    }
                                });
                            }
                        }
                        2 => {
                            ui.add_enabled_ui(verified_roblox, |ui| {
                                if secondary_button(ui, MiniIcon::Trash, "Disconnect", size)
                                    .clicked()
                                {
                                    self.disconnect_roblox_account();
                                }
                            });
                        }
                        _ => {}
                    });

                    if let Some(activation) = &self.activation {
                        ui.add_space(12.0);
                        stat_grid(
                            ui,
                            &[
                                (MiniIcon::Key, "Access", activation.licensee.clone()),
                                (
                                    MiniIcon::ShieldCheck,
                                    "Mode",
                                    activation.activation_mode.clone(),
                                ),
                            ],
                            2,
                        );
                    } else if let Some(error) = &self.activation_error {
                        ui.add_space(10.0);
                        scrolling_label(ui, error, width, FontId::proportional(11.0), phase::red());
                    }
                });
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
                        card_inner() - 74.0,
                        FontId::monospace(13.0),
                        phase::text(),
                    );
                });
            });

            if let Some(candidate) = self.selected_candidate() {
                ui.add_space(8.0);
                let file_count = candidate.plugin_files.len().to_string();
                let size = candidate
                    .plugin_files
                    .first()
                    .map(|plugin_file| human_size(plugin_file.size_bytes))
                    .unwrap_or_else(|| "None".to_owned());
                let backup = candidate
                    .plugin_files
                    .first()
                    .and_then(|plugin_file| plugin_file.modified)
                    .map(|_| "Recommended")
                    .unwrap_or("Clean");
                stat_grid(
                    ui,
                    &[
                        (MiniIcon::Stack, "Files", file_count),
                        (MiniIcon::Folder, "Size", size),
                        (MiniIcon::MapPin, "Source", candidate.source.clone()),
                        (MiniIcon::ShieldCheck, "Backup", backup.to_owned()),
                    ],
                    2,
                );
            }

            ui.add_space(10.0);
            section_label(ui, "Detected Paths");
            ui.add_space(4.0);
            self.folder_candidates(ui);

            ui.add_space(12.0);
            ui.horizontal(|ui| {
                let btn_width = (card_inner() - 16.0) / 3.0;
                if secondary_button(ui, MiniIcon::Folder, "Browse", Vec2::new(btn_width, 36.0))
                    .clicked()
                {
                    self.choose_folder();
                }
                if secondary_button(ui, MiniIcon::Eye, "Open", Vec2::new(btn_width, 36.0)).clicked()
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

    fn video_tab(&mut self, ui: &mut Ui) {
        draw_panel(ui, |ui| {
            section_label(ui, "Video Reference");
            ui.add_space(6.0);

            let (bridge_label, bridge_color) = if self.video_bridge_connected {
                ("Studio linked", phase::green())
            } else if self.video_bridge_listening {
                ("Ready for Studio", phase::accent())
            } else {
                ("Bridge offline", phase::warning())
            };
            let source_hint = if self.video_source.trim().is_empty() {
                "Paste a YouTube URL or pick a local MP4.".to_owned()
            } else {
                self.video_last_reference_status.clone()
            };

            egui::Frame::none()
                .fill(phase::input())
                .stroke(Stroke::new(1.0, phase::line()))
                .rounding(Rounding::same(8.0))
                .inner_margin(Margin::symmetric(12.0, 12.0))
                .show(ui, |ui| {
                    let inner_w = ui.available_width().max(1.0);
                    ui.horizontal(|ui| {
                        draw_icon(ui, MiniIcon::FilmStrip, Vec2::splat(30.0), phase::accent());
                        ui.add_space(8.0);
                        ui.vertical(|ui| {
                            let text_w = (inner_w - 180.0).max(80.0);
                            ui.add_sized(
                                Vec2::new(text_w, 20.0),
                                egui::Label::new(
                                    RichText::new("Open and link a video")
                                        .font(FontId::proportional(14.0))
                                        .strong()
                                        .color(phase::text()),
                                )
                                .wrap(false),
                            );
                            scrolling_label(
                                ui,
                                &source_hint,
                                text_w,
                                FontId::proportional(11.0),
                                phase::text_muted(),
                            );
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            status_pill(ui, bridge_label, bridge_color);
                        });
                    });

                    ui.add_space(12.0);
                    ui.label(
                        RichText::new("Video")
                            .font(FontId::proportional(11.0))
                            .color(phase::text_muted()),
                    );
                    ui.horizontal(|ui| {
                        let browse_w = 92.0;
                        let gap = ui.spacing().item_spacing.x;
                        let source_response = ui.add(
                            egui::TextEdit::singleline(&mut self.video_source)
                                .desired_width((inner_w - browse_w - gap).max(80.0))
                                .hint_text("YouTube URL or local MP4 path"),
                        );
                        if source_response.changed() && self.video_title.trim().is_empty() {
                            self.video_title =
                                video_reference::default_title_for(&self.video_source);
                        }
                        if secondary_button(ui, MiniIcon::Folder, "Browse", Vec2::new(92.0, 32.0))
                            .clicked()
                        {
                            self.pick_video_file();
                        }
                    });

                    ui.add_space(8.0);
                    ui.label(
                        RichText::new("Name")
                            .font(FontId::proportional(11.0))
                            .color(phase::text_muted()),
                    );
                    ui.add(
                        egui::TextEdit::singleline(&mut self.video_title)
                            .desired_width(inner_w)
                            .hint_text("Optional display name"),
                    );

                    ui.add_space(12.0);
                    if primary_button(
                        ui,
                        MiniIcon::External,
                        "Open Video",
                        Vec2::new(inner_w, 38.0),
                    )
                    .clicked()
                    {
                        self.open_video_popup();
                    }

                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        let btn_width = (inner_w - ui.spacing().item_spacing.x * 2.0) / 3.0;
                        if secondary_button(
                            ui,
                            MiniIcon::Link,
                            "Send Link",
                            Vec2::new(btn_width, 32.0),
                        )
                        .clicked()
                        {
                            self.send_video_reference();
                        }
                        let sync_label = if self.video_sync_enabled {
                            "Sync On"
                        } else {
                            "Sync Off"
                        };
                        let sync_icon = if self.video_sync_enabled {
                            MiniIcon::Check
                        } else {
                            MiniIcon::Pause
                        };
                        if secondary_button(ui, sync_icon, sync_label, Vec2::new(btn_width, 32.0))
                            .clicked()
                        {
                            self.video_sync_enabled = !self.video_sync_enabled;
                            self.send_video_sync_enabled();
                        }
                        if secondary_button(
                            ui,
                            MiniIcon::Trash,
                            "Clear",
                            Vec2::new(btn_width, 32.0),
                        )
                        .clicked()
                        {
                            self.clear_video_reference();
                        }
                    });
                });

            ui.add_space(10.0);
            egui::Frame::none()
                .fill(phase::input())
                .stroke(Stroke::new(1.0, phase::line()))
                .rounding(Rounding::same(8.0))
                .inner_margin(Margin::symmetric(12.0, 10.0))
                .show(ui, |ui| {
                    let inner_w = ui.available_width().max(1.0);
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.set_width((inner_w - 120.0).max(80.0));
                            ui.label(
                                RichText::new("Studio Sync")
                                    .font(FontId::proportional(13.0))
                                    .strong()
                                    .color(phase::text()),
                            );
                            let sync_hint = if self.video_sync_enabled {
                                "Viewer and Phase are following each other."
                            } else {
                                "Mirror play, pause, and scrubbing."
                            };
                            scrolling_label(
                                ui,
                                sync_hint,
                                (inner_w - 120.0).max(80.0),
                                FontId::proportional(11.0),
                                phase::text_muted(),
                            );
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            status_pill(
                                ui,
                                if self.video_sync_enabled { "On" } else { "Off" },
                                if self.video_sync_enabled {
                                    phase::green()
                                } else {
                                    phase::text_muted()
                                },
                            );
                        });
                    });
                    ui.add_space(8.0);
                    scrolling_label(
                        ui,
                        &self.video_last_plugin_state,
                        inner_w,
                        FontId::proportional(11.0),
                        phase::text_muted(),
                    );
                    ui.add_space(4.0);
                    scrolling_label(
                        ui,
                        &self.video_last_reference_status,
                        inner_w,
                        FontId::proportional(11.0),
                        phase::text_muted(),
                    );
                });

            ui.add_space(12.0);
            egui::CollapsingHeader::new("Timing")
                .default_open(false)
                .show(ui, |ui| {
                    egui::Frame::none()
                        .fill(phase::input())
                        .stroke(Stroke::new(1.0, phase::line()))
                        .rounding(Rounding::same(8.0))
                        .inner_margin(Margin::symmetric(12.0, 10.0))
                        .show(ui, |ui| {
                            let gap = ui.spacing().item_spacing.x;
                            let timing_width = ui.available_width().max(1.0);
                            let field_width = ((timing_width - gap * 2.0) / 3.0).max(72.0);
                            ui.horizontal(|ui| {
                                small_number_field(
                                    ui,
                                    "Duration",
                                    &mut self.video_duration_seconds,
                                    "0",
                                    field_width,
                                );
                                small_number_field(
                                    ui,
                                    "FPS",
                                    &mut self.video_fps,
                                    "60",
                                    field_width,
                                );
                                small_number_field(
                                    ui,
                                    "Start",
                                    &mut self.video_start_frame,
                                    "0",
                                    field_width,
                                );
                            });
                            ui.add_space(8.0);
                            ui.horizontal(|ui| {
                                small_number_field(
                                    ui,
                                    "Offset",
                                    &mut self.video_offset_seconds,
                                    "0",
                                    field_width,
                                );
                                small_number_field(
                                    ui,
                                    "Rate",
                                    &mut self.video_playback_rate,
                                    "1",
                                    field_width,
                                );
                                ui.vertical(|ui| {
                                    ui.set_width(field_width);
                                    ui.label(
                                        RichText::new("TYPE")
                                            .font(FontId::proportional(10.0))
                                            .color(phase::text_muted()),
                                    );
                                    ui.add_sized(
                                        Vec2::new(field_width, 24.0),
                                        egui::Label::new(
                                            RichText::new(
                                                video_reference::source_kind_for(
                                                    &self.video_source,
                                                )
                                                .as_protocol_str(),
                                            )
                                            .font(FontId::proportional(12.0))
                                            .color(phase::accent()),
                                        )
                                        .wrap(false),
                                    );
                                });
                            });
                        });
                });

            ui.add_space(8.0);
            egui::CollapsingHeader::new("Advanced Connection")
                .default_open(false)
                .show(ui, |ui| {
                    egui::Frame::none()
                        .fill(phase::input())
                        .stroke(Stroke::new(1.0, phase::line()))
                        .rounding(Rounding::same(8.0))
                        .inner_margin(Margin::symmetric(12.0, 10.0))
                        .show(ui, |ui| {
                            let inner_w = ui.available_width().max(1.0);
                            ui.label(
                                RichText::new(self.video_bridge_config.url())
                                    .font(FontId::monospace(12.0))
                                    .color(phase::text()),
                            );
                            ui.add_space(8.0);
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new("Token")
                                        .font(FontId::proportional(11.0))
                                        .color(phase::text_muted()),
                                );
                                ui.add(
                                    egui::TextEdit::singleline(&mut self.video_bridge_config.token)
                                        .desired_width((inner_w - 64.0).max(80.0))
                                        .password(true)
                                        .hint_text("optional"),
                                );
                            });
                            ui.add_space(10.0);
                            ui.horizontal(|ui| {
                                let btn_width = (inner_w - ui.spacing().item_spacing.x * 2.0) / 3.0;
                                if secondary_button(
                                    ui,
                                    MiniIcon::Refresh,
                                    "Restart",
                                    Vec2::new(btn_width, 32.0),
                                )
                                .clicked()
                                {
                                    self.restart_video_bridge();
                                }
                                if secondary_button(
                                    ui,
                                    MiniIcon::PlugsConnected,
                                    "Ping",
                                    Vec2::new(btn_width, 32.0),
                                )
                                .clicked()
                                {
                                    self.send_video_ping();
                                }
                                if secondary_button(
                                    ui,
                                    MiniIcon::Check,
                                    "Apply",
                                    Vec2::new(btn_width, 32.0),
                                )
                                .clicked()
                                {
                                    self.send_video_sync_enabled();
                                }
                            });

                            ui.add_space(12.0);
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new("Seconds")
                                        .font(FontId::proportional(11.0))
                                        .color(phase::text_muted()),
                                );
                                ui.add(
                                    egui::TextEdit::singleline(&mut self.video_position_input)
                                        .desired_width(94.0),
                                );
                            });
                            ui.add_space(8.0);
                            ui.horizontal(|ui| {
                                let btn_width = (inner_w - ui.spacing().item_spacing.x * 2.0) / 3.0;
                                if secondary_button(
                                    ui,
                                    MiniIcon::Refresh,
                                    "Seek",
                                    Vec2::new(btn_width, 32.0),
                                )
                                .clicked()
                                {
                                    self.seek_video_sync();
                                }
                                let play_label = if self.video_playing { "Stop" } else { "Play" };
                                let play_icon = if self.video_playing {
                                    MiniIcon::Pause
                                } else {
                                    MiniIcon::Play
                                };
                                if secondary_button(
                                    ui,
                                    play_icon,
                                    play_label,
                                    Vec2::new(btn_width, 32.0),
                                )
                                .clicked()
                                {
                                    self.set_video_playing(!self.video_playing);
                                }
                                if secondary_button(
                                    ui,
                                    MiniIcon::Broadcast,
                                    "State",
                                    Vec2::new(btn_width, 32.0),
                                )
                                .clicked()
                                {
                                    self.send_video_timeline("sync.timeline");
                                }
                            });
                        });
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
                        ui.set_width(card_inner() - 24.0);
                        let current = self
                            .selected_theme
                            .as_ref()
                            .map(|theme| theme.title.as_str())
                            .unwrap_or("Default Phase");
                        ui.horizontal(|ui| {
                            draw_icon(ui, MiniIcon::Palette, Vec2::splat(24.0), phase::accent());
                            ui.add_space(8.0);
                            ui.vertical(|ui| {
                                scrolling_label(
                                    ui,
                                    current,
                                    card_inner() - 150.0,
                                    FontId::proportional(13.5),
                                    phase::text(),
                                );
                                let image_summary = self
                                    .selected_theme
                                    .as_ref()
                                    .and_then(|theme| theme.background_image_id.as_deref())
                                    .filter(|image_id| !image_id.trim().is_empty())
                                    .unwrap_or("Theme color background");
                                scrolling_label(
                                    ui,
                                    image_summary,
                                    card_inner() - 150.0,
                                    FontId::proportional(10.5),
                                    phase::text_muted(),
                                );
                            });
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    status_pill(
                                        ui,
                                        self.theme_background_mode.label(),
                                        phase::accent(),
                                    );
                                },
                            );
                        });
                        ui.add_space(8.0);
                        stat_grid(
                            ui,
                            &[
                                (MiniIcon::Sparkle, "Active", current.to_owned()),
                                (
                                    MiniIcon::Eye,
                                    "Mode",
                                    self.theme_background_mode.label().to_owned(),
                                ),
                                (
                                    MiniIcon::Stack,
                                    "Loaded",
                                    self.theme_assets.len().to_string(),
                                ),
                            ],
                            3,
                        );
                    });

                ui.add_space(8.0);
                ui.horizontal_centered(|ui| {
                    let btn_width = (card_inner() - 16.0) / 3.0;
                    for mode in [
                        ThemeBackgroundMode::Crop,
                        ThemeBackgroundMode::Fit,
                        ThemeBackgroundMode::Stretch,
                    ] {
                        let selected = self.theme_background_mode == mode;
                        let label = mode.label();
                        if secondary_button(
                            ui,
                            if selected {
                                MiniIcon::Check
                            } else {
                                MiniIcon::Palette
                            },
                            label,
                            Vec2::new(btn_width, 34.0),
                        )
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
                    let btn_width = (card_inner() - 8.0) / 2.0;
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
                    if secondary_button(ui, MiniIcon::Refresh, "Default", Vec2::new(btn_width, 36.0))
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
                        card_inner(),
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
                            Vec2::new(card_inner(), 36.0),
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
                        card_inner(),
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
                        card_inner(),
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
                        ui.set_width(card_inner() - 16.0);
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

                section_label(ui, "Plugin Recovery");
                ui.add_space(6.0);
                egui::Frame::none()
                    .fill(phase::input())
                    .stroke(Stroke::new(1.0, phase::line()))
                    .rounding(Rounding::same(8.0))
                    .inner_margin(Margin::symmetric(14.0, 10.0))
                    .show(ui, |ui| {
                        ui.set_width(card_inner() - 16.0);
                        ui.label(
                            RichText::new("Reset Roblox plugin data")
                                .font(FontId::proportional(14.0))
                                .strong()
                                .color(phase::text()),
                        );
                        ui.add_space(4.0);
                        scrolling_label(
                            ui,
                            "Scans Roblox Studio plugin settings, backs up selected Phase data, then deletes only those keys. Close Studio first.",
                            card_inner() - 16.0,
                            FontId::proportional(11.0),
                            phase::text_muted(),
                        );
                        ui.add_space(10.0);

                        stat_grid(
                            ui,
                            &[
                                (
                                    MiniIcon::Stack,
                                    "Files",
                                    self.plugin_settings_inventory
                                        .files_with_phase_keys
                                        .to_string(),
                                ),
                                (
                                    MiniIcon::Palette,
                                    "Themes",
                                    self.plugin_settings_inventory.theme_keys.to_string(),
                                ),
                                (
                                    MiniIcon::Key,
                                    "Keybinds",
                                    self.plugin_settings_inventory.keybind_keys.to_string(),
                                ),
                            ],
                            3,
                        );
                        ui.add_space(10.0);

                        ui.checkbox(
                            &mut self.plugin_settings_reset_themes,
                            RichText::new("Themes")
                                .font(FontId::proportional(13.0))
                                .color(phase::text_secondary()),
                        );
                        ui.add_space(6.0);
                        ui.checkbox(
                            &mut self.plugin_settings_reset_keybinds,
                            RichText::new("Keybinds")
                                .font(FontId::proportional(13.0))
                                .color(phase::text_secondary()),
                        );
                        ui.add_space(10.0);

                        if secondary_button(
                            ui,
                            MiniIcon::Refresh,
                            "Scan Settings",
                            Vec2::new(card_inner() - 16.0, 34.0),
                        )
                        .clicked()
                        {
                            self.plugin_settings_inventory = phase_plugin_settings_inventory();
                            self.plugin_data_reset_status =
                                Some("Refreshed Roblox plugin settings scan.".to_owned());
                        }
                        ui.add_space(8.0);

                        if self.plugin_data_reset_confirm {
                            scrolling_label(
                                ui,
                                "Are you sure? Selected settings will be backed up, then removed from Roblox Studio plugin storage.",
                                card_inner() - 16.0,
                                FontId::proportional(11.0),
                                phase::warning(),
                            );
                            ui.add_space(8.0);
                            ui.horizontal(|ui| {
                                let btn_width = (card_inner() - 24.0) / 2.0;
                                if secondary_button(
                                    ui,
                                    MiniIcon::Trash,
                                    "Backup + Delete",
                                    Vec2::new(btn_width, 34.0),
                                )
                                .clicked()
                                {
                                    self.reset_phase_plugin_data();
                                }
                                if secondary_button(
                                    ui,
                                    MiniIcon::External,
                                    "Cancel",
                                    Vec2::new(btn_width, 34.0),
                                )
                                .clicked()
                                {
                                    self.plugin_data_reset_confirm = false;
                                }
                            });
                        } else if secondary_button(
                            ui,
                            MiniIcon::Trash,
                            "Backup + Delete Selected",
                            Vec2::new(card_inner() - 16.0, 36.0),
                        )
                        .clicked()
                        {
                            self.plugin_data_reset_confirm = true;
                        }

                        if let Some(status) = &self.plugin_data_reset_status {
                            ui.add_space(8.0);
                            scrolling_label(
                                ui,
                                status,
                                card_inner() - 16.0,
                                FontId::proportional(11.0),
                                phase::text_secondary(),
                            );
                        }
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
                        ui.set_width(card_inner() - 16.0);
                        let latest = self
                            .app_update
                            .as_ref()
                            .map(|update| update.version.as_str())
                            .unwrap_or("Current");
                        let mut update_items = vec![
                            (
                                MiniIcon::Stack,
                                "Installed",
                                env!("CARGO_PKG_VERSION").to_owned(),
                            ),
                            (MiniIcon::CloudArrowDown, "Latest", latest.to_owned()),
                        ];
                        if let Some(update) = &self.app_update {
                            update_items.push((
                                MiniIcon::Download,
                                "Package",
                                update.asset_name.clone(),
                            ));
                        }
                        stat_grid_flat(ui, &update_items, update_items.len());
                    });

                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    let btn_width = (card_inner() - 8.0) / 2.0;
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
                        card_inner() - 16.0,
                        FontId::proportional(11.0),
                        phase::warning(),
                    );
                }

                ui.add_space(16.0);

                section_label(ui, "Connection Diagnostics");
                ui.add_space(6.0);
                egui::Frame::none()
                    .fill(phase::input())
                    .stroke(Stroke::new(1.0, phase::line()))
                    .rounding(Rounding::same(8.0))
                    .inner_margin(Margin::symmetric(14.0, 10.0))
                    .show(ui, |ui| {
                        ui.set_width(card_inner() - 16.0);
                        let status = self
                            .diagnostics_report
                            .as_ref()
                            .map(diagnostics::DiagnosticReport::overall_status);
                        let status_text = if self.diagnostics_rx.is_some() {
                            "Checking"
                        } else {
                            status
                                .map(diagnostics::DiagnosticStatus::label)
                                .unwrap_or("Ready")
                        };
                        let status_color = match status {
                            Some(diagnostics::DiagnosticStatus::Good) => phase::green(),
                            Some(diagnostics::DiagnosticStatus::Warning) => phase::warning(),
                            Some(diagnostics::DiagnosticStatus::Problem) => phase::red(),
                            None => phase::blue(),
                        };
                        ui.horizontal(|ui| {
                            draw_icon(ui, MiniIcon::Search, Vec2::splat(16.0), status_color);
                            ui.add_space(6.0);
                            ui.vertical(|ui| {
                                ui.label(
                                    RichText::new("Phase server and install checks")
                                        .font(FontId::proportional(12.5))
                                        .color(phase::text_secondary()),
                                );
                                scrolling_label(
                                    ui,
                                    self.diagnostics_report
                                        .as_ref()
                                        .map(|report| report.summary.as_str())
                                        .unwrap_or("Open a simple connection check window."),
                                    card_inner() - 148.0,
                                    FontId::proportional(11.0),
                                    phase::text_muted(),
                                );
                            });
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    status_pill(ui, status_text, status_color);
                                },
                            );
                        });
                    });

                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    let btn_width = (card_inner() - 8.0) / 2.0;
                    let running = self.diagnostics_rx.is_some();
                    ui.add_enabled_ui(!running, |ui| {
                        if secondary_button(
                            ui,
                            MiniIcon::Search,
                            if running { "Checking" } else { "Diagnose" },
                            Vec2::new(btn_width, 36.0),
                        )
                        .clicked()
                        {
                            self.start_connection_diagnostics(ui.ctx());
                        }
                    });
                    if secondary_button(ui, MiniIcon::External, "Open", Vec2::new(btn_width, 36.0))
                        .clicked()
                    {
                        self.diagnostics_open = true;
                    }
                });

                ui.add_space(16.0);

                section_label(ui, "About");
                ui.add_space(6.0);
                egui::Frame::none()
                    .fill(phase::input())
                    .stroke(Stroke::new(1.0, phase::line()))
                    .rounding(Rounding::same(8.0))
                    .inner_margin(Margin::symmetric(14.0, 10.0))
                    .show(ui, |ui| {
                        ui.set_width(card_inner() - 16.0);
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
        self.ensure_theme_preview_fetch(ui.ctx(), &asset);
        let preview = self.theme_preview_textures.get(&asset.id).cloned();
        let accent = phase::hex_color(asset.theme_preview.accent.trim());
        let tint = accent.unwrap_or_else(phase::accent);
        let fallback_bg =
            phase::hex_color(asset.theme_preview.background.trim()).unwrap_or_else(phase::surface);

        let hover_id = egui::Id::new(("theme_row_hover", &asset.id));

        let slot = ui.painter().add(egui::Shape::Noop);
        let inner = egui::Frame::none()
            .fill(Color32::TRANSPARENT)
            .stroke(Stroke::NONE)
            .rounding(Rounding::same(8.0))
            .inner_margin(Margin::symmetric(THEME_ROW_MARGIN, 10.0))
            .show(ui, |ui| {
                ui.set_width(card_inner() - 24.0);
                ui.horizontal(|ui| {
                    let thumb_size = Vec2::new(68.0, 42.0);
                    let (thumb_rect, _) = ui.allocate_exact_size(thumb_size, Sense::hover());
                    if let Some(texture) = &preview {
                        let (_, uv) = theme_background_layout(
                            thumb_rect,
                            texture.size_vec2(),
                            ThemeBackgroundMode::Crop,
                        );
                        ui.painter()
                            .image(texture.id(), thumb_rect, uv, Color32::WHITE);
                    } else {
                        ui.painter()
                            .rect_filled(thumb_rect, Rounding::same(6.0), fallback_bg);
                        draw_icon_at(
                            ui.painter(),
                            Rect::from_center_size(thumb_rect.center(), Vec2::splat(18.0)),
                            MiniIcon::Palette,
                            tint,
                        );
                    }
                    ui.painter().rect_stroke(
                        thumb_rect,
                        Rounding::same(6.0),
                        Stroke::new(1.0, color_with_alpha(tint, 0.72)),
                    );
                    ui.add_space(8.0);
                    ui.vertical(|ui| {
                        scrolling_label(
                            ui,
                            &asset.title,
                            card_inner() - 224.0,
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
                            card_inner() - 224.0,
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

        let rect = inner.response.rect;
        let hovered = ui.rect_contains_pointer(rect);
        let hover_t = ui.ctx().animate_bool_with_time(hover_id, hovered, 0.14);

        let mut shapes = vec![egui::Shape::rect_filled(
            rect,
            Rounding::same(8.0),
            fallback_bg,
        )];
        if let Some(texture) = &preview {
            let (_, uv) = theme_background_layout(
                rect.shrink(1.0),
                texture.size_vec2(),
                ThemeBackgroundMode::Crop,
            );
            shapes.push(egui::Shape::image(
                texture.id(),
                rect.shrink(1.0),
                uv,
                color_with_alpha(Color32::WHITE, 0.58 + 0.14 * hover_t),
            ));
        }
        shapes.push(egui::Shape::rect_filled(
            rect,
            Rounding::same(8.0),
            color_with_alpha(Color32::BLACK, 0.48 - 0.08 * hover_t),
        ));
        let border = lerp_color(phase::line(), tint, 0.35 + 0.45 * hover_t);
        shapes.push(egui::Shape::rect_stroke(
            rect,
            Rounding::same(8.0),
            Stroke::new(1.0, border),
        ));
        ui.painter().set(slot, egui::Shape::Vec(shapes));
    }

    fn theme_search_row(&mut self, ui: &mut Ui) {
        let width = card_inner();
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
                self.refresh_local_release_status();
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
            ui.set_min_width(content_w());
            ui.set_max_width(content_w());

            egui::Frame::none()
                .fill(Color32::from_rgb(10, 8, 16))
                .stroke(Stroke::new(1.0, phase::line()))
                .rounding(Rounding::same(6.0))
                .inner_margin(Margin::same(8.0))
                .show(ui, |ui| {
                    ui.set_min_width(card_inner());
                    ui.set_max_width(card_inner());
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
                                    card_inner() - 76.0,
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

#[cfg(any(target_os = "windows", target_os = "macos"))]
fn load_tray_icon() -> Option<TrayIconImage> {
    let bytes = include_bytes!("../assets/PhaseAnimator.png");
    let image = image::load_from_memory(bytes).ok()?.to_rgba8();
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

    let resized = image::imageops::resize(&square, 32, 32, image::imageops::FilterType::Lanczos3);
    TrayIconImage::from_rgba(resized.into_raw(), 32, 32).ok()
}

fn tray_viewport_id() -> egui::ViewportId {
    egui::ViewportId::from_hash_of(TRAY_VIEWPORT_KEY)
}

fn diagnostics_viewport_id() -> egui::ViewportId {
    egui::ViewportId::from_hash_of(DIAGNOSTICS_VIEWPORT_KEY)
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
    MAIN_HWND.store(hwnd as isize, Ordering::Relaxed);
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

#[cfg(target_os = "windows")]
fn reveal_window_for_tray_signal() {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SW_SHOWNORMAL, SetForegroundWindow, ShowWindow,
    };

    let hwnd = MAIN_HWND.load(Ordering::Relaxed) as *mut core::ffi::c_void;
    if hwnd.is_null() {
        log_tray_debug("window reveal skipped: missing hwnd");
        return;
    }

    unsafe {
        let shown = ShowWindow(hwnd, SW_SHOWNORMAL);
        let focused = SetForegroundWindow(hwnd);
        log_tray_debug(format!(
            "window reveal requested: hwnd={}, show={}, focus={}",
            hwnd as isize, shown, focused
        ));
    }
}

#[cfg(not(target_os = "windows"))]
fn reveal_window_for_tray_signal() {}

#[cfg(any(target_os = "windows", target_os = "macos"))]
fn log_tray_debug(message: impl AsRef<str>) {
    use std::io::Write;

    let path = std::env::temp_dir().join("PhaseTrayDebug.log");
    let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    else {
        return;
    };

    let elapsed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs_f32())
        .unwrap_or_default();
    let _ = writeln!(file, "[{elapsed:.3}] {}", message.as_ref());
}

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

fn spawn_theme_preview_fetch(
    tx: Sender<ThemePreviewFetchResult>,
    asset_id: String,
    key: String,
    ctx: Context,
    loader: impl FnOnce() -> Result<Vec<u8>, String> + Send + 'static,
) {
    std::thread::spawn(move || {
        let image = loader().and_then(decode_texture_image);
        let _ = tx.send(ThemePreviewFetchResult {
            asset_id,
            key,
            image,
        });
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
        ui.set_min_width(content_w());
        ui.set_max_width(content_w());

        let frame = egui::Frame::none()
            .fill(phase::surface())
            .stroke(Stroke::new(1.0, phase::line()))
            .rounding(Rounding::same(8.0))
            .inner_margin(Margin::symmetric(14.0, 12.0));

        frame.show(ui, |ui| {
            ui.set_min_width(card_inner());
            ui.set_max_width(card_inner());
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
    ui.horizontal(|ui| {
        let label = ui.label(
            RichText::new(text.to_uppercase())
                .font(FontId::proportional(10.0))
                .color(phase::text_muted()),
        );
        // Trailing hairline fills the rest of the row so sections read as
        // clearly delimited groups at any column width.
        let y = label.rect.center().y + 0.5;
        let x0 = label.rect.right() + 8.0;
        let x1 = ui.max_rect().right();
        if x1 > x0 + 8.0 {
            ui.painter().hline(
                x0..=x1,
                y,
                Stroke::new(1.0, color_with_alpha(phase::line(), 0.7)),
            );
        }
    });
}

/// Equal-column stat grid. One container owns the geometry; every value is
/// rendered inside a clipped/scrolling rect so long paths cannot spill across
/// dividers or out of the card.
fn stat_grid(ui: &mut Ui, items: &[(MiniIcon, &str, String)], columns: usize) {
    stat_grid_impl(ui, items, columns, true);
}

fn stat_grid_flat(ui: &mut Ui, items: &[(MiniIcon, &str, String)], columns: usize) {
    stat_grid_impl(ui, items, columns, false);
}

fn stat_grid_impl(ui: &mut Ui, items: &[(MiniIcon, &str, String)], columns: usize, framed: bool) {
    if items.is_empty() || columns == 0 {
        return;
    }

    let columns = columns.min(items.len()).max(1);
    let rows = (items.len() + columns - 1) / columns;
    let row_height = 48.0;
    let width = ui.available_width().max(1.0);
    let height = row_height * rows as f32;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());

    let painter = ui.painter().clone();
    if framed {
        painter.rect_filled(rect, Rounding::same(8.0), phase::input());
        painter.rect_stroke(rect, Rounding::same(8.0), Stroke::new(1.0, phase::line()));
    }

    let cell_width = rect.width() / columns as f32;
    for col in 1..columns {
        let x = rect.left() + cell_width * col as f32;
        painter.vline(
            x,
            rect.top() + 7.0..=rect.bottom() - 7.0,
            Stroke::new(1.0, color_with_alpha(phase::line(), 0.65)),
        );
    }
    for row in 1..rows {
        let y = rect.top() + row_height * row as f32;
        painter.hline(
            rect.left() + 8.0..=rect.right() - 8.0,
            y,
            Stroke::new(1.0, color_with_alpha(phase::line(), 0.65)),
        );
    }

    for (idx, (icon, label, value)) in items.iter().enumerate() {
        let col = idx % columns;
        let row = idx / columns;
        let cell = Rect::from_min_size(
            Pos2::new(
                rect.left() + cell_width * col as f32,
                rect.top() + row_height * row as f32,
            ),
            Vec2::new(cell_width, row_height),
        )
        .shrink2(Vec2::new(10.0, 7.0));

        let icon_size = 15.0;
        let gap = 8.0;
        let label_font = FontId::proportional(8.0);
        let value_font = FontId::proportional(11.5);
        let label_galley =
            painter.layout_no_wrap(label.to_uppercase(), label_font, phase::text_muted());
        let value_galley = painter.layout_no_wrap(value.clone(), value_font, phase::text());
        let max_text_width = (cell.width() - icon_size - gap).max(1.0);
        let text_width = label_galley
            .size()
            .x
            .max(value_galley.size().x)
            .min(max_text_width);
        let group_width = icon_size + gap + text_width;
        let group_left = cell.center().x - group_width * 0.5;
        let icon_rect = Rect::from_center_size(
            Pos2::new(group_left + icon_size * 0.5, cell.center().y),
            Vec2::splat(icon_size),
        );
        draw_icon_at(&painter, icon_rect, *icon, phase::accent());

        let text_left = group_left + icon_size + gap;
        let text_height = label_galley.size().y + 2.0 + value_galley.size().y;
        let text_top = cell.center().y - text_height * 0.5;
        let label_rect = Rect::from_min_size(
            Pos2::new(text_left, text_top),
            Vec2::new(text_width, label_galley.size().y),
        );
        let label_x = if label_galley.size().x <= text_width {
            label_rect.center().x - label_galley.size().x * 0.5
        } else {
            label_rect.left()
        };
        painter.with_clip_rect(label_rect).galley(
            Pos2::new(label_x, label_rect.top()),
            label_galley,
            phase::text_muted(),
        );

        let value_rect = Rect::from_min_size(
            Pos2::new(text_left, label_rect.bottom() + 2.0),
            Vec2::new(text_width, value_galley.size().y),
        );
        let value_x = if value_galley.size().x <= text_width {
            value_rect.center().x - value_galley.size().x * 0.5
        } else {
            value_rect.left()
        };
        painter.with_clip_rect(value_rect).galley(
            Pos2::new(value_x, value_rect.top()),
            value_galley,
            phase::text(),
        );
    }
}

fn action_grid(ui: &mut Ui, count: usize, mut add_action: impl FnMut(&mut Ui, usize, Vec2)) {
    if count == 0 {
        return;
    }

    let width = ui.available_width().max(1.0);
    let gap = ui.spacing().item_spacing.x;
    let min_button_width = 108.0;
    let columns = if count >= 3 && width >= min_button_width * 3.0 + gap * 2.0 {
        3
    } else if count >= 2 && width >= min_button_width * 2.0 + gap {
        2
    } else {
        1
    };

    let mut index = 0;
    while index < count {
        let remaining = count - index;
        let row_count = remaining.min(columns);
        let row_button_width = if row_count == 1 {
            width
        } else {
            ((width - gap * (row_count.saturating_sub(1)) as f32) / row_count as f32)
                .max(min_button_width)
        };

        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            for offset in 0..row_count {
                add_action(ui, index + offset, Vec2::new(row_button_width, 36.0));
            }
        });

        index += row_count;
        if index < count {
            ui.add_space(8.0);
        }
    }
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
    // Added in the icon-vocabulary pass. Codepoints are from Phosphor v2.1.1,
    // which the bundled assets/Phosphor.ttf matches exactly (verified against the
    // font's cmap). Note the earlier Search/Key glyphs were mislabeled in this
    // font's mapping (E4A6 is actually a trash can, E2A8 a heart) and are now
    // corrected to the real magnifying-glass / key.
    Link,
    ShieldCheck,
    Info,
    Clock,
    Play,
    Pause,
    Trash,
    Palette,
    CloudArrowDown,
    Stack,
    Sparkle,
    Eye,
    Broadcast,
    PlugsConnected,
    FilmStrip,
    MapPin,
}

impl MiniIcon {
    fn glyph(self) -> &'static str {
        match self {
            MiniIcon::Bolt => "\u{E2DE}",           // lightning
            MiniIcon::Check => "\u{E184}",          // check-circle
            MiniIcon::Download => "\u{E20A}",       // download
            MiniIcon::External => "\u{E5DE}",       // arrow-square-out
            MiniIcon::Folder => "\u{E24A}",         // folder-notch
            MiniIcon::Gear => "\u{E272}",           // gear-six
            MiniIcon::Key => "\u{E2D6}",            // key
            MiniIcon::Lock => "\u{E308}",           // lock-simple
            MiniIcon::Refresh => "\u{E094}",        // arrows-clockwise
            MiniIcon::Rocket => "\u{E3FE}",         // rocket-launch
            MiniIcon::Search => "\u{E30C}",         // magnifying-glass
            MiniIcon::User => "\u{E4D6}",           // users
            MiniIcon::Link => "\u{E2E2}",           // link
            MiniIcon::ShieldCheck => "\u{E40C}",    // shield-check
            MiniIcon::Info => "\u{E2CE}",           // info
            MiniIcon::Clock => "\u{E19A}",          // clock
            MiniIcon::Play => "\u{E3D0}",           // play
            MiniIcon::Pause => "\u{E39E}",          // pause
            MiniIcon::Trash => "\u{E4A6}",          // trash
            MiniIcon::Palette => "\u{E6C8}",        // palette
            MiniIcon::CloudArrowDown => "\u{E1AC}", // cloud-arrow-down
            MiniIcon::Stack => "\u{E466}",          // stack
            MiniIcon::Sparkle => "\u{E6A2}",        // sparkle
            MiniIcon::Eye => "\u{E220}",            // eye
            MiniIcon::Broadcast => "\u{E0F2}",      // broadcast
            MiniIcon::PlugsConnected => "\u{EB5A}", // plugs-connected
            MiniIcon::FilmStrip => "\u{E792}",      // film-strip
            MiniIcon::MapPin => "\u{E316}",         // map-pin
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
        // Unreachable hand-drawn fallback: the font path above always returns
        // first. Newer icons have no hand-drawn version, so catch-all here.
        _ => {}
    }
}

fn compact_info_row(ui: &mut Ui, label: &str, value: &str, width: f32) {
    ui.horizontal(|ui| {
        ui.set_width(width);
        let label_width = 66.0_f32.min(width * 0.38);
        let value_width = (width - label_width - 8.0).max(72.0);
        ui.add_sized(
            Vec2::new(label_width, 18.0),
            egui::Label::new(
                RichText::new(label)
                    .font(FontId::proportional(12.0))
                    .color(phase::text_muted()),
            )
            .wrap(false),
        );
        scrolling_label(
            ui,
            value,
            value_width,
            FontId::proportional(12.5),
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

fn diagnostics_waiting_card(ui: &mut Ui) {
    egui::Frame::none()
        .fill(phase::surface())
        .stroke(Stroke::new(1.0, phase::line()))
        .rounding(Rounding::same(8.0))
        .inner_margin(Margin::symmetric(14.0, 12.0))
        .show(ui, |ui| {
            ui.set_width((ui.available_width() - 8.0).max(300.0));
            ui.horizontal(|ui| {
                draw_icon(ui, MiniIcon::Refresh, Vec2::splat(18.0), phase::blue());
                ui.add_space(8.0);
                ui.vertical(|ui| {
                    ui.label(
                        RichText::new("Checking connection")
                            .font(FontId::proportional(13.5))
                            .strong()
                            .color(phase::text()),
                    );
                    ui.label(
                        RichText::new("This should only take a few seconds.")
                            .font(FontId::proportional(11.0))
                            .color(phase::text_muted()),
                    );
                });
            });
        });
}

fn diagnostics_check_card(ui: &mut Ui, check: &diagnostics::DiagnosticCheck) {
    let color = match check.status {
        diagnostics::DiagnosticStatus::Good => phase::green(),
        diagnostics::DiagnosticStatus::Warning => phase::warning(),
        diagnostics::DiagnosticStatus::Problem => phase::red(),
    };
    let icon = match check.status {
        diagnostics::DiagnosticStatus::Good => MiniIcon::Check,
        diagnostics::DiagnosticStatus::Warning | diagnostics::DiagnosticStatus::Problem => {
            MiniIcon::Gear
        }
    };

    egui::Frame::none()
        .fill(phase::surface())
        .stroke(Stroke::new(1.0, phase::line()))
        .rounding(Rounding::same(8.0))
        .inner_margin(Margin::symmetric(14.0, 12.0))
        .show(ui, |ui| {
            let width = (ui.available_width() - 8.0).max(300.0);
            ui.set_width(width);
            ui.horizontal(|ui| {
                draw_icon(ui, icon, Vec2::splat(20.0), color);
                ui.add_space(8.0);
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        status_pill(ui, check.status.label(), color);
                        ui.add_space(6.0);
                        let elapsed = check
                            .elapsed_ms
                            .map(|value| format!("{value} ms"))
                            .unwrap_or_default();
                        scrolling_label(
                            ui,
                            &elapsed,
                            80.0,
                            FontId::proportional(10.5),
                            phase::text_muted(),
                        );
                    });
                    ui.add_space(3.0);
                    scrolling_label(
                        ui,
                        &check.title,
                        width - 42.0,
                        FontId::proportional(14.0),
                        phase::text(),
                    );
                    ui.add_space(3.0);
                    ui.label(
                        RichText::new(&check.detail)
                            .font(FontId::proportional(11.5))
                            .color(phase::text_secondary()),
                    );
                    if !check.next_step.trim().is_empty() && check.next_step != "No action needed."
                    {
                        ui.add_space(6.0);
                        ui.label(
                            RichText::new(format!("Try: {}", check.next_step))
                                .font(FontId::proportional(11.0))
                                .color(color),
                        );
                    }
                });
            });
        });
}

fn small_number_field(ui: &mut Ui, label: &str, value: &mut String, hint: &str, width: f32) {
    ui.vertical(|ui| {
        ui.set_width(width);
        ui.label(
            RichText::new(label)
                .font(FontId::proportional(10.0))
                .color(phase::text_muted()),
        );
        ui.add(
            egui::TextEdit::singleline(value)
                .desired_width(width)
                .hint_text(hint),
        );
    });
}

fn parse_f64_or(text: &str, fallback: f64) -> f64 {
    text.trim().parse::<f64>().unwrap_or(fallback)
}

fn parse_i64_or(text: &str, fallback: i64) -> i64 {
    text.trim().parse::<i64>().unwrap_or(fallback)
}

fn format_seconds(value: f64) -> String {
    if value.fract().abs() < 0.0005 {
        format!("{value:.0}")
    } else {
        format!("{value:.3}")
    }
}

fn reference_summary(value: &serde_json::Value) -> Option<String> {
    let object = value.as_object()?;
    let source = object
        .get("Source")
        .or_else(|| object.get("source"))
        .and_then(|value| value.as_str())
        .unwrap_or("");
    let title = object
        .get("Title")
        .or_else(|| object.get("title"))
        .and_then(|value| value.as_str())
        .unwrap_or("Video Reference");
    if source.trim().is_empty() {
        return Some("No video reference linked.".to_owned());
    }
    Some(format!("{title}: {source}"))
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

#[derive(Default)]
struct PluginSettingsResetSummary {
    files_changed: usize,
    removed_keys: usize,
}

#[derive(Default)]
struct PluginSettingsInventory {
    files_with_phase_keys: usize,
    theme_keys: usize,
    keybind_keys: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PluginSettingsCategory {
    Themes,
    Keybinds,
}

fn reset_phase_plugin_settings(
    categories: &[PluginSettingsCategory],
) -> Result<PluginSettingsResetSummary, String> {
    let paths = discover_roblox_plugin_settings_files();
    let mut summary = PluginSettingsResetSummary::default();

    for path in paths {
        let text = std::fs::read_to_string(&path).map_err(|error| {
            format!(
                "Could not read Roblox plugin settings at {}: {error}",
                compact_path(&path, 44)
            )
        })?;
        let mut value = serde_json::from_str::<serde_json::Value>(&text).map_err(|error| {
            format!(
                "Could not parse Roblox plugin settings at {}: {error}",
                compact_path(&path, 44)
            )
        })?;

        let Some(object) = value.as_object_mut() else {
            continue;
        };

        let keys = object
            .keys()
            .filter(|key| phase_setting_matches_any_category(key, categories))
            .cloned()
            .collect::<Vec<_>>();
        if keys.is_empty() {
            continue;
        }

        let mut selected = serde_json::Map::new();
        for key in &keys {
            if let Some(value) = object.get(key) {
                selected.insert(key.clone(), value.clone());
            }
        }

        backup_roblox_settings_file(&path, &text, &selected)?;
        let removed = keys.len();
        for key in keys {
            object.remove(&key);
        }

        let updated = serde_json::to_string_pretty(&value).map_err(|error| {
            format!(
                "Could not serialize Roblox plugin settings at {}: {error}",
                compact_path(&path, 44)
            )
        })?;
        std::fs::write(&path, updated).map_err(|error| {
            format!(
                "Could not update Roblox plugin settings at {}: {error}",
                compact_path(&path, 44)
            )
        })?;
        summary.files_changed += 1;
        summary.removed_keys += removed;
    }

    Ok(summary)
}

fn phase_plugin_settings_inventory() -> PluginSettingsInventory {
    let mut inventory = PluginSettingsInventory::default();

    for path in discover_roblox_plugin_settings_files() {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
            continue;
        };
        let Some(object) = value.as_object() else {
            continue;
        };

        let mut file_has_phase_keys = false;
        for key in object.keys() {
            if !key.starts_with("PhaseAnimator") {
                continue;
            }
            file_has_phase_keys = true;
            if phase_setting_matches_category(key, PluginSettingsCategory::Themes) {
                inventory.theme_keys += 1;
            }
            if phase_setting_matches_category(key, PluginSettingsCategory::Keybinds) {
                inventory.keybind_keys += 1;
            }
        }
        if file_has_phase_keys {
            inventory.files_with_phase_keys += 1;
        }
    }

    inventory
}

fn phase_setting_matches_any_category(key: &str, categories: &[PluginSettingsCategory]) -> bool {
    categories
        .iter()
        .any(|category| phase_setting_matches_category(key, *category))
}

fn phase_setting_matches_category(key: &str, category: PluginSettingsCategory) -> bool {
    match category {
        PluginSettingsCategory::Themes => matches!(
            key,
            "PhaseAnimatorThemeV2"
                | "PhaseAnimatorTheme"
                | "PhaseAnimatorThemeName"
                | "PhaseAnimatorThemePresets"
                | "PhaseAnimatorFontProfile"
                | "PhaseAnimatorGlassSettings"
                | "PhaseAnimatorMotionSettings"
                | "PhaseAnimatorTimelineSettings"
                | "PhaseAnimator_ViewTransitionProfile_v1"
        ),
        PluginSettingsCategory::Keybinds => key == "PhaseAnimator_Keybinds_V1",
    }
}

fn discover_roblox_plugin_settings_files() -> Vec<PathBuf> {
    let mut paths = Vec::new();

    #[cfg(target_os = "windows")]
    {
        if let Ok(local_app_data) = std::env::var("LOCALAPPDATA") {
            collect_roblox_plugin_settings_files(
                &PathBuf::from(local_app_data).join("Roblox"),
                &mut paths,
            );
        }
    }

    #[cfg(target_os = "macos")]
    {
        if let Some(data_dir) = dirs::data_dir() {
            collect_roblox_plugin_settings_files(&data_dir.join("Roblox"), &mut paths);
        }
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        if let Some(data_dir) = dirs::data_dir() {
            collect_roblox_plugin_settings_files(&data_dir.join("Roblox"), &mut paths);
        }
    }

    paths.sort();
    paths.dedup();
    paths
}

fn collect_roblox_plugin_settings_files(root: &Path, paths: &mut Vec<PathBuf>) {
    let Ok(users) = std::fs::read_dir(root) else {
        return;
    };

    for user in users.flatten() {
        let Ok(file_type) = user.file_type() else {
            continue;
        };
        if !file_type.is_dir() {
            continue;
        }

        let user_path = user.path();
        let user_name = user.file_name().to_string_lossy().to_string();
        if !user_name.chars().all(|ch| ch.is_ascii_digit()) {
            continue;
        }

        let installed_plugins = user_path.join("InstalledPlugins");
        let Ok(plugins) = std::fs::read_dir(installed_plugins) else {
            continue;
        };
        for plugin_dir in plugins.flatten() {
            let settings_path = plugin_dir.path().join("settings.json");
            if settings_path.is_file() && settings_file_mentions_phase_animator(&settings_path) {
                paths.push(settings_path);
            }
        }
    }
}

fn settings_file_mentions_phase_animator(path: &Path) -> bool {
    std::fs::read_to_string(path)
        .map(|text| text.contains("\"PhaseAnimator"))
        .unwrap_or(false)
}

fn backup_roblox_settings_file(
    path: &Path,
    contents: &str,
    selected: &serde_json::Map<String, serde_json::Value>,
) -> Result<(), String> {
    let timestamp = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let full_backup_path =
        path.with_file_name(format!("settings.phase-full-backup-{timestamp}.json"));
    std::fs::write(&full_backup_path, contents).map_err(|error| {
        format!(
            "Could not back up Roblox plugin settings to {}: {error}",
            compact_path(&full_backup_path, 44)
        )
    })?;

    let selected_backup_path =
        path.with_file_name(format!("settings.phase-selected-backup-{timestamp}.json"));
    let backup = json!({
        "sourcePath": path.to_string_lossy(),
        "backedUpAtUnix": timestamp,
        "settings": selected,
    });
    let selected_text = serde_json::to_string_pretty(&backup)
        .map_err(|error| format!("Could not prepare selected settings backup: {error}"))?;
    std::fs::write(&selected_backup_path, selected_text).map_err(|error| {
        format!(
            "Could not back up selected Phase settings to {}: {error}",
            compact_path(&selected_backup_path, 44)
        )
    })?;

    Ok(())
}

fn plural(count: usize) -> &'static str {
    if count == 1 { "" } else { "s" }
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
    if let Some(image_id) = parse_theme_background_image_id_from_json(theme_code) {
        return Some(image_id);
    }

    theme_code.split('|').find_map(normalize_roblox_image_id)
}

fn parse_theme_background_image_id_from_json(theme_code: &str) -> Option<String> {
    let payload = theme_code.split('|').nth(2)?.trim();
    let value = serde_json::from_str::<serde_json::Value>(payload).ok()?;
    value
        .get("background")
        .and_then(|background| background.get("imageId"))
        .and_then(serde_json::Value::as_str)
        .and_then(normalize_roblox_image_id)
}

fn normalize_roblox_image_id(value: &str) -> Option<String> {
    let value = value.trim();
    let value = value.strip_prefix('i').unwrap_or(value).trim();
    let value = value.strip_prefix("rbxassetid://").unwrap_or(value).trim();
    if value.chars().all(|ch| ch.is_ascii_digit()) && !value.is_empty() {
        Some(value.to_owned())
    } else {
        None
    }
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

fn ease_out_cubic(t: f32) -> f32 {
    1.0 - (1.0 - t.clamp(0.0, 1.0)).powi(3)
}

fn ease_out_back(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0) - 1.0;
    1.0 + 2.70158 * t.powi(3) + 1.70158 * t.powi(2)
}

fn color_with_alpha(color: Color32, alpha: f32) -> Color32 {
    let alpha = (color.a() as f32 * alpha.clamp(0.0, 1.0)).round() as u8;
    Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), alpha)
}

fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let mix = |left: u8, right: u8| left as f32 + (right as f32 - left as f32) * t;
    Color32::from_rgba_unmultiplied(
        mix(a.r(), b.r()).round() as u8,
        mix(a.g(), b.g()).round() as u8,
        mix(a.b(), b.b()).round() as u8,
        mix(a.a(), b.a()).round() as u8,
    )
}

fn icon_button_text(
    icon: MiniIcon,
    text: &str,
    icon_color: Option<Color32>,
    text_size: f32,
    icon_size: f32,
    gap: f32,
) -> WidgetText {
    let mut job = egui::text::LayoutJob::default();
    job.break_on_newline = false;
    job.first_row_min_height = icon_size.max(text_size);
    job.append(
        icon.glyph(),
        0.0,
        TextFormat {
            font_id: FontId::new(icon_size, FontFamily::Name(PHOSPHOR_FONT.into())),
            color: icon_color.unwrap_or(Color32::PLACEHOLDER),
            valign: Align::Center,
            ..Default::default()
        },
    );
    job.append(
        text,
        gap,
        TextFormat {
            font_id: FontId::proportional(text_size),
            color: Color32::PLACEHOLDER,
            valign: Align::Center,
            ..Default::default()
        },
    );
    WidgetText::from(job)
}

#[derive(Clone, Copy)]
struct PhaseButtonVisuals {
    fill: Color32,
    hover_fill: Color32,
    active_fill: Color32,
    stroke: Stroke,
    hover_stroke: Stroke,
    active_stroke: Stroke,
    text_color: Color32,
}

fn apply_button_visuals(ui: &mut Ui, visuals: PhaseButtonVisuals) {
    let widgets = &mut ui.style_mut().visuals.widgets;
    widgets.noninteractive.weak_bg_fill = visuals.fill;
    widgets.noninteractive.bg_stroke = visuals.stroke;
    widgets.noninteractive.fg_stroke = Stroke::new(1.0, visuals.text_color);

    widgets.inactive.weak_bg_fill = visuals.fill;
    widgets.inactive.bg_stroke = visuals.stroke;
    widgets.inactive.fg_stroke = Stroke::new(1.0, visuals.text_color);

    widgets.hovered.weak_bg_fill = visuals.hover_fill;
    widgets.hovered.bg_stroke = visuals.hover_stroke;
    widgets.hovered.fg_stroke = Stroke::new(1.0, visuals.text_color);

    widgets.active.weak_bg_fill = visuals.active_fill;
    widgets.active.bg_stroke = visuals.active_stroke;
    widgets.active.fg_stroke = Stroke::new(1.0, visuals.text_color);
}

fn primary_button(ui: &mut Ui, icon: MiniIcon, text: &str, size: Vec2) -> egui::Response {
    let opacity = if ui.is_enabled() { 1.0 } else { 0.45 };
    let text_color = color_with_alpha(phase::text_on_accent(), opacity);
    let visuals = PhaseButtonVisuals {
        fill: color_with_alpha(phase::accent(), opacity),
        hover_fill: color_with_alpha(phase::accent_hover(), opacity),
        active_fill: color_with_alpha(phase::accent_dim(), opacity),
        stroke: Stroke::new(
            1.0,
            color_with_alpha(phase::text_on_accent(), 0.85 * opacity),
        ),
        hover_stroke: Stroke::new(1.0, color_with_alpha(phase::text_on_accent(), opacity)),
        active_stroke: Stroke::new(
            1.0,
            color_with_alpha(phase::text_on_accent(), 0.75 * opacity),
        ),
        text_color,
    };

    ui.scope(|ui| {
        apply_button_visuals(ui, visuals);
        ui.add_sized(
            size,
            Button::new(icon_button_text(icon, text, None, 16.0, 19.0, 10.0))
                .frame(true)
                .min_size(size)
                .rounding(Rounding::same(6.0))
                .wrap(false),
        )
    })
    .inner
}

fn secondary_button(ui: &mut Ui, icon: MiniIcon, text: &str, size: Vec2) -> egui::Response {
    let opacity = if ui.is_enabled() { 1.0 } else { 0.45 };
    let visuals = PhaseButtonVisuals {
        fill: color_with_alpha(phase::input(), opacity),
        hover_fill: color_with_alpha(phase::surface_hover(), opacity),
        active_fill: color_with_alpha(phase::surface_active(), opacity),
        stroke: Stroke::new(1.0, color_with_alpha(phase::line(), opacity)),
        hover_stroke: Stroke::new(1.0, color_with_alpha(phase::accent(), opacity)),
        active_stroke: Stroke::new(1.0, color_with_alpha(phase::line(), opacity)),
        text_color: color_with_alpha(phase::text_secondary(), opacity),
    };

    ui.scope(|ui| {
        apply_button_visuals(ui, visuals);
        ui.add_sized(
            size,
            Button::new(icon_button_text(icon, text, None, 14.0, 15.0, 8.0))
                .frame(true)
                .min_size(size)
                .rounding(Rounding::same(6.0))
                .wrap(false),
        )
    })
    .inner
}

fn status_action(ui: &mut Ui, icon: MiniIcon, text: &str, size: Vec2) {
    let visuals = PhaseButtonVisuals {
        fill: phase::surface_active(),
        hover_fill: phase::surface_active(),
        active_fill: phase::surface_active(),
        stroke: Stroke::new(1.0, phase::line()),
        hover_stroke: Stroke::new(1.0, phase::line()),
        active_stroke: Stroke::new(1.0, phase::line()),
        text_color: phase::text_secondary(),
    };

    ui.scope(|ui| {
        apply_button_visuals(ui, visuals);
        let _ = ui.add_sized(
            size,
            Button::new(icon_button_text(
                icon,
                text,
                Some(phase::green()),
                14.0,
                15.0,
                8.0,
            ))
            .frame(true)
            .min_size(size)
            .rounding(Rounding::same(6.0))
            .sense(Sense::hover())
            .wrap(false),
        );
    });
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
        if let Some(palette) = palette_from_theme_json(code) {
            return Some(palette);
        }

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

    fn palette_from_theme_json(code: &str) -> Option<Palette> {
        let payload = code.split('|').nth(2)?.trim();
        if !payload.starts_with('{') {
            return None;
        }

        let value = serde_json::from_str::<serde_json::Value>(payload).ok()?;
        let palette = value.get("palette")?.as_object()?;

        Some(Palette {
            background: color_field(palette, "Background")?,
            surface: color_field(palette, "PanelBackground")
                .or_else(|| color_field(palette, "Surface"))?,
            surface_hover: color_field(palette, "SurfaceHover")?,
            surface_active: color_field(palette, "SurfaceActive")?,
            input: color_field(palette, "Input")?,
            line: color_field(palette, "Line")
                .or_else(|| color_field(palette, "Separator"))
                .or_else(|| color_field(palette, "PanelBorder"))?,
            accent: color_field(palette, "Accent")?,
            accent_hover: color_field(palette, "AccentHover")?,
            accent_dim: color_field(palette, "AccentDim")
                .or_else(|| color_field(palette, "AccentMuted"))?,
            blue: color_field(palette, "Blue")?,
            green: color_field(palette, "Green")?,
            red: color_field(palette, "Red")?,
            warning: color_field(palette, "Warning")?,
            text: color_field(palette, "TextPrimary").or_else(|| color_field(palette, "Text"))?,
            text_secondary: color_field(palette, "TextSecondary")?,
            text_muted: color_field(palette, "TextMuted")
                .or_else(|| color_field(palette, "TextDim"))?,
            text_on_accent: color_field(palette, "TextOnAccent")?,
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

    fn color_field(
        palette: &serde_json::Map<String, serde_json::Value>,
        field: &str,
    ) -> Option<Color32> {
        palette.get(field)?.as_str().and_then(hex_color)
    }

    pub fn hex_color(value: &str) -> Option<Color32> {
        let value = value.trim().trim_start_matches('#');
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

#[cfg(test)]
mod tests {
    use super::*;

    const PA2_THEME_CODE: &str = r##"PA2|Violet Nebula|{"schemaVersion":2,"name":"Violet Nebula","palette":{"Background":"#080713","PanelBackground":"#21153D","SurfaceHover":"#3A345A","SurfaceActive":"#433B63","Input":"#0D0818","Line":"#5A348A","Accent":"#B985E8","AccentHover":"#E7C7FF","AccentDim":"#7442A8","Blue":"#85C7FF","Green":"#4BC67A","Red":"#E04E4E","Warning":"#E4A940","TextPrimary":"#F3EEFF","TextSecondary":"#D8B6F2","TextMuted":"#A36BD2","TextOnAccent":"#05040A"},"background":{"imageId":"rbxassetid://1161841954"}}"##;

    #[test]
    fn parses_pa2_theme_json_palette() {
        assert!(phase::palette_from_theme_code(PA2_THEME_CODE).is_some());
    }

    #[test]
    fn parses_pa2_theme_background_image_id() {
        assert_eq!(
            parse_theme_background_image_id(PA2_THEME_CODE).as_deref(),
            Some("1161841954")
        );
    }

    #[test]
    fn parses_legacy_theme_background_image_id() {
        assert_eq!(
            parse_theme_background_image_id("PA1|Theme|000000.111111|i96046223266953").as_deref(),
            Some("96046223266953")
        );
    }
}
