//! Companion shell: a sidebar with four pages (Home, Reference, Account,
//! Settings) drawn with the Phase design kit.
use super::kit;
use super::*;
use kit::Tone;

#[derive(Default)]
pub(super) struct ShellState {
    pub(super) install_location: bool,
    reference_open: bool,
    reference_error: Option<String>,
    reference_timing: bool,
    install_confirmation: Option<ReleaseChannel>,
    shown_page: Option<Page>,
    page_shown_at: Option<Instant>,
    last_phase: Option<InstallPhase>,
    completed_at: Option<Instant>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NextStep {
    Wait,
    Location,
    Connect,
    RefreshAccess,
    Check,
    Install,
    Ready,
    Unavailable,
}

fn next_step(
    busy: bool,
    location: bool,
    access: bool,
    linked: bool,
    current: bool,
    release: bool,
    available: bool,
) -> NextStep {
    if busy {
        NextStep::Wait
    } else if current {
        NextStep::Ready
    } else if !location {
        NextStep::Location
    } else if !access {
        if linked {
            NextStep::RefreshAccess
        } else {
            NextStep::Connect
        }
    } else if !release {
        NextStep::Check
    } else if !available {
        NextStep::Unavailable
    } else {
        NextStep::Install
    }
}

const WIDE_RAIL: f32 = 224.0;
const COMPACT_RAIL: f32 = 76.0;
const CONTENT_MAX: f32 = 700.0;

impl PhaseInstallerApp {
    pub(super) fn render_companion_root(&mut self, ui: &mut Ui) {
        if ui.input(|i| i.raw.dropped_files.iter().any(|f| f.path.is_some())) {
            self.open_page(Page::Reference);
        }
        kit::track_input_mode(ui.ctx());
        kit::set_surface_tint(self.theme_background.as_ref().and(self.theme_art_tint));

        // Remember the moment an install finishes for the success animation.
        let state = &mut self.shell;
        if self.phase == InstallPhase::Complete
            && state.last_phase == Some(InstallPhase::Installing)
        {
            state.completed_at = Some(Instant::now());
        }
        state.last_phase = Some(self.phase);
        if self.screenshot_path.is_some()
            && std::env::var("PHASE_UI_DIALOG").ok().as_deref() == Some("success")
        {
            state.completed_at = Instant::now().checked_sub(std::time::Duration::from_millis(900));
        }
        if self.screenshot_path.is_some()
            && std::env::var("PHASE_UI_DIALOG").ok().as_deref() == Some("timing")
        {
            state.reference_timing = true;
        }

        let modal_open = self.shell.install_confirmation.is_some() || self.shell.install_location;
        if !modal_open {
            let pages = [Page::Home, Page::Reference, Page::Account, Page::Settings];
            let keys = [
                egui::Key::Num1,
                egui::Key::Num2,
                egui::Key::Num3,
                egui::Key::Num4,
            ];
            let chosen = ui.input(|i| {
                i.modifiers
                    .command
                    .then(|| keys.iter().position(|k| i.key_pressed(*k)))
                    .flatten()
            });
            if let Some(index) = chosen {
                self.open_page(pages[index]);
            }
        }
        self.paint_theme_background(ui);
        let full = ui.max_rect();
        if !self.has_theme_background_art() {
            paint_ambient(ui.painter(), full);
        }

        let compact = full.width() < 720.0;
        let rail = if compact { COMPACT_RAIL } else { WIDE_RAIL };
        #[cfg(target_os = "macos")]
        let top = 28.0;
        #[cfg(not(target_os = "macos"))]
        let top = 0.0;

        let rail_rect = Rect::from_min_max(full.min, Pos2::new(full.left() + rail, full.bottom()));
        ui.painter().rect_filled(
            rail_rect,
            Rounding::ZERO,
            kit::tinted(color_with_alpha(
                lerp_color(phase::background(), phase::surface(), 0.35),
                if backdrop_active() { 0.45 } else { 0.72 },
            )),
        );
        ui.painter().vline(
            rail_rect.right() - 0.5,
            rail_rect.y_range(),
            Stroke::new(1.0, color_with_alpha(phase::line(), 0.28)),
        );
        let mut rail_ui = ui.child_ui(
            Rect::from_min_max(
                rail_rect.min + Vec2::new(14.0, 20.0 + top),
                rail_rect.max - Vec2::new(14.0, 16.0),
            ),
            egui::Layout::top_down(Align::LEFT),
        );
        rail_ui.set_enabled(!modal_open);
        self.sidebar(&mut rail_ui, compact);

        let content_rect =
            Rect::from_min_max(Pos2::new(rail_rect.right(), full.top() + top), full.max);
        let mut content_ui = ui.child_ui(content_rect, egui::Layout::top_down(Align::LEFT));
        content_ui.set_enabled(!modal_open);
        self.page_body(&mut content_ui, compact);

        self.overlays(ui.ctx());
    }

    fn page_body(&mut self, ui: &mut Ui, compact: bool) {
        // Pages fade in and spring up into place.
        let offset_id = egui::Id::new("page-rise");
        let state = &mut self.shell;
        if state.shown_page != Some(self.page) {
            let first = state.shown_page.is_none();
            state.shown_page = Some(self.page);
            state.page_shown_at = (self.screenshot_path.is_none()).then(Instant::now);
            if !first && self.screenshot_path.is_none() {
                kit::spring_kick(ui.ctx(), offset_id, 18.0);
            }
        }
        let rise = kit::gentle_spring(ui.ctx(), offset_id, 0.0);
        let t = state
            .page_shown_at
            .map(|at| ease_out_cubic(at.elapsed().as_secs_f32() / 0.28))
            .unwrap_or(1.0);
        if t < 1.0 {
            ui.ctx().request_repaint();
        }

        let pad = if compact { 22.0 } else { 36.0 };
        let mut scroll = egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .id_source(("companion-page", self.page));
        if self.reset_body_scroll {
            scroll = scroll.vertical_scroll_offset(0.0);
        }
        scroll.show(ui, |ui| {
            egui::Frame::none()
                .inner_margin(Margin {
                    left: pad,
                    right: pad,
                    top: 24.0,
                    bottom: 30.0,
                })
                .show(ui, |ui| {
                    ui.set_opacity(t);
                    ui.add_space((6.0 + rise).max(0.0));
                    let width = ui.available_width().min(CONTENT_MAX);
                    ui.set_width(width);
                    ui.spacing_mut().item_spacing = Vec2::new(10.0, 8.0);
                    match self.page {
                        Page::Reference => self.reference_page(ui),
                        Page::Account => self.account_page(ui),
                        Page::Settings => self.settings_page(ui),
                        Page::Home => self.home_page(ui),
                    }
                });
        });
        self.reset_body_scroll = false;
    }

    // -----------------------------------------------------------------------
    // Sidebar

    fn sidebar(&mut self, ui: &mut Ui, compact: bool) {
        ui.add_space(4.0);
        ui.spacing_mut().item_spacing.y = 4.0;
        let highlight = ui.painter().add(egui::Shape::Noop);
        let origin = ui.min_rect().top();
        let mut selected_rect = None;
        for (tab, icon, label) in [
            (Page::Home, Icon::House, "Home"),
            (Page::Reference, Icon::MonitorPlay, "Reference"),
            (Page::Account, Icon::UserCircle, "Account"),
            (Page::Settings, Icon::Sliders, "Settings"),
        ] {
            let selected = self.page == tab;
            let response = kit::nav_item(ui, icon, label, selected, compact);
            if selected {
                selected_rect = Some(response.rect);
            }
            if response.clicked() {
                self.open_page(tab);
            }
        }
        if let Some(rect) = selected_rect {
            // One pill that slides between items, measured from the rail top
            // so window resizes don't animate it.
            let y = origin + kit::spring(ui.ctx(), egui::Id::new("nav-pill"), rect.top() - origin);
            ui.painter().set(
                highlight,
                kit::nav_highlight(Rect::from_min_size(Pos2::new(rect.left(), y), rect.size())),
            );
        }

        ui.with_layout(egui::Layout::bottom_up(Align::LEFT), |ui| {
            let name = self.account_summary();
            let detail = if self.has_early_access() {
                "Early Access"
            } else if self.activation.as_ref().is_some_and(|a| a.ok && a.active) {
                "Install access"
            } else if self.plugin_token.is_some() {
                "Phase account"
            } else {
                "Tap to connect"
            };
            let texture = self.phase_avatar.as_ref().or(self.roblox_avatar.as_ref());
            if kit::account_chip(ui, texture, &name, detail, compact).clicked() {
                self.open_page(Page::Account);
            }
            ui.add_space(10.0);
            if let Some(update) = &self.app_update {
                let label = format!("Companion {} is ready", update.version);
                if compact {
                    if kit::icon_button(ui, Icon::DownloadSimple, &label, 36.0, true).clicked() {
                        self.open_page(Page::Settings);
                    }
                } else if kit::quiet_button(ui, Some(Icon::DownloadSimple), "Update companion")
                    .on_hover_text(label)
                    .clicked()
                {
                    self.open_page(Page::Settings);
                }
                ui.add_space(4.0);
            }
            studio_status(ui, self.video_bridge_connected, compact);
        });
    }

    fn has_early_access(&self) -> bool {
        self.plugin_token.is_some()
            && self
                .build_access
                .as_ref()
                .is_some_and(|access| access.is_early_access())
    }

    fn account_busy(&self) -> bool {
        self.link_rx.is_some()
            || self.link_status_rx.is_some()
            || self.account_refresh_rx.is_some()
            || self.roblox_oauth_rx.is_some()
            || self.roblox_oauth_status_rx.is_some()
    }

    fn has_access(&self) -> bool {
        self.activation.as_ref().is_some_and(|a| a.ok && a.active)
    }

    // -----------------------------------------------------------------------
    // Home

    fn home_page(&mut self, ui: &mut Ui) {
        let account_busy = self.account_busy();
        let step = next_step(
            self.is_busy() || account_busy,
            self.selected_folder.is_some(),
            self.has_access(),
            self.plugin_token.is_some(),
            self.local_release_current,
            self.release.is_some(),
            self.release
                .as_ref()
                .is_some_and(|r| !r.blocked && r.download_available),
        );

        self.hero(ui, step, account_busy);
        ui.add_space(14.0);

        if let Some(error) = self
            .release_error
            .clone()
            .or_else(|| self.activation_error.clone())
        {
            kit::banner(ui, Tone::Danger, Icon::Warning, &error);
            ui.add_space(14.0);
        }

        self.overview_tiles(ui);

        if self.has_early_access() {
            ui.add_space(18.0);
            self.early_access_card(ui, account_busy);
        }

        if !self.activity.is_empty() {
            ui.add_space(18.0);
            kit::section_label(ui, "Recent activity");
            kit::card(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 10.0;
                for line in self.activity.iter().rev().take(4) {
                    ui.horizontal(|ui| {
                        let (rect, _) =
                            ui.allocate_exact_size(Vec2::new(10.0, 18.0), Sense::hover());
                        ui.painter()
                            .circle_filled(rect.center(), 3.5, kit::readable(line.color));
                        ui.add(
                            egui::Label::new(
                                RichText::new(&line.text)
                                    .size(13.0)
                                    .color(phase::text_secondary()),
                            )
                            .wrap(true),
                        );
                    });
                }
            });
        }
    }

    fn hero(&mut self, ui: &mut Ui, step: NextStep, account_busy: bool) {
        let installed = self.has_local_phase_install();
        let version = self
            .release
            .as_ref()
            .map(|r| r.latest_version.trim_start_matches(['v', 'V']).to_owned())
            .unwrap_or_default();
        let (headline, detail) = match step {
            NextStep::Wait => (
                if self.account_refresh_rx.is_some() {
                    "Checking your access".to_owned()
                } else if account_busy {
                    "Waiting for your browser".to_owned()
                } else {
                    phase_text(self.phase).trim_end_matches("...").to_owned()
                },
                if account_busy {
                    "Finish connecting in your browser. This updates automatically.".to_owned()
                } else {
                    String::new()
                },
            ),
            NextStep::Location => (
                "Choose where to install".to_owned(),
                "Select your Roblox Studio plugins folder.".to_owned(),
            ),
            NextStep::Connect => (
                "Connect your account".to_owned(),
                "A Phase account is required to install the plugin.".to_owned(),
            ),
            NextStep::RefreshAccess => (
                "Verify your access".to_owned(),
                "Your account is connected. Refresh it to confirm install access.".to_owned(),
            ),
            NextStep::Check => (
                "Check for updates".to_owned(),
                "Look up the latest plugin release.".to_owned(),
            ),
            NextStep::Install if installed => (
                format!("Version {version} is available"),
                "Update to get the latest changes.".to_owned(),
            ),
            NextStep::Install => (
                "Install Phase Animator".to_owned(),
                format!("Version {version} will be installed into Roblox Studio."),
            ),
            NextStep::Ready => (
                "Phase is up to date".to_owned(),
                if version.is_empty() {
                    "The latest version is installed.".to_owned()
                } else {
                    format!("Version {version} is installed.")
                },
            ),
            NextStep::Unavailable => (
                "No release available".to_owned(),
                "Check again later.".to_owned(),
            ),
        };

        let (primary, icon) = match step {
            NextStep::Location => (Some("Choose location"), Icon::FolderOpen),
            NextStep::Connect => (Some("Connect account"), Icon::UserCircle),
            NextStep::RefreshAccess => (Some("Verify access"), Icon::ShieldCheck),
            NextStep::Check => (Some("Check for updates"), Icon::Refresh),
            NextStep::Install => (
                Some(if self.phase == InstallPhase::Error {
                    "Retry install"
                } else if installed {
                    if self.has_early_access() {
                        "Update stable build"
                    } else {
                        "Update"
                    }
                } else {
                    "Install"
                }),
                Icon::DownloadSimple,
            ),
            _ => (None, Icon::Refresh),
        };
        let show_check = matches!(
            step,
            NextStep::Ready | NextStep::Unavailable | NextStep::Install
        );

        let mut action_clicked = false;
        let mut check_clicked = false;
        let loading = self.phase == InstallPhase::Checking && self.release.is_none();
        let fade = kit::change_fade(
            ui.ctx(),
            egui::Id::new("hero-copy"),
            kit::hash_of(&(&headline, loading)),
        );
        let success = self
            .shell
            .completed_at
            .map(|at| at.elapsed().as_secs_f32())
            .filter(|t| *t < SUCCESS_SECS);
        kit::card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 16.0;
                let (slot, _) = ui.allocate_exact_size(Vec2::splat(48.0), Sense::hover());
                let logo_alpha = match success {
                    Some(t) => {
                        paint_success(ui.painter(), slot, t);
                        ui.ctx().request_repaint();
                        ((t - (SUCCESS_SECS - 0.4)) / 0.4).clamp(0.0, 1.0)
                    }
                    None => 1.0,
                };
                if let (Some(logo), true) = (&self.logo, logo_alpha > 0.0) {
                    egui::Image::new(logo)
                        .tint(Color32::from_white_alpha((255.0 * logo_alpha) as u8))
                        .paint_at(ui, slot);
                }
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 4.0;
                    // New copy fades up into place; total height stays fixed.
                    ui.add_space(2.0 + (1.0 - fade) * 6.0);
                    ui.set_opacity(fade);
                    if loading {
                        ui.add_space(2.0);
                        kit::skeleton(ui, Vec2::new(220.0, 20.0), 6.0);
                        ui.add_space(4.0);
                        kit::skeleton(ui, Vec2::new(300.0, 13.0), 5.0);
                    } else {
                        kit::title(ui, &headline, 20.0);
                        if !detail.is_empty() {
                            kit::note(ui, &detail);
                        }
                    }
                    ui.add_space(fade * 6.0);
                });
            });
            if self.is_busy() {
                ui.add_space(18.0);
                ui.horizontal(|ui| {
                    let width = (ui.available_width() - 56.0).clamp(120.0, 360.0);
                    // Bar and percentage glide rather than jump between reports.
                    let shown = kit::gentle_spring(
                        ui.ctx(),
                        egui::Id::new("install-progress"),
                        self.progress,
                    )
                    .clamp(0.0, 1.0);
                    ui.vertical(|ui| {
                        ui.add_space(6.0);
                        kit::progress(ui, shown, width);
                    });
                    ui.label(
                        RichText::new(format!("{:.0}%", shown * 100.0))
                            .size(kit::CAPTION_SIZE)
                            .color(phase::text_secondary()),
                    );
                });
            } else if step == NextStep::Wait {
                ui.add_space(14.0);
                kit::spinner(ui, 22.0, phase::text_secondary());
            }
            if primary.is_some() || show_check {
                ui.add_space(20.0);
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = Vec2::new(8.0, 8.0);
                    if let Some(label) = primary {
                        action_clicked = kit::primary_button(ui, Some(icon), label).clicked();
                    }
                    if show_check {
                        ui.add_enabled_ui(!self.is_busy(), |ui| {
                            check_clicked = kit::quiet_button(ui, None, "Check again").clicked();
                        });
                    }
                });
            }
        });

        // Return runs the card's action when nothing else has focus.
        if primary.is_some()
            && ui.is_enabled()
            && ui.input(|i| i.key_pressed(egui::Key::Enter))
            && ui.memory(|m| m.focused().is_none())
        {
            action_clicked = true;
        }
        if action_clicked {
            match step {
                NextStep::Location => self.choose_folder(),
                NextStep::Connect => self.start_phase_account_link(ui.ctx()),
                NextStep::RefreshAccess => self.begin_phase_account_refresh(ui.ctx()),
                NextStep::Check => self.start_check(),
                NextStep::Install => {
                    if self
                        .installed_release
                        .as_ref()
                        .is_some_and(|r| r.channel == ReleaseChannel::EarlyAccess)
                    {
                        self.shell.install_confirmation = Some(ReleaseChannel::Stable);
                    } else {
                        self.start_install();
                    }
                }
                _ => {}
            }
        }
        if check_clicked {
            self.start_check();
        }
    }

    fn overview_tiles(&mut self, ui: &mut Ui) {
        let location_name = self
            .selected_folder
            .as_ref()
            .map(|path| friendly_folder(path))
            .unwrap_or_else(|| "Not chosen".to_owned());
        let location_tone = if self.selected_folder.is_some() {
            Tone::Good
        } else {
            Tone::Attention
        };
        let account = self.account_summary();
        let account_tone = if self.has_access() {
            Tone::Good
        } else if self.account_busy() {
            Tone::Busy
        } else {
            Tone::Attention
        };
        let (plugin_value, plugin_tone) = match &self.release {
            Some(release) if self.local_release_current => {
                (release.latest_version.clone(), Tone::Good)
            }
            Some(release) if self.has_local_phase_install() => (
                format!("{} available", release.latest_version),
                Tone::Accent,
            ),
            Some(_) => ("Not installed".to_owned(), Tone::Neutral),
            None if self.is_busy() => ("Checking…".to_owned(), Tone::Busy),
            None => ("Unknown".to_owned(), Tone::Neutral),
        };

        let mut clicked = None;
        let mut tiles = |ui: &mut Ui, index: usize| {
            let response = match index {
                0 => kit::status_tile(
                    ui,
                    Icon::FolderOpen,
                    "Install location",
                    &location_name,
                    location_tone,
                )
                .on_hover_text(
                    self.selected_folder
                        .as_ref()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|| "Choose where Phase is installed".to_owned()),
                ),
                1 => kit::status_tile(ui, Icon::UserCircle, "Account", &account, account_tone),
                _ => kit::status_tile(ui, Icon::PuzzlePiece, "Plugin", &plugin_value, plugin_tone)
                    .on_hover_text("Check for updates"),
            };
            if response.clicked() {
                clicked = Some(index);
            }
        };
        if ui.available_width() >= 480.0 {
            ui.spacing_mut().item_spacing.x = 12.0;
            ui.columns(3, |columns| {
                for (index, column) in columns.iter_mut().enumerate() {
                    tiles(column, index);
                }
            });
        } else {
            for index in 0..3 {
                tiles(ui, index);
                ui.add_space(4.0);
            }
        }
        match clicked {
            Some(0) => self.shell.install_location = true,
            Some(1) => self.open_page(Page::Account),
            Some(_) if !self.is_busy() => self.start_check(),
            _ => {}
        }
    }

    fn early_access_card(&mut self, ui: &mut Ui, account_busy: bool) {
        let available = self
            .build_access
            .as_ref()
            .and_then(|a| a.downloadable_release())
            .is_some();
        let mut download = false;
        kit::card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 14.0;
                kit::icon_chip(ui, Icon::Sparkle, phase::blue(), 42.0);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 3.0;
                    ui.horizontal(|ui| {
                        kit::title(ui, "Early Access", 16.0);
                    });
                    kit::note(
                        ui,
                        if available {
                            "Your personal tester build is ready. It replaces the stable plugin, and a backup is kept."
                        } else {
                            "Access verified. Your build will appear here when Early Access opens."
                        },
                    );
                });
            });
            ui.add_space(12.0);
            kit::divider(ui);
            ui.add_space(4.0);
            if kit::toggle(
                ui,
                &mut self.stable_updates_paused,
                "Pause stable update notifications",
                "Stay on your tester build without reminders.",
                true,
            ) {
                self.save_account_cache();
            }
            ui.add_enabled_ui(
                !self.is_busy() && !account_busy && available && self.selected_folder.is_some(),
                |ui| {
                    download = kit::secondary_button(
                        ui,
                        Some(Icon::DownloadSimple),
                        "Download Early Access build",
                    )
                    .clicked();
                },
            );
        });
        if download {
            self.shell.install_confirmation = Some(ReleaseChannel::EarlyAccess);
        }
    }

    // -----------------------------------------------------------------------
    // Reference

    fn reference_page(&mut self, ui: &mut Ui) {
        let dropped = ui.input(|i| i.raw.dropped_files.iter().find_map(|f| f.path.clone()));
        if let Some(path) = dropped {
            self.video_source = path.display().to_string();
            self.video_title = video_reference::default_title_for(&self.video_source);
            self.shell.reference_open = false;
            self.shell.reference_error = None;
        }
        let connected = self.video_bridge_connected;
        kit::page_header(
            ui,
            "Video reference",
            "Line up footage with your Studio timeline, frame for frame.",
            |ui| {
                kit::status_pill(
                    ui,
                    if connected { Tone::Good } else { Tone::Neutral },
                    if connected {
                        "Studio linked"
                    } else {
                        "Studio offline"
                    },
                );
            },
        );

        if self.shell.reference_open {
            self.now_playing(ui);
        } else if is_local_video(&self.video_source) {
            self.file_card(ui);
        } else {
            if kit::drop_zone(ui).clicked() && self.pick_video_file() {
                self.video_title = video_reference::default_title_for(&self.video_source);
                self.shell.reference_error = None;
            }
            ui.add_space(14.0);
            let has_source = !self.video_source.trim().is_empty();
            let (response, open_clicked) = kit::field_with_icon(
                ui,
                if self.video_source.contains("youtu") {
                    Icon::YoutubeLogo
                } else {
                    Icon::Link
                },
                &mut self.video_source,
                "Or paste a YouTube link or file path",
                |ui| {
                    if has_source {
                        kit::icon_button(ui, Icon::ArrowRight, "Open reference", 34.0, true)
                            .clicked()
                    } else {
                        false
                    }
                },
            );
            response.widget_info(|| {
                egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, "Reference source")
            });
            if response.changed() {
                self.video_title = video_reference::default_title_for(&self.video_source);
                self.shell.reference_error = None;
            }
            let submitted = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if has_source && (open_clicked || submitted) {
                self.open_reference();
            }
            if let Some(error) = self.shell.reference_error.clone() {
                ui.add_space(10.0);
                kit::banner(ui, Tone::Danger, Icon::Warning, &error);
            }
        }

        ui.add_space(18.0);
        self.timing_card(ui);
    }

    /// A chosen local file: lands with a short rise, then offers Open.
    fn file_card(&mut self, ui: &mut Ui) {
        let path = PathBuf::from(self.video_source.trim());
        let appear = kit::change_fade(
            ui.ctx(),
            egui::Id::new("file-card"),
            kit::hash_of(&self.video_source),
        );
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| self.video_source.clone());
        let folder = path
            .parent()
            .map(|p| compact_path(p, 48))
            .unwrap_or_default();
        let size = std::fs::metadata(&path).ok().map(|m| human_size(m.len()));
        let mut action = None;
        ui.add_space((1.0 - appear) * 12.0);
        ui.scope(|ui| {
            ui.set_opacity(appear);
            kit::card(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 14.0;
                    let (rect, _) = ui.allocate_exact_size(Vec2::new(72.0, 52.0), Sense::hover());
                    ui.painter().rect_filled(
                        rect,
                        12.0,
                        color_with_alpha(phase::surface_hover(), 0.8),
                    );
                    kit::glyph(
                        ui.painter(),
                        rect.center(),
                        Icon::FilmStrip,
                        24.0,
                        phase::text(),
                    );
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 3.0;
                        ui.add_space(4.0);
                        let width = (ui.available_width() - 90.0).max(80.0);
                        ui.label(kit::truncated(
                            ui,
                            &name,
                            kit::semibold(16.0),
                            phase::text(),
                            width,
                        ));
                        let meta = match &size {
                            Some(size) => format!("{size}  ·  {folder}"),
                            None => folder.clone(),
                        };
                        ui.label(kit::truncated(
                            ui,
                            &meta,
                            kit::body(kit::CAPTION_SIZE),
                            phase::text_muted(),
                            width,
                        ));
                    });
                    ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        if kit::icon_button(ui, Icon::Trash, "Remove", 34.0, false).clicked() {
                            action = Some("remove");
                        }
                        if kit::icon_button(
                            ui,
                            Icon::UploadSimple,
                            "Choose another file",
                            34.0,
                            false,
                        )
                        .clicked()
                        {
                            action = Some("replace");
                        }
                    });
                });
                ui.add_space(18.0);
                if kit::primary_button(ui, Some(Icon::MonitorPlay), "Open reference").clicked() {
                    action = Some("open");
                }
            });
        });
        if let Some(error) = self.shell.reference_error.clone() {
            ui.add_space(10.0);
            kit::banner(ui, Tone::Danger, Icon::Warning, &error);
        }
        match action {
            Some("open") => self.open_reference(),
            Some("replace") => {
                if self.pick_video_file() {
                    self.video_title = video_reference::default_title_for(&self.video_source);
                    self.shell.reference_error = None;
                }
            }
            Some("remove") => {
                self.video_source.clear();
                self.video_title.clear();
                self.shell.reference_error = None;
            }
            _ => {}
        }
    }

    fn open_reference(&mut self) {
        self.open_video_popup();
        self.shell.reference_open = self.video_bridge_status == "Video popup opened.";
        self.shell.reference_error =
            (!self.shell.reference_open).then(|| self.video_bridge_status.clone());
    }

    fn now_playing(&mut self, ui: &mut Ui) {
        let youtube = self.video_source.contains("youtu");
        let mut action = None;
        kit::card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 14.0;
                let (rect, _) = ui.allocate_exact_size(Vec2::new(88.0, 56.0), Sense::hover());
                ui.painter().rect_filled(
                    rect,
                    12.0,
                    lerp_color(phase::accent_dim(), phase::surface(), 0.3),
                );
                kit::glyph(
                    ui.painter(),
                    rect.center(),
                    if youtube {
                        Icon::YoutubeLogo
                    } else {
                        Icon::FilmStrip
                    },
                    26.0,
                    phase::text(),
                );
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 3.0;
                    ui.label(
                        RichText::new("Now referencing")
                            .size(kit::CAPTION_SIZE)
                            .color(phase::accent_hover()),
                    );
                    let title = if self.video_title.is_empty() {
                        "Untitled reference"
                    } else {
                        &self.video_title
                    };
                    let galley = kit::truncated(
                        ui,
                        title,
                        kit::semibold(17.0),
                        phase::text(),
                        ui.available_width() - 130.0,
                    );
                    ui.label(galley);
                    kit::caption(
                        ui,
                        if self.video_bridge_connected {
                            "Playback follows the Studio timeline."
                        } else {
                            "Waiting for Studio to connect."
                        },
                    );
                });
                ui.with_layout(egui::Layout::right_to_left(Align::Min), |ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    if kit::icon_button(ui, Icon::Trash, "Clear reference", 34.0, false).clicked() {
                        action = Some("clear");
                    }
                    if kit::icon_button(ui, Icon::UploadSimple, "Replace reference", 34.0, false)
                        .clicked()
                    {
                        action = Some("replace");
                    }
                    if kit::icon_button(ui, Icon::External, "Reopen viewer", 34.0, false).clicked()
                    {
                        action = Some("reopen");
                    }
                });
            });
            ui.add_space(22.0);
            let duration = parse_f64_or(&self.video_duration_seconds, 0.0);
            let seek_end = if duration > 0.0 {
                duration
            } else {
                self.video_position_seconds.max(3600.0)
            };
            if kit::scrubber(ui, &mut self.video_position_seconds, seek_end) {
                self.video_position_input = format_seconds(self.video_position_seconds);
                self.seek_video_sync();
            }
            ui.horizontal(|ui| {
                kit::caption(ui, format_seconds(self.video_position_seconds));
                ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                    kit::caption(
                        ui,
                        if duration > 0.0 {
                            format_seconds(duration)
                        } else {
                            "--:--".to_owned()
                        },
                    );
                });
            });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let total = 36.0 + 56.0 + 36.0 + 2.0 * 18.0;
                ui.add_space((ui.available_width() - total).max(0.0) / 2.0);
                ui.spacing_mut().item_spacing.x = 18.0;
                ui.vertical(|ui| {
                    ui.add_space(10.0);
                    if kit::icon_button(ui, Icon::SkipBack, "Back 5 seconds", 36.0, false).clicked()
                    {
                        action = Some("back");
                    }
                });
                if kit::icon_button(
                    ui,
                    if self.video_playing {
                        Icon::Pause
                    } else {
                        Icon::Play
                    },
                    if self.video_playing { "Pause" } else { "Play" },
                    56.0,
                    true,
                )
                .clicked()
                {
                    action = Some("toggle");
                }
                ui.vertical(|ui| {
                    ui.add_space(10.0);
                    if kit::icon_button(ui, Icon::SkipForward, "Forward 5 seconds", 36.0, false)
                        .clicked()
                    {
                        action = Some("forward");
                    }
                });
            });
        });
        match action {
            Some("toggle") => self.set_video_playing(!self.video_playing),
            Some(step @ ("back" | "forward")) => {
                let delta = if step == "back" { -5.0 } else { 5.0 };
                self.video_position_seconds = (self.video_position_seconds + delta).max(0.0);
                self.video_position_input = format_seconds(self.video_position_seconds);
                self.seek_video_sync();
            }
            Some("reopen") => self.open_video_popup(),
            Some("replace") => {
                if self.pick_video_file() {
                    self.video_title = video_reference::default_title_for(&self.video_source);
                    self.open_reference();
                }
            }
            Some("clear") => {
                self.clear_video_reference();
                self.video_source.clear();
                self.video_title.clear();
                self.shell.reference_open = false;
            }
            _ => {}
        }
    }

    fn timing_card(&mut self, ui: &mut Ui) {
        let open = self.shell.reference_timing;
        let header = kit::card(ui, |ui| {
            let (rect, response) =
                ui.allocate_exact_size(Vec2::new(ui.available_width(), 40.0), Sense::click());
            let chip = Rect::from_min_size(
                Pos2::new(rect.left(), rect.center().y - 17.0),
                Vec2::splat(34.0),
            );
            kit::paint_icon_chip(ui.painter(), chip, Icon::Clock, phase::accent_hover());
            ui.painter().text(
                Pos2::new(chip.right() + 12.0, rect.center().y - 9.0),
                Align2::LEFT_CENTER,
                "Timing & sync",
                kit::semibold(kit::TEXT_SIZE),
                phase::text(),
            );
            ui.painter().text(
                Pos2::new(chip.right() + 12.0, rect.center().y + 10.0),
                Align2::LEFT_CENTER,
                "Frame rate, offsets and playback behaviour",
                kit::body(kit::CAPTION_SIZE),
                phase::text_muted(),
            );
            let turn = ui
                .ctx()
                .animate_bool_with_time(response.id.with("open"), open, 0.18);
            let caret = Pos2::new(rect.right() - 12.0, rect.center().y);
            kit::glyph(
                ui.painter(),
                caret,
                if turn > 0.5 {
                    Icon::CaretDown
                } else {
                    Icon::CaretRight
                },
                15.0,
                phase::text_secondary(),
            );
            response.widget_info(|| {
                egui::WidgetInfo::selected(
                    egui::WidgetType::CollapsingHeader,
                    open,
                    "Timing & sync",
                )
            });
            if open {
                ui.add_space(10.0);
                kit::divider(ui);
                let field = 96.0;
                kit::setting_line(ui, "Name", "Shown in the viewer title.", 230.0, |ui| {
                    let width = ui.available_width().min(230.0);
                    kit::value_field(
                        ui,
                        &mut self.video_title,
                        "Untitled",
                        "",
                        width,
                        false,
                        false,
                    );
                });

                kit::section_label(ui, "Timing");
                kit::setting_line(
                    ui,
                    "Frame rate",
                    "Match your animation's frame rate.",
                    field + 150.0,
                    |ui| {
                        kit::value_field(ui, &mut self.video_fps, "60", "fps", field, true, false);
                        let mut preset = self.video_fps.trim().parse::<u32>().unwrap_or(0);
                        if kit::segmented(
                            ui,
                            "reference-fps",
                            &[(24, "24"), (30, "30"), (60, "60")],
                            &mut preset,
                        ) {
                            self.video_fps = preset.to_string();
                        }
                    },
                );
                kit::divider(ui);
                kit::setting_line(
                    ui,
                    "Duration",
                    "Leave empty to read it from the video.",
                    field,
                    |ui| {
                        kit::value_field(
                            ui,
                            &mut self.video_duration_seconds,
                            "Auto",
                            "s",
                            field,
                            true,
                            false,
                        );
                    },
                );
                kit::divider(ui);
                kit::setting_line(
                    ui,
                    "Start frame",
                    "The Studio frame where the video begins.",
                    field,
                    |ui| {
                        kit::value_field(
                            ui,
                            &mut self.video_start_frame,
                            "0",
                            "",
                            field,
                            true,
                            false,
                        );
                    },
                );
                kit::divider(ui);
                kit::setting_line(
                    ui,
                    "Offset",
                    "Nudge the video earlier or later.",
                    field,
                    |ui| {
                        kit::value_field(
                            ui,
                            &mut self.video_offset_seconds,
                            "0",
                            "s",
                            field,
                            true,
                            false,
                        );
                    },
                );
                kit::divider(ui);
                kit::setting_line(
                    ui,
                    "Playback rate",
                    "Speed relative to the timeline.",
                    field,
                    |ui| {
                        kit::value_field(
                            ui,
                            &mut self.video_playback_rate,
                            "1",
                            "×",
                            field,
                            true,
                            false,
                        );
                    },
                );

                kit::section_label(ui, "Playback");
                if kit::toggle(
                    ui,
                    &mut self.video_sync_enabled,
                    "Follow Studio",
                    "Scrub and play along with the Studio timeline.",
                    true,
                ) {
                    self.send_video_sync_enabled();
                }

                kit::section_label(ui, "Connection");
                kit::setting_line(
                    ui,
                    "Access token",
                    "Only needed if the plugin asks for one.",
                    180.0,
                    |ui| {
                        let width = ui.available_width().min(180.0);
                        kit::value_field(
                            ui,
                            &mut self.video_bridge_config.token,
                            "None",
                            "",
                            width,
                            false,
                            true,
                        );
                    },
                );
                ui.add_space(4.0);
                kit::caption(ui, "Changes apply the next time you open the viewer.");
            }
            response.clicked()
        });
        if header.inner {
            self.shell.reference_timing = !open;
        }
    }

    // -----------------------------------------------------------------------
    // Account

    fn account_page(&mut self, ui: &mut Ui) {
        let access = self.has_access();
        kit::page_header(
            ui,
            "Account",
            "Your Phase identity unlocks installs, updates and early builds.",
            |ui| {
                kit::status_pill(
                    ui,
                    if access { Tone::Good } else { Tone::Attention },
                    if access {
                        "Access verified"
                    } else {
                        "No install access"
                    },
                );
            },
        );
        self.phase_account_card(ui);
        ui.add_space(16.0);
        self.roblox_card(ui);
    }

    fn phase_account_card(&mut self, ui: &mut Ui) {
        let busy = self.link_rx.is_some()
            || self.link_status_rx.is_some()
            || self.account_refresh_rx.is_some();
        let waiting_link = self.link_rx.is_some() || self.link_status_rx.is_some();
        let mut action = None;
        kit::section_label(ui, "Phase account");
        kit::card(ui, |ui| {
            if let Some(user) = self.linked_user.clone() {
                let name = user
                    .display_name
                    .clone()
                    .filter(|n| !n.trim().is_empty())
                    .unwrap_or_else(|| user.username.clone());
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 16.0;
                    if self.phase_avatar.is_none() && user.avatar_url.is_some() {
                        kit::skeleton(ui, Vec2::splat(64.0), 32.0);
                    } else {
                        kit::avatar(ui, self.phase_avatar.as_ref(), &name, 64.0, Sense::hover());
                    }
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 4.0;
                        ui.add_space(4.0);
                        kit::title(ui, &name, 19.0);
                        kit::caption(ui, format!("@{}", user.username));
                        ui.add_space(2.0);
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 6.0;
                            if self.has_access() {
                                kit::status_pill(ui, Tone::Good, "Install access");
                            } else {
                                kit::status_pill(ui, Tone::Attention, "Access not verified");
                            }
                            if self.has_early_access() {
                                kit::early_access_badge(ui);
                            }
                        });
                    });
                });
                ui.add_space(18.0);
                kit::divider(ui);
                ui.add_space(12.0);
                ui.horizontal_wrapped(|ui| {
                    ui.add_enabled_ui(!busy, |ui| {
                        if kit::secondary_button(ui, Some(Icon::Refresh), "Refresh access")
                            .clicked()
                        {
                            action = Some("refresh");
                        }
                    });
                    ui.add_enabled_ui(self.phase_disconnect_rx.is_none(), |ui| {
                        if kit::quiet_button(ui, Some(Icon::SignOut), "Disconnect").clicked() {
                            action = Some("disconnect");
                        }
                    });
                    if self.account_refresh_rx.is_some() {
                        kit::spinner(ui, 22.0, phase::accent_hover());
                    }
                });
            } else if waiting_link {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 14.0;
                    kit::spinner(ui, 34.0, phase::accent_hover());
                    ui.vertical(|ui| {
                        kit::title(ui, "Finish in your browser", 17.0);
                        kit::note(ui, "Approve the connection on phase and this window updates automatically.");
                    });
                });
                if let Some(code) = self.link_code.clone() {
                    ui.add_space(16.0);
                    kit::caption(ui, "Your code");
                    ui.add_space(4.0);
                    link_code(ui, &code);
                }
                if self.link_url.is_some() {
                    ui.add_space(16.0);
                    if kit::secondary_button(ui, Some(Icon::External), "Open browser again")
                        .clicked()
                    {
                        action = Some("browser");
                    }
                }
            } else {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 14.0;
                    kit::icon_chip(ui, Icon::Link, phase::accent_hover(), 48.0);
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 4.0;
                        kit::title(ui, "Connect your Phase account", 17.0);
                        kit::note(
                            ui,
                            "Sign in once in your browser. We'll remember this device.",
                        );
                    });
                });
                ui.add_space(18.0);
                if kit::primary_button(ui, Some(Icon::UserCircle), "Connect Phase account")
                    .clicked()
                {
                    action = Some("connect");
                }
            }
        });
        match action {
            Some("refresh") => self.begin_phase_account_refresh(ui.ctx()),
            Some("disconnect") => self.start_phase_disconnect(ui.ctx()),
            Some("connect") => self.start_phase_account_link(ui.ctx()),
            Some("browser") => {
                if let Some(url) = self.link_url.clone() {
                    let _ = open::that(url);
                }
            }
            _ => {}
        }
    }

    fn roblox_card(&mut self, ui: &mut Ui) {
        let mut action = None;
        let oauth_busy = self.roblox_oauth_rx.is_some() || self.roblox_oauth_status_rx.is_some();
        kit::section_label(ui, "Roblox");
        kit::card(ui, |ui| {
            if self.roblox_user_id.trim().is_empty() {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 14.0;
                    kit::icon_chip(ui, Icon::ShieldCheck, phase::blue(), 44.0);
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 4.0;
                        kit::title(ui, "Verify with Roblox", 16.0);
                        kit::note(
                            ui,
                            "An alternative to a Phase account. Needed for license keys.",
                        );
                    });
                });
                ui.add_space(14.0);
                ui.horizontal_wrapped(|ui| {
                    ui.add_enabled_ui(!oauth_busy, |ui| {
                        if kit::secondary_button(ui, Some(Icon::ShieldCheck), "Verify with Roblox")
                            .clicked()
                        {
                            action = Some("verify");
                        }
                    });
                    if self.roblox_oauth_url.is_some()
                        && kit::quiet_button(ui, Some(Icon::External), "Continue in browser")
                            .clicked()
                    {
                        action = Some("continue");
                    }
                    if oauth_busy {
                        kit::spinner(ui, 22.0, phase::blue());
                    }
                });
            } else {
                let name = self
                    .roblox_username
                    .clone()
                    .filter(|n| !n.trim().is_empty())
                    .unwrap_or_else(|| format!("Roblox {}", self.roblox_user_id.trim()));
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 14.0;
                    kit::avatar(ui, self.roblox_avatar.as_ref(), &name, 48.0, Sense::hover());
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 3.0;
                        ui.add_space(3.0);
                        kit::title(ui, &name, 16.0);
                        kit::caption(ui, "Verified Roblox account");
                    });
                    ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                        if kit::quiet_button(ui, Some(Icon::SignOut), "Disconnect").clicked() {
                            action = Some("disconnect");
                        }
                    });
                });
                ui.add_space(16.0);
                kit::divider(ui);
                ui.add_space(12.0);
                ui.label(
                    RichText::new("License key")
                        .font(kit::semibold(kit::TEXT_SIZE))
                        .color(phase::text()),
                );
                kit::caption(
                    ui,
                    "Have a key instead of a subscription? Activate it here.",
                );
                ui.add_space(8.0);
                let activating = self.activation_rx.is_some();
                let (response, submit) = kit::field_with_icon(
                    ui,
                    Icon::Key,
                    &mut self.license_key,
                    "Enter your license key",
                    |ui| {
                        ui.add_enabled_ui(!activating, |ui| {
                            kit::icon_button(ui, Icon::ArrowRight, "Activate", 34.0, true).clicked()
                        })
                        .inner
                    },
                );
                let entered =
                    response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if (submit || entered) && !activating && !self.license_key.trim().is_empty() {
                    action = Some("activate");
                }
                if let Some(error) = self.activation_error.clone() {
                    ui.add_space(8.0);
                    kit::banner(ui, Tone::Danger, Icon::Warning, &error);
                }
            }
        });
        match action {
            Some("verify") => self.start_roblox_oauth(ui.ctx()),
            Some("continue") => {
                if let Some(url) = self.roblox_oauth_url.clone() {
                    let _ = open::that(url);
                }
            }
            Some("disconnect") => self.disconnect_roblox_account(),
            Some("activate") => self.start_activation(ui.ctx()),
            _ => {}
        }
    }

    // -----------------------------------------------------------------------
    // Settings

    fn settings_page(&mut self, ui: &mut Ui) {
        kit::page_header(ui, "Settings", "Make the companion yours.", |_| {});

        kit::group(ui, "Appearance", |ui| {
            let trailing = (ui.available_width() * 0.45).clamp(150.0, 240.0);
            kit::row(
                ui,
                Icon::Palette,
                "Theme",
                "Marketplace themes restyle the whole app.",
                trailing,
                |ui| {
                    self.theme_picker(ui, trailing);
                },
            );
            kit::divider(ui);
            kit::row(
                ui,
                Icon::Image,
                "Background",
                "How theme artwork fills the window.",
                190.0,
                |ui| {
                    let mut mode = self.theme_background_mode;
                    if kit::segmented(
                        ui,
                        "background-fit",
                        &[
                            (ThemeBackgroundMode::Crop, "Crop"),
                            (ThemeBackgroundMode::Fit, "Fit"),
                            (ThemeBackgroundMode::Stretch, "Stretch"),
                        ],
                        &mut mode,
                    ) {
                        self.theme_background_mode = mode;
                        self.save_account_cache();
                    }
                },
            );
        });

        kit::group(ui, "Installation", |ui| {
            let path = self
                .selected_folder
                .as_ref()
                .map(|p| compact_path(p, 46))
                .unwrap_or_else(|| "Not chosen yet".to_owned());
            let mut change = false;
            kit::row(
                ui,
                Icon::FolderOpen,
                "Install location",
                &path,
                100.0,
                |ui| {
                    change = kit::secondary_button(ui, None, "Change").clicked();
                },
            );
            if change {
                self.shell.install_location = true;
            }
            kit::divider(ui);
            let backup = kit::toggle(
                ui,
                &mut self.backup_before_install,
                "Keep a backup",
                "Save the previous plugin before every update.",
                true,
            );
            kit::divider(ui);
            let reminder = kit::toggle(
                ui,
                &mut self.restart_studio_hint,
                "Remind me to restart Studio",
                "Show a reminder after installing.",
                true,
            );
            if backup || reminder {
                self.save_account_cache();
            }
        });

        kit::group(ui, "Studio", |ui| {
            let busy = self.studio_patch_rx.is_some();
            let mut shortcuts = self.studio_patch_status.state == studio_patch::PatchState::Enabled;
            let can_change = !busy
                && !self.studio_patch_status.studio_running
                && (self.studio_patch_status.can_enable()
                    || self.studio_patch_status.can_disable());
            if kit::toggle(
                ui,
                &mut shortcuts,
                "Phase Ctrl shortcuts",
                if self.studio_patch_status.studio_running {
                    "Close Roblox Studio to change this."
                } else {
                    "Let Phase use Ctrl shortcuts while you animate. The original is kept as a backup."
                },
                can_change,
            ) {
                self.begin_studio_patch_action(
                    if shortcuts {
                        studio_patch::PatchAction::Enable
                    } else {
                        studio_patch::PatchAction::Disable
                    },
                    ui.ctx(),
                );
            }
            kit::divider(ui);
            let message = self
                .studio_patch_message
                .clone()
                .unwrap_or_else(|| "Look for a compatible Studio build.".to_owned());
            let mut scan = false;
            kit::row(ui, Icon::Keyboard, "Scan Studio", &message, 90.0, |ui| {
                ui.add_enabled_ui(!busy, |ui| {
                    scan = kit::secondary_button(ui, None, "Scan").clicked();
                });
            });
            if scan {
                self.begin_studio_patch_action(studio_patch::PatchAction::Inspect, ui.ctx());
            }
        });

        kit::group(ui, "Connections", |ui| {
            let busy = self.diagnostics_rx.is_some() || self.diagnostics_fix_rx.is_some();
            let status = match &self.diagnostics_report {
                Some(report) if report.overall_status() == diagnostics::DiagnosticStatus::Good => {
                    "Everything looks healthy."
                }
                Some(_) => "Some checks need attention.",
                None if busy => "Running checks…",
                None => "Test your connection to Phase and Roblox.",
            };
            let needs_repair = self
                .diagnostics_report
                .as_ref()
                .is_some_and(|r| r.overall_status() != diagnostics::DiagnosticStatus::Good);
            let mut action = None;
            kit::row(
                ui,
                Icon::Heartbeat,
                "Connection check",
                status,
                if needs_repair { 180.0 } else { 90.0 },
                |ui| {
                    ui.add_enabled_ui(!busy, |ui| {
                        if kit::secondary_button(
                            ui,
                            None,
                            if self.diagnostics_report.is_some() {
                                "Report"
                            } else {
                                "Run"
                            },
                        )
                        .clicked()
                        {
                            action = Some("check");
                        }
                        if needs_repair
                            && kit::secondary_button(ui, Some(Icon::Wrench), "Repair").clicked()
                        {
                            action = Some("repair");
                        }
                    });
                },
            );
            kit::divider(ui);
            kit::row(
                ui,
                Icon::Broadcast,
                "Video bridge",
                if self.video_bridge_connected {
                    "Connected to Studio."
                } else {
                    "Waiting for Studio."
                },
                150.0,
                |ui| {
                    if kit::quiet_button(ui, None, "Ping").clicked() {
                        action = Some("ping");
                    }
                    if kit::quiet_button(ui, None, "Restart").clicked() {
                        action = Some("restart");
                    }
                },
            );
            match action {
                Some("check") => {
                    if self.diagnostics_report.is_none() {
                        self.start_connection_diagnostics(ui.ctx());
                    }
                    self.diagnostics_open = true;
                }
                Some("repair") => self.start_connection_fix(ui.ctx()),
                Some("ping") => self.send_video_ping(),
                Some("restart") => self.restart_video_bridge(),
                _ => {}
            }
        });

        kit::group(ui, "About", |ui| {
            let busy = self.app_update_rx.is_some() || self.app_update_install_rx.is_some();
            let detail = if let Some(update) = &self.app_update {
                format!(
                    "Version {} · {} is available",
                    env!("CARGO_PKG_VERSION"),
                    update.version
                )
            } else if let Some(error) = &self.app_update_error {
                format!("Version {} · {error}", env!("CARGO_PKG_VERSION"))
            } else {
                format!("Version {} · Up to date", env!("CARGO_PKG_VERSION"))
            };
            let mut action = None;
            kit::row(ui, Icon::Package, "Phase Companion", &detail, 150.0, |ui| {
                ui.add_enabled_ui(!busy, |ui| {
                    if self.app_update.is_some() {
                        if kit::secondary_button(ui, Some(Icon::DownloadSimple), "Install")
                            .clicked()
                        {
                            action = Some("install");
                        }
                    } else if kit::secondary_button(ui, Some(Icon::Refresh), "Check").clicked() {
                        action = Some("check");
                    }
                });
            });
            match action {
                Some("install") => self.start_app_update_install(ui.ctx()),
                Some("check") => self.begin_app_update_check(ui.ctx()),
                _ => {}
            }
        });

        kit::group(ui, "Reset", |ui| {
            kit::row(
                ui,
                Icon::Trash,
                "Reset plugin data",
                "Clear saved Phase data from Studio. Close Studio first; a backup is made.",
                0.0,
                |_| {},
            );
            ui.horizontal(|ui| {
                ui.add_space(46.0);
                ui.checkbox(&mut self.plugin_settings_reset_themes, "Themes");
                ui.checkbox(&mut self.plugin_settings_reset_keybinds, "Keybinds");
            });
            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                ui.add_space(46.0);
                if self.plugin_data_reset_confirm {
                    if kit::secondary_button(ui, Some(Icon::Trash), "Back up and delete").clicked()
                    {
                        self.reset_phase_plugin_data();
                    }
                    if kit::quiet_button(ui, None, "Cancel").clicked() {
                        self.plugin_data_reset_confirm = false;
                    }
                } else {
                    ui.add_enabled_ui(
                        self.plugin_settings_reset_themes || self.plugin_settings_reset_keybinds,
                        |ui| {
                            if kit::secondary_button(ui, None, "Reset selected…").clicked() {
                                self.plugin_data_reset_confirm = true;
                            }
                        },
                    );
                }
            });
            if let Some(status) = &self.plugin_data_reset_status {
                ui.add_space(6.0);
                kit::caption(ui, status);
            }
            ui.add_space(12.0);
        });
    }

    fn theme_picker(&mut self, ui: &mut Ui, width: f32) {
        let current = self
            .selected_theme
            .as_ref()
            .map(|t| t.title.clone())
            .unwrap_or_else(|| "Default".to_owned());
        let picker_id = ui.make_persistent_id("theme-picker");
        if self.screenshot_path.is_some()
            && std::env::var("PHASE_UI_DIALOG").ok().as_deref() == Some("themes")
        {
            ui.memory_mut(|m| m.open_popup(picker_id));
        }
        let selected_asset = self.selected_theme.as_ref().and_then(|selected| {
            self.theme_assets
                .iter()
                .find(|a| a.id == selected.asset_id)
                .cloned()
        });
        if let Some(asset) = &selected_asset {
            self.ensure_theme_preview_fetch(ui.ctx(), asset);
        }
        let image = selected_asset
            .as_ref()
            .and_then(|a| self.theme_preview_textures.get(&a.id))
            .cloned();
        let open = ui.memory(|m| m.is_popup_open(picker_id));
        let response = kit::select_button(
            ui,
            image.as_ref(),
            Some(phase::background()),
            &current,
            width,
            open,
        );
        if response.clicked() {
            ui.memory_mut(|m| m.toggle_popup(picker_id));
        }
        kit::picker(ui, &response, picker_id, |ui| {
            kit::theme_search(ui, &mut self.theme_search);
            ui.add_space(6.0);
            egui::ScrollArea::vertical()
                .max_height(260.0)
                .show(ui, |ui| {
                    if "default".contains(&self.theme_search.to_lowercase())
                        && kit::theme_choice(
                            ui,
                            "Default",
                            self.logo_default.as_ref(),
                            self.selected_theme.is_none(),
                            phase::surface(),
                        )
                        .clicked()
                    {
                        self.reset_theme(ui.ctx());
                        ui.memory_mut(|m| m.close_popup());
                    }
                    let assets: Vec<_> = self
                        .theme_assets
                        .iter()
                        .filter(|a| theme_matches_search(a, &self.theme_search))
                        .cloned()
                        .collect();
                    if assets.is_empty() && !self.theme_search.is_empty() {
                        kit::note(ui, "No matching themes.");
                    }
                    for asset in assets {
                        let enabled =
                            self.theme_apply_rx.is_none() && self.theme_transition.is_none();
                        let response = ui
                            .add_enabled_ui(enabled, |ui| {
                                kit::theme_choice(
                                    ui,
                                    &asset.title,
                                    self.theme_preview_textures.get(&asset.id),
                                    self.selected_theme
                                        .as_ref()
                                        .is_some_and(|t| t.asset_id == asset.id),
                                    phase::hex_color(&asset.theme_preview.background)
                                        .unwrap_or_else(phase::surface),
                                )
                            })
                            .inner;
                        if ui.is_rect_visible(response.rect) {
                            self.ensure_theme_preview_fetch(ui.ctx(), &asset);
                        }
                        if response.clicked() {
                            self.start_theme_apply(ui.ctx(), asset);
                            ui.memory_mut(|m| m.close_popup());
                        }
                    }
                });
            ui.add_space(4.0);
            ui.add_enabled_ui(self.theme_fetch_rx.is_none(), |ui| {
                if kit::quiet_button(ui, Some(Icon::Refresh), "Refresh themes").clicked() {
                    self.begin_theme_fetch(ui.ctx());
                }
            });
            if let Some(error) = &self.theme_error {
                kit::caption(ui, error);
            }
        });
    }

    // -----------------------------------------------------------------------
    // Dialogs

    fn overlays(&mut self, ctx: &Context) {
        if self.screenshot_path.is_some()
            && std::env::var("PHASE_UI_DIALOG").ok().as_deref() == Some("early-access")
        {
            self.shell.install_confirmation = Some(ReleaseChannel::EarlyAccess);
        }
        if let Some(channel) = self.shell.install_confirmation {
            let early = channel == ReleaseChannel::EarlyAccess;
            let mut confirm = false;
            let mut cancel = false;
            let open = kit::modal(
                ctx,
                "channel-install-confirmation",
                if early {
                    "Switch to Early Access?"
                } else {
                    "Switch to stable?"
                },
                440.0,
                |ui| {
                    kit::note(
                        ui,
                        if early {
                            "This replaces your normal Phase Animator plugin in Studio. Your current file is backed up, and stable update notifications are paused."
                        } else {
                            "This replaces the Early Access plugin with stable Phase Animator and resumes stable update notifications."
                        },
                    );
                    if early {
                        ui.add_space(12.0);
                        kit::banner(
                            ui,
                            Tone::Attention,
                            Icon::Lock,
                            "This build is assigned to your account. Sharing it may cost you Early Access.",
                        );
                    }
                    ui.add_space(20.0);
                    ui.allocate_ui_with_layout(
                        Vec2::new(ui.available_width(), 48.0),
                        egui::Layout::right_to_left(Align::Center),
                        |ui| {
                            let allowed = !self.is_busy()
                                && (!early
                                    || self
                                        .build_access
                                        .as_ref()
                                        .and_then(|a| a.downloadable_release())
                                        .is_some());
                            ui.add_enabled_ui(allowed, |ui| {
                                confirm = kit::primary_button(
                                    ui,
                                    Some(Icon::DownloadSimple),
                                    if early {
                                        "Download build"
                                    } else {
                                        "Install stable"
                                    },
                                )
                                .clicked();
                            });
                            cancel = kit::quiet_button(ui, None, "Cancel").clicked();
                        },
                    );
                },
            );
            if confirm {
                self.shell.install_confirmation = None;
                self.start_install_channel(channel);
            } else if cancel || !open {
                self.shell.install_confirmation = None;
            }
        }

        if self.shell.install_location {
            let mut done = false;
            let open = kit::modal(
                ctx,
                "compact-install-location",
                "Install location",
                440.0,
                |ui| {
                    kit::note(ui, "Phase installs into your Roblox Studio plugins folder.");
                    ui.add_space(14.0);
                    ui.spacing_mut().item_spacing.y = 6.0;
                    for candidate in self.candidates.clone() {
                        let selected = self
                            .selected_folder
                            .as_ref()
                            .is_some_and(|p| normalize_path(p) == normalize_path(&candidate.path));
                        if folder_option(
                            ui,
                            &candidate.source,
                            &candidate.path.display().to_string(),
                            selected,
                        )
                        .clicked()
                        {
                            self.selected_folder = Some(candidate.path);
                            self.refresh_local_release_status();
                        }
                    }
                    if let Some(path) = self.selected_folder.clone() {
                        let known = self
                            .candidates
                            .iter()
                            .any(|c| normalize_path(&c.path) == normalize_path(&path));
                        if !known {
                            folder_option(ui, "Custom folder", &path.display().to_string(), true);
                        }
                    }
                    if self.candidates.is_empty() && self.selected_folder.is_none() {
                        kit::banner(
                            ui,
                            Tone::Attention,
                            Icon::Warning,
                            "We couldn't find Studio automatically. Browse to your plugins folder.",
                        );
                    }
                    ui.add_space(16.0);
                    ui.horizontal_wrapped(|ui| {
                        if kit::secondary_button(ui, Some(Icon::FolderOpen), "Browse…").clicked()
                        {
                            self.choose_folder();
                        }
                        if kit::quiet_button(ui, Some(Icon::Refresh), "Scan again").clicked() {
                            self.refresh_detection();
                        }
                        ui.add_enabled_ui(self.selected_folder.is_some(), |ui| {
                            if kit::quiet_button(ui, Some(Icon::External), "Open").clicked() {
                                self.open_folder();
                            }
                        });
                    });
                    ui.add_space(8.0);
                    ui.allocate_ui_with_layout(
                        Vec2::new(ui.available_width(), 48.0),
                        egui::Layout::right_to_left(Align::Center),
                        |ui| {
                            done = kit::primary_button(ui, Some(Icon::Check), "Done").clicked();
                        },
                    );
                },
            );
            if done || !open {
                self.shell.install_location = false;
            }
        }
    }
}

const SUCCESS_SECS: f32 = 2.8;

/// A green disc that springs in, a check that draws itself, then a fade.
fn paint_success(painter: &egui::Painter, rect: Rect, t: f32) {
    let out = 1.0 - ((t - (SUCCESS_SECS - 0.4)) / 0.4).clamp(0.0, 1.0);
    let grow = ease_out_back((t / 0.38).clamp(0.0, 1.0));
    let center = rect.center();
    let color = color_with_alpha(kit::tone_color(Tone::Good), out);
    painter.circle_filled(center, 22.0 * grow, color);
    let points = [
        center + Vec2::new(-8.5, 0.5),
        center + Vec2::new(-2.5, 6.5),
        center + Vec2::new(9.0, -6.0),
    ];
    let draw = ((t - 0.22) / 0.34).clamp(0.0, 1.0);
    let first = (points[1] - points[0]).length();
    let second = (points[2] - points[1]).length();
    let mut travelled = ease_out_cubic(draw) * (first + second);
    let mut line = vec![points[0]];
    for pair in points.windows(2) {
        let length = (pair[1] - pair[0]).length();
        if travelled >= length {
            line.push(pair[1]);
            travelled -= length;
        } else {
            line.push(pair[0] + (pair[1] - pair[0]) * (travelled / length));
            break;
        }
    }
    if draw > 0.0 {
        painter.add(egui::Shape::line(
            line,
            Stroke::new(3.0, color_with_alpha(phase::background(), out)),
        ));
    }
}

fn is_local_video(source: &str) -> bool {
    let source = source.trim().to_ascii_lowercase();
    !source.starts_with("http")
        && [".mp4", ".mov", ".m4v", ".webm"]
            .iter()
            .any(|ext| source.ends_with(ext))
}

/// "Roblox › Plugins" rather than a raw path or environment-variable label.
fn friendly_folder(path: &Path) -> String {
    let parts: Vec<_> = path
        .components()
        .rev()
        .filter_map(|c| c.as_os_str().to_str())
        .take(2)
        .collect();
    match parts.as_slice() {
        [last, parent] => format!("{parent} › {last}"),
        [last] => (*last).to_owned(),
        _ => path.display().to_string(),
    }
}

fn paint_ambient(painter: &egui::Painter, rect: Rect) {
    radial_gradient(
        painter,
        Pos2::new(
            rect.right() - rect.width() * 0.12,
            rect.top() + rect.height() * 0.05,
        ),
        rect.width() * 0.55,
        color_with_alpha(phase::accent(), 0.10),
    );
    radial_gradient(
        painter,
        Pos2::new(
            rect.left() + rect.width() * 0.35,
            rect.bottom() + rect.height() * 0.1,
        ),
        rect.width() * 0.5,
        color_with_alpha(phase::blue(), 0.06),
    );
}

fn studio_status(ui: &mut Ui, connected: bool, compact: bool) {
    let color = if connected {
        phase::green()
    } else {
        phase::text_muted()
    };
    let label = if connected {
        "Studio connected"
    } else {
        "Studio not connected"
    };
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 22.0), Sense::hover());
    let dot = if compact {
        rect.center()
    } else {
        Pos2::new(rect.left() + 12.0, rect.center().y)
    };
    if connected {
        ui.painter()
            .circle_filled(dot, 7.0, color_with_alpha(color, 0.2));
    }
    ui.painter().circle_filled(dot, 3.5, color);
    if !compact {
        ui.painter().text(
            Pos2::new(dot.x + 12.0, rect.center().y),
            Align2::LEFT_CENTER,
            label,
            kit::body(kit::CAPTION_SIZE),
            phase::text_secondary(),
        );
    }
    response.on_hover_text(label);
}

/// Large, spaced characters for a device link code.
fn link_code(ui: &mut Ui, code: &str) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        for ch in code.chars().filter(|c| !c.is_whitespace()) {
            if ch == '-' {
                kit::caption(ui, "–");
                continue;
            }
            let (rect, _) = ui.allocate_exact_size(Vec2::new(34.0, 42.0), Sense::hover());
            ui.painter()
                .rect_filled(rect, 9.0, color_with_alpha(phase::input(), 0.95));
            ui.painter().rect_stroke(
                rect,
                9.0,
                Stroke::new(1.0, color_with_alpha(phase::accent(), 0.35)),
            );
            ui.painter().text(
                rect.center(),
                Align2::CENTER_CENTER,
                ch.to_string(),
                kit::semibold(20.0),
                phase::text(),
            );
        }
    });
}

fn folder_option(ui: &mut Ui, name: &str, path: &str, selected: bool) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 58.0), Sense::click());
    let hover =
        ui.ctx()
            .animate_bool_with_time(response.id.with("hover"), response.hovered(), 0.12);
    let fill = if selected {
        color_with_alpha(phase::accent(), 0.14)
    } else {
        color_with_alpha(phase::surface_hover(), 0.25 + 0.35 * hover)
    };
    ui.painter().rect_filled(rect, 12.0, fill);
    ui.painter().rect_stroke(
        rect,
        12.0,
        Stroke::new(
            1.0,
            if selected {
                color_with_alpha(phase::accent(), 0.5)
            } else {
                color_with_alpha(phase::line(), 0.35)
            },
        ),
    );
    let radio = Pos2::new(rect.left() + 22.0, rect.center().y);
    ui.painter().circle_stroke(
        radio,
        8.0,
        Stroke::new(
            1.5,
            if selected {
                phase::accent()
            } else {
                phase::text_muted()
            },
        ),
    );
    if selected {
        ui.painter().circle_filled(radio, 4.5, phase::accent());
    }
    let width = rect.width() - 56.0;
    let name_g = kit::truncated(
        ui,
        name,
        kit::semibold(kit::TEXT_SIZE),
        phase::text(),
        width,
    );
    let path_g = kit::truncated(
        ui,
        path,
        kit::body(kit::CAPTION_SIZE),
        phase::text_muted(),
        width,
    );
    ui.painter().galley(
        Pos2::new(rect.left() + 42.0, rect.top() + 10.0),
        name_g,
        phase::text(),
    );
    ui.painter().galley(
        Pos2::new(rect.left() + 42.0, rect.top() + 31.0),
        path_g,
        phase::text_muted(),
    );
    response
        .widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::RadioButton, selected, name));
    response.on_hover_text(path)
}

#[cfg(test)]
mod workflow_tests {
    use super::*;
    #[test]
    fn setup_offers_only_the_first_missing_requirement() {
        assert_eq!(
            next_step(false, false, false, false, false, true, true),
            NextStep::Location
        );
        assert_eq!(
            next_step(false, true, false, false, false, true, true),
            NextStep::Connect
        );
        assert_eq!(
            next_step(false, true, false, true, false, true, true),
            NextStep::RefreshAccess
        );
        assert_eq!(
            next_step(false, true, true, true, false, true, true),
            NextStep::Install
        );
    }
    #[test]
    fn busy_ready_and_blocked_releases_cannot_offer_install() {
        assert_eq!(
            next_step(true, true, true, true, false, true, true),
            NextStep::Wait
        );
        assert_eq!(
            next_step(false, true, true, true, true, true, true),
            NextStep::Ready
        );
        assert_eq!(
            next_step(false, true, true, true, false, true, false),
            NextStep::Unavailable
        );
        assert_eq!(
            next_step(false, true, true, true, false, false, false),
            NextStep::Check
        );
    }
}
