use super::*;

const APP_INSET: f32 = 0.0;
const SIDEBAR_WIDTH: f32 = 178.0;
const SHELL_GAP: f32 = 0.0;
const HEADER_HEIGHT: f32 = 66.0;
const INSTALL_FOOTER_HEIGHT: f32 = 66.0;
const WORKSPACE_GUTTER_X: f32 = 18.0;
const WORKSPACE_GUTTER_Y: f32 = 12.0;

impl PhaseInstallerApp {
    pub(super) fn render_companion_root(&mut self, ui: &mut Ui) {
        self.paint_theme_background(ui);
        let canvas = ui.available_rect_before_wrap();
        ui.allocate_rect(canvas, Sense::hover());

        let entrance =
            ease_out_cubic((self.ui_started_at.elapsed().as_secs_f32() / 0.28).clamp(0.0, 1.0));
        if entrance < 1.0 {
            ui.ctx().request_repaint_after(Duration::from_millis(16));
        }

        let shell = canvas
            .shrink(APP_INSET)
            .translate(Vec2::new(0.0, (1.0 - entrance) * 6.0));
        ui.allocate_ui_at_rect(shell, |ui| {
            ui.set_opacity(0.45 + entrance * 0.55);
            self.desktop_shell(ui);
        });
    }

    fn desktop_shell(&mut self, ui: &mut Ui) {
        let available = ui.available_size();
        let sidebar_width = SIDEBAR_WIDTH.min((available.x * 0.27).max(154.0));
        let workspace_width = (available.x - sidebar_width - SHELL_GAP).max(320.0);

        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = SHELL_GAP;
            ui.allocate_ui_with_layout(
                Vec2::new(sidebar_width, available.y),
                egui::Layout::top_down(Align::Min),
                |ui| self.sidebar(ui),
            );
            ui.allocate_ui_with_layout(
                Vec2::new(workspace_width, available.y),
                egui::Layout::top_down(Align::Min),
                |ui| self.workspace(ui),
            );
        });
    }

    fn sidebar(&mut self, ui: &mut Ui) {
        let rect = ui.available_rect_before_wrap();
        ui.allocate_rect(rect, Sense::hover());
        let fill = if self.has_theme_background_art() {
            color_with_alpha(phase::surface(), 0.82)
        } else {
            phase::surface()
        };
        ui.painter().rect_filled(rect, Rounding::ZERO, fill);
        ui.painter().vline(
            rect.right(),
            rect.top()..=rect.bottom(),
            Stroke::new(1.0, color_with_alpha(phase::line(), 0.62)),
        );

        let inner = rect.shrink2(Vec2::new(12.0, 14.0));
        ui.allocate_ui_at_rect(inner, |ui| {
            let destinations = [
                (ViewTab::Install, MiniIcon::CloudArrowDown, "Updates"),
                (ViewTab::Video, MiniIcon::FilmStrip, "Video sync"),
                (ViewTab::Folders, MiniIcon::Folder, "Plugin folders"),
                (ViewTab::Account, MiniIcon::User, "Accounts"),
                (ViewTab::Options, MiniIcon::Gear, "Settings"),
            ];
            for (tab, icon, label) in destinations {
                if navigation_row(ui, tab == self.active_tab, icon, label).clicked() {
                    self.select_tab(tab);
                }
                ui.add_space(3.0);
            }

            let bottom = ui.max_rect().bottom();
            let account_top = bottom - 74.0;
            if ui.cursor().top() < account_top {
                ui.add_space(account_top - ui.cursor().top());
            }
            ui.painter().hline(
                ui.min_rect().left()..=ui.min_rect().right(),
                ui.cursor().top(),
                Stroke::new(1.0, color_with_alpha(phase::line(), 0.45)),
            );
            ui.add_space(12.0);
            self.sidebar_identity(ui);
        });
    }

    fn sidebar_identity(&mut self, ui: &mut Ui) {
        let linked = self.plugin_token.is_some();
        let (rect, response) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), 46.0), Sense::click());
        let hover = hover_t(ui, response.id, response.hovered());
        if hover > 0.0 {
            ui.painter().rect_filled(
                rect,
                Rounding::ZERO,
                color_with_alpha(phase::surface_hover(), 0.55 * hover),
            );
        }
        let avatar = Rect::from_center_size(
            Pos2::new(rect.left() + 18.0, rect.center().y),
            Vec2::splat(28.0),
        );
        if let Some(texture) = self.phase_avatar.as_ref().or(self.logo.as_ref()) {
            ui.painter().image(
                texture.id(),
                avatar,
                Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                Color32::WHITE,
            );
        }
        ui.painter().circle_stroke(
            avatar.center(),
            avatar.width() * 0.5,
            Stroke::new(1.0, color_with_alpha(phase::line(), 0.8)),
        );
        let text_left = avatar.right() + 9.0;
        ui.painter().text(
            Pos2::new(text_left, rect.center().y - 7.0),
            Align2::LEFT_CENTER,
            compact_middle(&self.account_summary(), 16),
            type_display(11.5),
            phase::text(),
        );
        ui.painter().text(
            Pos2::new(text_left, rect.center().y + 9.0),
            Align2::LEFT_CENTER,
            if linked {
                "Account connected"
            } else {
                "Connect account"
            },
            FontId::proportional(9.5),
            phase::text_muted(),
        );
        if response.clicked() {
            self.select_tab(ViewTab::Account);
        }
        if response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
    }

    fn workspace(&mut self, ui: &mut Ui) {
        let bounds = ui.available_rect_before_wrap();
        ui.allocate_rect(bounds, Sense::hover());
        let workspace_alpha = if self.has_theme_background_art() {
            0.60
        } else {
            0.88
        };
        ui.painter().rect_filled(
            bounds,
            Rounding::ZERO,
            color_with_alpha(phase::background(), workspace_alpha),
        );

        let header = Rect::from_min_size(bounds.min, Vec2::new(bounds.width(), HEADER_HEIGHT));
        let footer_height = if self.active_tab == ViewTab::Install {
            INSTALL_FOOTER_HEIGHT
        } else {
            0.0
        };
        let body = Rect::from_min_max(
            Pos2::new(bounds.left(), header.bottom()),
            Pos2::new(bounds.right(), bounds.bottom() - footer_height),
        );
        ui.painter().hline(
            bounds.left()..=bounds.right(),
            header.bottom(),
            Stroke::new(1.0, color_with_alpha(phase::line(), 0.48)),
        );

        ui.allocate_ui_at_rect(
            body.shrink2(Vec2::new(WORKSPACE_GUTTER_X, WORKSPACE_GUTTER_Y)),
            |ui| self.workspace_content(ui),
        );

        if self.active_tab == ViewTab::Install {
            let footer = Rect::from_min_max(
                Pos2::new(bounds.left(), bounds.bottom() - INSTALL_FOOTER_HEIGHT),
                bounds.right_bottom(),
            );
            ui.painter().hline(
                footer.left()..=footer.right(),
                footer.top(),
                Stroke::new(1.0, color_with_alpha(phase::line(), 0.58)),
            );
            ui.allocate_ui_at_rect(footer.shrink2(Vec2::new(WORKSPACE_GUTTER_X, 10.0)), |ui| {
                self.install_action_bar(ui)
            });
        }
        // Paint the persistent orientation header last so scroll content can
        // never cover it when a dense page extends beyond the viewport.
        ui.allocate_ui_at_rect(header, |ui| self.workspace_header(ui));
    }

    fn workspace_header(&self, ui: &mut Ui) {
        let rect = ui
            .available_rect_before_wrap()
            .shrink2(Vec2::new(WORKSPACE_GUTTER_X, 0.0));
        ui.allocate_rect(rect, Sense::hover());
        let (title, detail, icon) = match self.active_tab {
            ViewTab::Install => ("Phase Animator", "Plugin updates", MiniIcon::CloudArrowDown),
            ViewTab::Video => (
                "Video sync",
                "Reference playback linked to Studio",
                MiniIcon::FilmStrip,
            ),
            ViewTab::Folders => (
                "Plugin folders",
                "Where Phase Animator is installed",
                MiniIcon::Folder,
            ),
            ViewTab::Account => ("Accounts", "Access required to install", MiniIcon::User),
            ViewTab::Options => ("Settings", "Appearance and preferences", MiniIcon::Gear),
        };
        draw_icon_at(
            ui.painter(),
            Rect::from_center_size(
                Pos2::new(rect.left() + 9.0, rect.center().y),
                Vec2::splat(18.0),
            ),
            icon,
            phase::text_secondary(),
        );
        ui.painter().text(
            Pos2::new(rect.left() + 30.0, rect.center().y - 8.0),
            Align2::LEFT_CENTER,
            title,
            type_display(18.0),
            phase::text(),
        );
        ui.painter().text(
            Pos2::new(rect.left() + 30.0, rect.center().y + 11.0),
            Align2::LEFT_CENTER,
            detail,
            FontId::proportional(10.5),
            phase::text_muted(),
        );
    }

    fn workspace_content(&mut self, ui: &mut Ui) {
        let width = ui.available_width().max(1.0);
        set_content_width(width);
        if self.reset_body_scroll {
            // `vertical_scroll_offset` below applies only for the first frame after navigation.
        }
        let mut scroll = egui::ScrollArea::vertical()
            .id_source(("phase-workspace", self.active_tab.index()))
            .auto_shrink([false, false]);
        if self.reset_body_scroll {
            scroll = scroll.vertical_scroll_offset(0.0);
        }
        scroll.show(ui, |ui| {
            ui.set_width(width);
            let dt = ui.input(|input| input.stable_dt).clamp(0.0, 1.0 / 30.0);
            let frame = self.tab_page_motion.step(dt, 0.16);
            if frame.running {
                ui.ctx().request_repaint();
            }
            ui.scope(|ui| {
                ui.set_opacity(frame.opacity);
                ui.add_space(frame.offset.abs() * 0.16);
                match self.active_tab {
                    ViewTab::Install => self.install_workspace(ui),
                    ViewTab::Account => self.account_workspace_v3(ui),
                    ViewTab::Folders => self.folders_workspace_v3(ui),
                    ViewTab::Video => self.video_workspace_v3(ui),
                    ViewTab::Options => self.settings_workspace_v3(ui),
                }
                ui.add_space(PAGE_BOTTOM_INSET);
            });
        });
        self.reset_body_scroll = false;
    }

    fn install_workspace(&mut self, ui: &mut Ui) {
        let latest_version = self
            .release
            .as_ref()
            .map(|release| release.latest_version.clone())
            .unwrap_or_else(|| "Not checked".to_owned());
        let installed_version = if self.local_release_current
            || (self.phase == InstallPhase::Complete && self.release.is_some())
        {
            latest_version.clone()
        } else if self.has_local_phase_install() {
            "Earlier version".to_owned()
        } else {
            "Not installed".to_owned()
        };
        let state_title = match self.phase {
            InstallPhase::Checking => "Checking for updates…",
            InstallPhase::Ready => "Update available",
            InstallPhase::Downloading => "Downloading update…",
            InstallPhase::Installing => "Installing update…",
            InstallPhase::Complete => "You’re up to date",
            InstallPhase::Error => "Couldn’t check for updates",
            InstallPhase::Idle => "Check for the latest version",
        };
        let state_detail = if let Some(error) = self.release_error.as_deref() {
            error.to_owned()
        } else if self.phase == InstallPhase::Ready {
            format!("Phase Animator {latest_version} is ready to install.")
        } else if self.phase == InstallPhase::Complete {
            format!("Phase Animator {latest_version} is installed.")
        } else {
            "Compare your installed version with the latest release.".to_owned()
        };

        ui.add_space(8.0);
        ui.label(
            RichText::new(state_title)
                .font(type_display(22.0))
                .color(phase::text()),
        );
        ui.add_space(5.0);
        ui.add(
            egui::Label::new(RichText::new(state_detail).font(type_body()).color(
                if self.phase == InstallPhase::Error {
                    phase::red()
                } else {
                    phase::text_secondary()
                },
            ))
            .wrap(true),
        );

        if self.is_busy() {
            ui.add_space(14.0);
            progress_rail(ui, self.progress, phase_color(self.phase));
        }

        ui.add_space(24.0);
        compact_facts_v3(
            ui,
            &[("INSTALLED", installed_version), ("LATEST", latest_version)],
        );
    }

    fn install_action_bar(&mut self, ui: &mut Ui) {
        let busy = self.is_busy();
        action_grid(ui, 2, |ui, index, size| match index {
            0 => {
                ui.add_enabled_ui(!busy, |ui| {
                    if secondary_button(ui, MiniIcon::Refresh, "Check for updates", size).clicked()
                    {
                        self.start_check();
                    }
                });
            }
            _ => {
                ui.add_enabled_ui(self.phase == InstallPhase::Ready && !busy, |ui| {
                    if primary_button(ui, MiniIcon::Download, "Install update", size).clicked() {
                        self.start_install();
                    }
                });
            }
        });
    }

    fn account_workspace_v3(&mut self, ui: &mut Ui) {
        let width = ui.available_width();
        let phase_linked = self.plugin_token.is_some();
        let phase_busy = self.link_rx.is_some()
            || self.link_status_rx.is_some()
            || self.account_refresh_rx.is_some();
        let oauth_busy = self.roblox_oauth_rx.is_some() || self.roblox_oauth_status_rx.is_some();
        let roblox_verified = !self.roblox_user_id.trim().is_empty();
        let access_ready = self
            .activation
            .as_ref()
            .is_some_and(|activation| activation.ok && activation.active);
        let phase_name = self
            .linked_user
            .as_ref()
            .map(display_linked_user)
            .unwrap_or_else(|| "No Phase account connected".to_owned());

        let access_detail = if let Some(activation) = &self.activation {
            let source = match activation.activation_mode.as_str() {
                "phaseAccount" => "Phase account",
                "robloxPurchase" => "Roblox ownership",
                "licenseKey" => "License key",
                _ => "Verified access",
            };
            format!("{} · {source}", activation.licensee)
        } else if phase_busy || oauth_busy {
            "Waiting for browser approval".to_owned()
        } else if phase_linked {
            "Refresh the connected account to verify access".to_owned()
        } else {
            "Connect Phase to verify access in one step".to_owned()
        };

        workspace_section(
            ui,
            "Install access",
            "One verified account is required before an update can be installed.",
        );
        identity_line_v3(
            ui,
            MiniIcon::ShieldCheck,
            if access_ready {
                "Ready to install"
            } else if phase_busy || oauth_busy {
                "Verification in progress"
            } else {
                "Connect to continue"
            },
            &access_detail,
            if access_ready {
                ("Ready", phase::green())
            } else if phase_busy || oauth_busy {
                ("Waiting", phase::blue())
            } else {
                ("Required", phase::warning())
            },
        );
        if !access_ready {
            ui.add_space(10.0);
            let action_count = if !phase_linked && self.link_url.is_some() {
                2
            } else {
                1
            };
            action_grid(ui, action_count, |ui, index, size| {
                if phase_linked {
                    ui.add_enabled_ui(!phase_busy, |ui| {
                        if primary_button(ui, MiniIcon::Refresh, "Refresh access", size).clicked() {
                            self.begin_phase_account_refresh(ui.ctx());
                        }
                    });
                } else if index == 0 {
                    ui.add_enabled_ui(!phase_busy, |ui| {
                        if primary_button(ui, MiniIcon::Link, "Connect Phase", size).clicked() {
                            self.start_phase_account_link(ui.ctx());
                        }
                    });
                } else if secondary_button(ui, MiniIcon::External, "Open browser", size).clicked() {
                    if let Some(url) = self.link_url.clone() {
                        if let Err(error) = open::that(url) {
                            self.log(phase::warning(), format!("Open browser failed: {error}"));
                        }
                    }
                }
            });
        }
        if let Some(error) = &self.activation_error {
            ui.add_space(8.0);
            ui.label(RichText::new(error).font(type_label()).color(phase::red()));
        }

        ui.add_space(18.0);
        egui::CollapsingHeader::new("Manage connections")
            .id_source("account-connections-v4")
            .default_open(false)
            .show(ui, |ui| {
                ui.add_space(8.0);
                identity_line_v3(
                    ui,
                    MiniIcon::User,
                    &phase_name,
                    "Phase account",
                    if phase_linked {
                        ("Connected", phase::green())
                    } else {
                        ("Not connected", phase::text_muted())
                    },
                );
                ui.add_space(6.0);
                if phase_linked {
                    action_grid(ui, 1, |ui, _, size| {
                        ui.add_enabled_ui(self.phase_disconnect_rx.is_none(), |ui| {
                            if danger_button(ui, MiniIcon::Trash, "Disconnect Phase", size)
                                .clicked()
                            {
                                self.start_phase_disconnect(ui.ctx());
                            }
                        });
                    });
                } else if access_ready {
                    action_grid(ui, 1, |ui, _, size| {
                        ui.add_enabled_ui(!phase_busy, |ui| {
                            if secondary_button(ui, MiniIcon::Link, "Connect Phase", size).clicked()
                            {
                                self.start_phase_account_link(ui.ctx());
                            }
                        });
                    });
                }

                ui.add_space(18.0);
                let roblox_identity = self
                    .roblox_username
                    .as_deref()
                    .filter(|name| !name.trim().is_empty())
                    .unwrap_or(if roblox_verified {
                        self.roblox_user_id.as_str()
                    } else {
                        "No Roblox account verified"
                    });
                identity_line_v3(
                    ui,
                    MiniIcon::ShieldCheck,
                    roblox_identity,
                    "Roblox ownership",
                    if roblox_verified {
                        ("Verified", phase::green())
                    } else if oauth_busy {
                        ("Waiting", phase::blue())
                    } else {
                        ("Not verified", phase::text_muted())
                    },
                );
                ui.add_space(6.0);
                if roblox_verified {
                    action_grid(ui, 1, |ui, _, size| {
                        if danger_button(ui, MiniIcon::Trash, "Disconnect Roblox", size).clicked() {
                            self.disconnect_roblox_account();
                        }
                    });
                    if !access_ready {
                        ui.add_space(14.0);
                        ui.label(
                            RichText::new("License key")
                                .font(type_label())
                                .color(phase::text_secondary()),
                        );
                        ui.add_space(4.0);
                        ui.add(
                            egui::TextEdit::singleline(&mut self.license_key)
                                .desired_width(width)
                                .password(true)
                                .hint_text("Enter a Phase license key"),
                        );
                        ui.add_space(8.0);
                        action_grid(ui, 1, |ui, _, size| {
                            ui.add_enabled_ui(self.activation_rx.is_none(), |ui| {
                                if primary_button(ui, MiniIcon::Key, "Activate license", size)
                                    .clicked()
                                {
                                    self.start_activation(ui.ctx());
                                }
                            });
                        });
                    }
                } else {
                    let action_count = if self.roblox_oauth_url.is_some() {
                        2
                    } else {
                        1
                    };
                    action_grid(ui, action_count, |ui, index, size| {
                        if index == 0 {
                            ui.add_enabled_ui(!oauth_busy, |ui| {
                                if secondary_button(
                                    ui,
                                    MiniIcon::ShieldCheck,
                                    "Verify Roblox instead",
                                    size,
                                )
                                .clicked()
                                {
                                    self.start_roblox_oauth(ui.ctx());
                                }
                            });
                        } else if secondary_button(ui, MiniIcon::External, "Open browser", size)
                            .clicked()
                        {
                            if let Some(url) = self.roblox_oauth_url.clone() {
                                let _ = open::that(url);
                            }
                        }
                    });
                }
            });
    }

    fn folders_workspace_v3(&mut self, ui: &mut Ui) {
        workspace_section(ui, "Install folder", "");
        let selected_path = self
            .selected_folder
            .as_ref()
            .map(|path| path.to_string_lossy().to_string())
            .unwrap_or_else(|| "No folder selected".to_owned());
        let selected_health = self
            .selected_candidate()
            .map(|candidate| candidate.health.clone());
        let selected_ready = selected_health == Some(FolderHealth::Ready);
        identity_line_v3(
            ui,
            MiniIcon::Folder,
            if self.selected_folder.is_some() {
                "Roblox Studio plugins"
            } else {
                "Choose a folder to continue"
            },
            &selected_path,
            match selected_health.as_ref() {
                Some(FolderHealth::Ready) => ("Ready", phase::green()),
                Some(FolderHealth::Empty) => ("Empty", phase::warning()),
                Some(FolderHealth::Missing) => ("Missing", phase::red()),
                None => ("Required", phase::warning()),
            },
        );

        ui.add_space(10.0);
        if self.selected_folder.is_some() {
            action_grid(ui, 2, |ui, index, size| {
                if index == 0 {
                    if secondary_button(ui, MiniIcon::Folder, "Change folder", size).clicked() {
                        self.choose_folder();
                    }
                } else if secondary_button(ui, MiniIcon::Eye, "Open folder", size).clicked() {
                    self.open_folder();
                }
            });
        } else {
            action_grid(ui, 1, |ui, _, size| {
                if primary_button(ui, MiniIcon::Folder, "Choose folder", size).clicked() {
                    self.choose_folder();
                }
            });
        }

        let selected_normalized = self
            .selected_folder
            .as_ref()
            .map(|path| normalize_path(path));
        let other_candidates: Vec<_> = self
            .candidates
            .iter()
            .filter(|candidate| {
                selected_normalized
                    .as_ref()
                    .is_none_or(|selected| normalize_path(&candidate.path) != *selected)
            })
            .cloned()
            .collect();
        ui.add_space(18.0);
        egui::CollapsingHeader::new(format!(
            "Other detected folders ({})",
            other_candidates.len()
        ))
        .id_source("other-plugin-folders-v4")
        .default_open(self.selected_folder.is_none() || !selected_ready)
        .show(ui, |ui| {
            ui.add_space(8.0);
            if other_candidates.is_empty() {
                ui.label(
                    RichText::new("No other Studio plugin folders were found.")
                        .font(type_label())
                        .color(phase::text_muted()),
                );
            } else {
                for candidate in other_candidates {
                    let response = selection_line_v3(
                        ui,
                        &candidate.path.to_string_lossy(),
                        &candidate.source,
                        false,
                        health_label(&candidate.health),
                    );
                    if response.clicked() {
                        self.selected_folder = Some(candidate.path.clone());
                        self.refresh_local_release_status();
                    }
                }
            }
            ui.add_space(8.0);
            action_grid(ui, 1, |ui, _, size| {
                if secondary_button(ui, MiniIcon::Refresh, "Rescan Studio folders", size).clicked()
                {
                    self.refresh_detection();
                }
            });
        });
    }

    fn video_workspace_v3(&mut self, ui: &mut Ui) {
        let width = ui.available_width();
        workspace_section(
            ui,
            "Reference source",
            "A YouTube URL or local video sent to the Studio timeline.",
        );
        ui.label(
            RichText::new("VIDEO")
                .font(type_caption())
                .color(phase::text_muted()),
        );
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = PHASE_GRID_GAP;
            let browse_w = phase_action_content_width(ui, "Browse");
            let source = ui.add(
                egui::TextEdit::singleline(&mut self.video_source)
                    .desired_width((width - browse_w - PHASE_GRID_GAP).max(100.0))
                    .hint_text(
                        RichText::new("YouTube URL or local video path").color(phase::text_muted()),
                    ),
            );
            if source.changed() && self.video_title.trim().is_empty() {
                self.video_title = video_reference::default_title_for(&self.video_source);
            }
            if secondary_button(
                ui,
                MiniIcon::Folder,
                "Browse",
                Vec2::new(browse_w, PHASE_COMPACT_ACTION_HEIGHT),
            )
            .clicked()
            {
                self.pick_video_file();
            }
        });
        ui.add_space(10.0);
        ui.label(
            RichText::new("DISPLAY NAME")
                .font(type_caption())
                .color(phase::text_muted()),
        );
        ui.add_space(4.0);
        ui.add(
            egui::TextEdit::singleline(&mut self.video_title)
                .desired_width(width)
                .hint_text(RichText::new("Optional reference name").color(phase::text_muted())),
        );
        ui.add_space(12.0);
        if primary_button(
            ui,
            MiniIcon::Link,
            "Send reference to Studio",
            Vec2::new(width, PHASE_PROMINENT_ACTION_HEIGHT),
        )
        .clicked()
        {
            self.send_video_reference();
        }
        ui.add_space(8.0);
        action_grid(ui, 3, |ui, index, size| match index {
            0 => {
                if secondary_button(ui, MiniIcon::External, "Open viewer", size).clicked() {
                    self.open_video_popup();
                }
            }
            1 => {
                let sync_label = if self.video_sync_enabled {
                    "Sync on"
                } else {
                    "Sync off"
                };
                if choice_button(
                    ui,
                    if self.video_sync_enabled {
                        MiniIcon::Check
                    } else {
                        MiniIcon::Pause
                    },
                    sync_label,
                    self.video_sync_enabled,
                    size,
                )
                .clicked()
                {
                    self.video_sync_enabled = !self.video_sync_enabled;
                    self.send_video_sync_enabled();
                }
            }
            _ => {
                if secondary_button(ui, MiniIcon::Trash, "Clear", size).clicked() {
                    self.clear_video_reference();
                }
            }
        });

        ui.add_space(24.0);
        workspace_section(
            ui,
            "Studio connection",
            "Current bridge and timeline state.",
        );
        identity_line_v3(
            ui,
            MiniIcon::Broadcast,
            if self.video_bridge_connected {
                "Studio bridge connected"
            } else {
                "Waiting for Studio"
            },
            &self.video_last_plugin_state,
            if self.video_bridge_connected {
                ("Connected", phase::green())
            } else {
                ("Listening", phase::blue())
            },
        );
        ui.add_space(8.0);
        egui::CollapsingHeader::new("Timing and playback")
            .id_source("video-timing-v3")
            .default_open(false)
            .show(ui, |ui| {
                ui.add_space(6.0);
                let field_w =
                    phase_grid_cell_width(ui.available_width(), 3, PHASE_GRID_GAP).max(72.0);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = PHASE_GRID_GAP;
                    small_number_field(
                        ui,
                        "Duration",
                        &mut self.video_duration_seconds,
                        "0",
                        field_w,
                    );
                    small_number_field(ui, "FPS", &mut self.video_fps, "60", field_w);
                    small_number_field(ui, "Start", &mut self.video_start_frame, "0", field_w);
                });
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = PHASE_GRID_GAP;
                    small_number_field(ui, "Offset", &mut self.video_offset_seconds, "0", field_w);
                    small_number_field(ui, "Rate", &mut self.video_playback_rate, "1", field_w);
                });
            });
        egui::CollapsingHeader::new("Advanced bridge controls")
            .id_source("video-bridge-v3")
            .default_open(false)
            .show(ui, |ui| {
                ui.add_space(6.0);
                ui.label(
                    RichText::new(self.video_bridge_config.url())
                        .font(FontId::monospace(11.0))
                        .color(phase::text_secondary()),
                );
                ui.add_space(8.0);
                ui.add(
                    egui::TextEdit::singleline(&mut self.video_bridge_config.token)
                        .desired_width(ui.available_width())
                        .password(true)
                        .hint_text("Optional bridge token"),
                );
                ui.add_space(8.0);
                action_grid(ui, 3, |ui, index, size| match index {
                    0 => {
                        if secondary_button(ui, MiniIcon::Refresh, "Restart", size).clicked() {
                            self.restart_video_bridge();
                        }
                    }
                    1 => {
                        if secondary_button(ui, MiniIcon::PlugsConnected, "Ping", size).clicked() {
                            self.send_video_ping();
                        }
                    }
                    _ => {
                        if secondary_button(ui, MiniIcon::Check, "Apply", size).clicked() {
                            self.send_video_sync_enabled();
                        }
                    }
                });
            });
    }

    fn settings_workspace_v3(&mut self, ui: &mut Ui) {
        workspace_section(
            ui,
            "Appearance",
            "Choose the theme and background treatment.",
        );
        let current_theme = self
            .selected_theme
            .as_ref()
            .map(|theme| theme.title.as_str())
            .unwrap_or("Default Phase");
        identity_line_v3(
            ui,
            MiniIcon::Palette,
            current_theme,
            "Active theme",
            ("", Color32::TRANSPARENT),
        );
        ui.add_space(6.0);
        if let Some(mode) = theme_mode_rail_v3(ui, self.theme_background_mode) {
            self.theme_background_mode = mode;
            self.save_account_cache();
        }
        ui.add_space(10.0);
        let preference_summary = format!(
            "Install preferences · Backup {} · Reminder {}",
            if self.backup_before_install {
                "on"
            } else {
                "off"
            },
            if self.restart_studio_hint {
                "on"
            } else {
                "off"
            }
        );
        egui::CollapsingHeader::new(preference_summary)
            .id_source("install-preferences-v4")
            .default_open(false)
            .show(ui, |ui| {
                ui.add_space(6.0);
                toggle_line_v3(
                    ui,
                    &mut self.backup_before_install,
                    "Back up before install",
                );
                toggle_line_v3(
                    ui,
                    &mut self.restart_studio_hint,
                    "Remind me to restart Studio",
                );
            });
        ui.add_space(8.0);
        self.theme_search_control_v3(ui);
        let matching_themes: Vec<_> = self
            .theme_assets
            .iter()
            .filter(|asset| theme_matches_search(asset, &self.theme_search))
            .cloned()
            .collect();
        let visible_count = self.visible_theme_count.min(matching_themes.len());
        if self.theme_assets.is_empty() && self.theme_fetch_rx.is_some() {
            ui.add_space(10.0);
            ui.label(
                RichText::new("Loading marketplace themes…")
                    .font(type_label())
                    .color(phase::text_muted()),
            );
        } else if matching_themes.is_empty() {
            ui.add_space(10.0);
            ui.label(
                RichText::new(if self.theme_search.trim().is_empty() {
                    "No marketplace themes are available."
                } else {
                    "No themes match this search."
                })
                .font(type_label())
                .color(phase::text_muted()),
            );
        } else {
            ui.add_space(6.0);
            for asset in matching_themes.iter().take(visible_count).cloned() {
                self.theme_line_v3(ui, asset);
            }
            if visible_count < matching_themes.len() {
                ui.add_space(6.0);
                action_grid(ui, 1, |ui, _, size| {
                    if secondary_button(ui, MiniIcon::Download, "Show more themes", size).clicked()
                    {
                        self.visible_theme_count =
                            (self.visible_theme_count + 6).min(matching_themes.len());
                    }
                });
            }
        }

        ui.add_space(PHASE_GRID_GAP);
        action_grid(ui, 2, |ui, index, size| match index {
            0 => {
                ui.add_enabled_ui(self.theme_fetch_rx.is_none(), |ui| {
                    if secondary_button(ui, MiniIcon::Refresh, "Refresh catalog", size).clicked() {
                        self.begin_theme_fetch(ui.ctx());
                    }
                });
            }
            _ => {
                if secondary_button(ui, MiniIcon::Refresh, "Restore default", size).clicked() {
                    self.reset_theme(ui.ctx());
                }
            }
        });

        ui.add_space(18.0);
        egui::CollapsingHeader::new("Maintenance and diagnostics")
            .id_source("settings-maintenance-v4")
            .default_open(false)
            .show(ui, |ui| {
                ui.add_space(10.0);
                workspace_section(
                    ui,
                    "Companion version",
                    "This desktop app updates separately from the plugin.",
                );
                let companion_busy =
                    self.app_update_rx.is_some() || self.app_update_install_rx.is_some();
                let companion_detail = if let Some(update) = &self.app_update {
                    format!("Version {} is available", update.version)
                } else if companion_busy {
                    "Checking for an update…".to_owned()
                } else if let Some(error) = &self.app_update_error {
                    error.clone()
                } else {
                    "No update is currently available".to_owned()
                };
                identity_line_v3(
                    ui,
                    MiniIcon::CloudArrowDown,
                    &format!("Phase Companion {}", env!("CARGO_PKG_VERSION")),
                    &companion_detail,
                    if self.app_update.is_some() {
                        ("Update", phase::warning())
                    } else if companion_busy {
                        ("Checking", phase::blue())
                    } else {
                        ("Current", phase::green())
                    },
                );
                ui.add_space(8.0);
                action_grid(ui, 2, |ui, index, size| {
                    if index == 0 {
                        ui.add_enabled_ui(!companion_busy, |ui| {
                            if secondary_button(ui, MiniIcon::Refresh, "Check for update", size)
                                .clicked()
                            {
                                self.begin_app_update_check(ui.ctx());
                            }
                        });
                    } else {
                        ui.add_enabled_ui(
                            self.app_update.is_some() && self.app_update_install_rx.is_none(),
                            |ui| {
                                if primary_button(ui, MiniIcon::Download, "Install update", size)
                                    .clicked()
                                {
                                    self.start_app_update_install(ui.ctx());
                                }
                            },
                        );
                    }
                });

                ui.add_space(22.0);
                workspace_section(
                    ui,
                    "Connection check",
                    "Use this only when Phase cannot connect or install.",
                );
                let diagnostic_running =
                    self.diagnostics_rx.is_some() || self.diagnostics_fix_rx.is_some();
                let diagnostic_problem = self.diagnostics_report.as_ref().is_some_and(|report| {
                    report.overall_status() != diagnostics::DiagnosticStatus::Good
                });
                identity_line_v3(
                    ui,
                    MiniIcon::Search,
                    if diagnostic_running {
                        "Checking connections"
                    } else if let Some(report) = &self.diagnostics_report {
                        report.summary.as_str()
                    } else {
                        "No connection check has been run"
                    },
                    if diagnostic_problem {
                        "A repair path is available"
                    } else {
                        "Checks Phase services and local folder access"
                    },
                    if diagnostic_running {
                        ("Working", phase::blue())
                    } else if diagnostic_problem {
                        ("Problem", phase::red())
                    } else if self.diagnostics_report.is_some() {
                        ("Good", phase::green())
                    } else {
                        ("", Color32::TRANSPARENT)
                    },
                );
                ui.add_space(8.0);
                let diagnostic_actions = if diagnostic_problem {
                    3
                } else if self.diagnostics_report.is_some() {
                    2
                } else {
                    1
                };
                action_grid(ui, diagnostic_actions, |ui, index, size| {
                    if index == 0 {
                        ui.add_enabled_ui(!diagnostic_running, |ui| {
                            if secondary_button(ui, MiniIcon::Search, "Run check", size).clicked() {
                                self.start_connection_diagnostics(ui.ctx());
                            }
                        });
                    } else if diagnostic_problem && index == 1 {
                        ui.add_enabled_ui(!diagnostic_running, |ui| {
                            if primary_button(ui, MiniIcon::Gear, "Repair", size).clicked() {
                                self.start_connection_fix(ui.ctx());
                            }
                        });
                    } else if secondary_button(ui, MiniIcon::External, "Open report", size).clicked()
                    {
                        self.diagnostics_open = true;
                    }
                });

                ui.add_space(18.0);
                egui::CollapsingHeader::new("Reset plugin data")
                    .id_source("settings-reset-data-v4")
                    .default_open(false)
                    .show(ui, |ui| {
                        ui.add_space(8.0);
                        ui.label(
                            RichText::new(
                                "Backs up selected Phase settings, then removes them from Studio.",
                            )
                            .font(type_label())
                            .color(phase::text_muted()),
                        );
                        ui.add_space(8.0);
                        compact_facts_v3(
                            ui,
                            &[
                                (
                                    "FILES",
                                    self.plugin_settings_inventory
                                        .files_with_phase_keys
                                        .to_string(),
                                ),
                                (
                                    "THEMES",
                                    self.plugin_settings_inventory.theme_keys.to_string(),
                                ),
                                (
                                    "KEYBINDS",
                                    self.plugin_settings_inventory.keybind_keys.to_string(),
                                ),
                            ],
                        );
                        ui.add_space(8.0);
                        switch_row_v3(
                            ui,
                            &mut self.plugin_settings_reset_themes,
                            "Theme data",
                            "Include saved Phase themes.",
                        );
                        switch_row_v3(
                            ui,
                            &mut self.plugin_settings_reset_keybinds,
                            "Keybind data",
                            "Include saved Phase shortcuts.",
                        );
                        ui.add_space(8.0);
                        if self.plugin_data_reset_confirm {
                            ui.label(
                                RichText::new(
                                    "Close Roblox Studio first. A backup is created before deletion.",
                                )
                                .font(type_label())
                                .color(phase::warning()),
                            );
                            ui.add_space(8.0);
                            action_grid(ui, 2, |ui, index, size| {
                                if index == 0 {
                                    if secondary_button(ui, MiniIcon::External, "Cancel", size)
                                        .clicked()
                                    {
                                        self.plugin_data_reset_confirm = false;
                                    }
                                } else if danger_button(
                                    ui,
                                    MiniIcon::Trash,
                                    "Back up and delete",
                                    size,
                                )
                                .clicked()
                                {
                                    self.reset_phase_plugin_data();
                                }
                            });
                        } else {
                            action_grid(ui, 2, |ui, index, size| {
                                if index == 0 {
                                    if secondary_button(
                                        ui,
                                        MiniIcon::Refresh,
                                        "Rescan data",
                                        size,
                                    )
                                    .clicked()
                                    {
                                        self.plugin_settings_inventory =
                                            phase_plugin_settings_inventory();
                                        self.plugin_data_reset_status =
                                            Some("Studio plugin data rescanned.".to_owned());
                                    }
                                } else if danger_button(
                                    ui,
                                    MiniIcon::Trash,
                                    "Reset selected data",
                                    size,
                                )
                                .clicked()
                                {
                                    self.plugin_data_reset_confirm = true;
                                }
                            });
                        }
                        if let Some(status) = &self.plugin_data_reset_status {
                            ui.add_space(6.0);
                            ui.label(
                                RichText::new(status)
                                    .font(type_label())
                                    .color(phase::text_muted()),
                            );
                        }
                    });
            });
    }

    fn theme_search_control_v3(&mut self, ui: &mut Ui) {
        let width = ui.available_width();
        let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 38.0), Sense::hover());
        let text_rect = Rect::from_min_max(
            Pos2::new(rect.left() + 34.0, rect.top() + 5.0),
            Pos2::new(rect.right() - 34.0, rect.bottom() - 5.0),
        );
        ui.painter()
            .rect_filled(rect, Rounding::same(6.0), phase::input());
        let text_response = ui
            .allocate_ui_at_rect(text_rect, |ui| {
                ui.visuals_mut().widgets.inactive.bg_fill = Color32::TRANSPARENT;
                ui.visuals_mut().widgets.hovered.bg_fill = Color32::TRANSPARENT;
                ui.visuals_mut().widgets.active.bg_fill = Color32::TRANSPARENT;
                ui.visuals_mut().widgets.inactive.bg_stroke = Stroke::NONE;
                ui.visuals_mut().widgets.hovered.bg_stroke = Stroke::NONE;
                ui.visuals_mut().widgets.active.bg_stroke = Stroke::NONE;
                ui.add_sized(
                    text_rect.size(),
                    egui::TextEdit::singleline(&mut self.theme_search)
                        .font(type_body())
                        .frame(false)
                        .hint_text(
                            RichText::new("Search marketplace themes").color(phase::text_muted()),
                        ),
                )
            })
            .inner;
        let focused = text_response.has_focus();
        ui.painter().rect_stroke(
            rect,
            Rounding::same(6.0),
            Stroke::new(
                if focused { 1.5 } else { 1.0 },
                if focused {
                    phase::accent_hover()
                } else {
                    phase::line()
                },
            ),
        );
        draw_icon_at(
            ui.painter(),
            Rect::from_center_size(
                Pos2::new(rect.left() + 18.0, rect.center().y),
                Vec2::splat(15.0),
            ),
            MiniIcon::Search,
            if focused {
                phase::text()
            } else {
                phase::text_muted()
            },
        );
        if !self.theme_search.is_empty() {
            let clear_rect = Rect::from_center_size(
                Pos2::new(rect.right() - 18.0, rect.center().y),
                Vec2::splat(24.0),
            );
            let clear = ui.interact(
                clear_rect,
                ui.make_persistent_id("clear-theme-search-v3"),
                Sense::click(),
            );
            ui.painter().text(
                clear_rect.center(),
                Align2::CENTER_CENTER,
                "×",
                FontId::proportional(15.0),
                if clear.hovered() {
                    phase::text()
                } else {
                    phase::text_muted()
                },
            );
            if clear.clicked() {
                self.theme_search.clear();
                self.visible_theme_count = 6;
                text_response.request_focus();
            }
        }
        if text_response.changed() {
            self.visible_theme_count = 6;
        }
    }

    fn theme_line_v3(&mut self, ui: &mut Ui, asset: verification::PhaseThemeAsset) {
        self.ensure_theme_preview_fetch(ui.ctx(), &asset);
        let preview = self.theme_preview_textures.get(&asset.id).cloned();
        let width = ui.available_width();
        let (rect, response) = ui.allocate_exact_size(Vec2::new(width, 52.0), Sense::hover());
        let tint =
            phase::hex_color(asset.theme_preview.accent.trim()).unwrap_or_else(phase::accent);
        if response.hovered() {
            ui.painter().rect_filled(
                rect,
                Rounding::ZERO,
                lerp_color(phase::background(), phase::surface_hover(), 0.42),
            );
        }
        let preview_rect = Rect::from_center_size(
            Pos2::new(rect.left() + 29.0, rect.center().y),
            Vec2::new(54.0, 32.0),
        );
        if let Some(texture) = preview {
            let (_, uv) = theme_background_layout(
                preview_rect,
                texture.size_vec2(),
                ThemeBackgroundMode::Crop,
            );
            ui.painter()
                .image(texture.id(), preview_rect, uv, Color32::WHITE);
        } else {
            let fallback = phase::hex_color(asset.theme_preview.background.trim())
                .unwrap_or_else(phase::input);
            ui.painter()
                .rect_filled(preview_rect, Rounding::same(4.0), fallback);
        }
        ui.painter().rect_stroke(
            preview_rect,
            Rounding::same(4.0),
            Stroke::new(1.0, color_with_alpha(tint, 0.72)),
        );
        ui.painter().text(
            Pos2::new(rect.left() + 68.0, rect.center().y - 7.0),
            Align2::LEFT_CENTER,
            &asset.title,
            type_display(11.5),
            phase::text(),
        );
        let owner = asset
            .owner
            .as_ref()
            .map(display_theme_owner)
            .unwrap_or_else(|| "Phase marketplace".to_owned());
        ui.painter().text(
            Pos2::new(rect.left() + 68.0, rect.center().y + 9.0),
            Align2::LEFT_CENTER,
            format!("{} · {} installs", owner, asset.install_count),
            FontId::proportional(9.5),
            phase::text_muted(),
        );
        let apply_width = phase_action_content_width(ui, "Apply");
        ui.allocate_ui_at_rect(
            Rect::from_min_size(
                Pos2::new(rect.right() - apply_width, rect.top() + 7.0),
                Vec2::new(apply_width, PHASE_COMPACT_ACTION_HEIGHT),
            ),
            |ui| {
                let busy = self.theme_apply_rx.is_some() || self.theme_transition.is_some();
                ui.add_enabled_ui(!busy, |ui| {
                    if secondary_button(
                        ui,
                        MiniIcon::Download,
                        "Apply",
                        Vec2::new(apply_width, PHASE_COMPACT_ACTION_HEIGHT),
                    )
                    .clicked()
                    {
                        self.start_theme_apply(ui.ctx(), asset);
                    }
                });
            },
        );
        ui.painter().hline(
            rect.left()..=rect.right(),
            rect.bottom(),
            Stroke::new(1.0, color_with_alpha(phase::line(), 0.34)),
        );
    }
}

fn theme_mode_rail_v3(ui: &mut Ui, current: ThemeBackgroundMode) -> Option<ThemeBackgroundMode> {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 34.0), Sense::hover());
    ui.painter()
        .rect_filled(rect, Rounding::same(6.0), phase::input());
    ui.painter().rect_stroke(
        rect,
        Rounding::same(6.0),
        Stroke::new(1.0, color_with_alpha(phase::line(), 0.62)),
    );
    let modes = [
        ThemeBackgroundMode::Crop,
        ThemeBackgroundMode::Fit,
        ThemeBackgroundMode::Stretch,
    ];
    let cell_width = rect.width() / modes.len() as f32;
    let mut selected_mode = None;
    for (index, mode) in modes.iter().enumerate() {
        let cell = Rect::from_min_size(
            Pos2::new(rect.left() + cell_width * index as f32, rect.top()),
            Vec2::new(cell_width, rect.height()),
        );
        let response = ui.interact(
            cell,
            ui.make_persistent_id(("theme-background-mode-v3", index)),
            Sense::click(),
        );
        let selected = *mode == current;
        let hover = hover_t(ui, response.id, response.hovered());
        if selected || hover > 0.0 {
            ui.painter().rect_filled(
                cell.shrink(3.0),
                Rounding::same(4.0),
                if selected {
                    lerp_color(phase::input(), phase::surface_active(), 0.72)
                } else {
                    lerp_color(phase::input(), phase::surface_hover(), hover)
                },
            );
        }
        if index > 0 {
            ui.painter().vline(
                cell.left(),
                cell.top() + 9.0..=cell.bottom() - 9.0,
                Stroke::new(1.0, color_with_alpha(phase::line(), 0.42)),
            );
        }
        let label = mode.label();
        let label_width = text_width(ui, label, type_label());
        let group_width = label_width + if selected { 22.0 } else { 0.0 };
        let left = cell.center().x - group_width * 0.5;
        if selected {
            draw_icon_at(
                ui.painter(),
                Rect::from_center_size(Pos2::new(left + 7.0, cell.center().y), Vec2::splat(14.0)),
                MiniIcon::Check,
                phase::text(),
            );
        }
        ui.painter().text(
            Pos2::new(left + if selected { 22.0 } else { 0.0 }, cell.center().y),
            Align2::LEFT_CENTER,
            label,
            if selected {
                type_display(11.5)
            } else {
                type_label()
            },
            if selected {
                phase::text()
            } else {
                phase::text_secondary()
            },
        );
        if response.has_focus() {
            ui.painter().rect_stroke(
                cell.shrink(3.0),
                Rounding::same(4.0),
                Stroke::new(1.0, phase::accent_hover()),
            );
        }
        if response.clicked() {
            selected_mode = Some(*mode);
        }
        if response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
    }
    selected_mode
}

fn workspace_section(ui: &mut Ui, title: &str, detail: &str) {
    ui.label(
        RichText::new(title)
            .font(type_display(14.0))
            .color(phase::text()),
    );
    if !detail.is_empty() {
        ui.add_space(2.0);
        ui.label(
            RichText::new(detail)
                .font(type_label())
                .color(phase::text_muted()),
        );
    }
    ui.add_space(8.0);
}

fn identity_line_v3(
    ui: &mut Ui,
    icon: MiniIcon,
    title: &str,
    detail: &str,
    status: (&str, Color32),
) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 46.0), Sense::hover());
    draw_icon_at(
        ui.painter(),
        Rect::from_center_size(
            Pos2::new(rect.left() + 13.0, rect.center().y),
            Vec2::splat(18.0),
        ),
        icon,
        phase::accent_hover(),
    );
    let status_width = if status.0.is_empty() {
        0.0
    } else {
        text_width(ui, status.0, type_label()) + 18.0
    };
    let text_clip = Rect::from_min_max(
        Pos2::new(rect.left() + 34.0, rect.top()),
        Pos2::new(rect.right() - status_width - 8.0, rect.bottom()),
    );
    let painter = ui.painter().with_clip_rect(text_clip);
    painter.text(
        Pos2::new(text_clip.left(), rect.center().y - 9.0),
        Align2::LEFT_CENTER,
        title,
        type_display(13.0),
        phase::text(),
    );
    painter.text(
        Pos2::new(text_clip.left(), rect.center().y + 10.0),
        Align2::LEFT_CENTER,
        detail,
        FontId::proportional(10.5),
        phase::text_muted(),
    );
    if !status.0.is_empty() {
        ui.painter().circle_filled(
            Pos2::new(rect.right() - status_width + 5.0, rect.center().y),
            3.5,
            status.1,
        );
        ui.painter().text(
            Pos2::new(rect.right() - 4.0, rect.center().y),
            Align2::RIGHT_CENTER,
            status.0,
            type_label(),
            phase::text_secondary(),
        );
    }
}

fn toggle_line_v3(ui: &mut Ui, value: &mut bool, title: &str) {
    switch_row_v3(ui, value, title, "");
}

fn switch_row_v3(ui: &mut Ui, value: &mut bool, title: &str, detail: &str) {
    let width = ui.available_width();
    let compact = detail.is_empty();
    let height = if compact { 36.0 } else { 44.0 };
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, height), Sense::click());
    let hover = hover_t(ui, response.id, response.hovered());
    if hover > 0.0 {
        ui.painter().rect_filled(
            rect,
            Rounding::same(7.0),
            lerp_color(phase::surface(), phase::surface_hover(), hover),
        );
    }
    ui.painter().text(
        Pos2::new(
            rect.left() + 2.0,
            rect.center().y + if compact { 0.0 } else { -8.0 },
        ),
        Align2::LEFT_CENTER,
        title,
        type_display(12.0),
        phase::text(),
    );
    if !compact {
        ui.painter().text(
            Pos2::new(rect.left() + 2.0, rect.center().y + 10.0),
            Align2::LEFT_CENTER,
            detail,
            FontId::proportional(10.5),
            phase::text_muted(),
        );
    }
    let track = Rect::from_center_size(
        Pos2::new(rect.right() - 22.0, rect.center().y),
        Vec2::new(38.0, 20.0),
    );
    ui.painter().rect_filled(
        track,
        Rounding::same(10.0),
        if *value {
            lerp_color(phase::surface(), phase::accent(), 0.55)
        } else {
            phase::input()
        },
    );
    ui.painter().circle_filled(
        Pos2::new(
            if *value {
                track.right() - 10.0
            } else {
                track.left() + 10.0
            },
            track.center().y,
        ),
        7.0,
        if *value {
            phase::text()
        } else {
            phase::text_muted()
        },
    );
    if response.clicked() {
        *value = !*value;
    }
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
}

fn compact_facts_v3(ui: &mut Ui, items: &[(&str, String)]) {
    if items.is_empty() {
        return;
    }
    let width = ui.available_width();
    let cell_width = width / items.len() as f32;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 42.0), Sense::hover());
    for (index, (label, value)) in items.iter().enumerate() {
        let left = rect.left() + cell_width * index as f32;
        if index > 0 {
            ui.painter().vline(
                left,
                rect.top() + 7.0..=rect.bottom() - 7.0,
                Stroke::new(1.0, color_with_alpha(phase::line(), 0.45)),
            );
        }
        let clip = Rect::from_min_size(
            Pos2::new(left + 10.0, rect.top()),
            Vec2::new(cell_width - 20.0, rect.height()),
        );
        let painter = ui.painter().with_clip_rect(clip);
        painter.text(
            Pos2::new(clip.left(), rect.center().y - 8.0),
            Align2::LEFT_CENTER,
            *label,
            type_caption(),
            phase::text_muted(),
        );
        painter.text(
            Pos2::new(clip.left(), rect.center().y + 9.0),
            Align2::LEFT_CENTER,
            value,
            type_label(),
            phase::text(),
        );
    }
}

fn selection_line_v3(
    ui: &mut Ui,
    title: &str,
    detail: &str,
    selected: bool,
    status: &str,
) -> egui::Response {
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, 48.0), Sense::click());
    let hover = hover_t(ui, response.id, response.hovered());
    if selected || hover > 0.0 {
        ui.painter().rect_filled(
            rect,
            Rounding::ZERO,
            if selected {
                lerp_color(phase::background(), phase::accent_dim(), 0.14)
            } else {
                lerp_color(phase::background(), phase::surface_hover(), hover)
            },
        );
    }
    if selected {
        ui.painter().rect_filled(
            Rect::from_min_max(
                Pos2::new(rect.left(), rect.top() + 9.0),
                Pos2::new(rect.left() + 2.0, rect.bottom() - 9.0),
            ),
            Rounding::ZERO,
            phase::accent(),
        );
    }
    let clip = Rect::from_min_max(
        Pos2::new(rect.left() + 12.0, rect.top()),
        Pos2::new(rect.right() - 80.0, rect.bottom()),
    );
    let painter = ui.painter().with_clip_rect(clip);
    painter.text(
        Pos2::new(clip.left(), rect.center().y - 7.0),
        Align2::LEFT_CENTER,
        title,
        FontId::monospace(10.5),
        if selected {
            phase::text()
        } else {
            phase::text_secondary()
        },
    );
    painter.text(
        Pos2::new(clip.left(), rect.center().y + 9.0),
        Align2::LEFT_CENTER,
        detail,
        type_caption(),
        phase::text_muted(),
    );
    ui.painter().text(
        Pos2::new(rect.right() - 8.0, rect.center().y),
        Align2::RIGHT_CENTER,
        if selected { "Selected" } else { status },
        type_label(),
        if selected {
            phase::accent_hover()
        } else {
            phase::text_muted()
        },
    );
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response
}

fn navigation_row(ui: &mut Ui, selected: bool, icon: MiniIcon, label: &str) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 36.0), Sense::click());
    let hover = hover_t(ui, response.id, response.hovered() || response.has_focus());
    if selected {
        ui.painter().rect_filled(
            rect,
            Rounding::same(7.0),
            color_with_alpha(phase::surface_active(), 0.78),
        );
    } else if hover > 0.0 {
        ui.painter().rect_filled(
            rect,
            Rounding::same(7.0),
            lerp_color(phase::surface(), phase::surface_hover(), hover),
        );
    }
    draw_icon_at(
        ui.painter(),
        Rect::from_center_size(
            Pos2::new(rect.left() + 18.0, rect.center().y),
            Vec2::splat(16.0),
        ),
        icon,
        if selected {
            phase::text()
        } else {
            phase::text_muted()
        },
    );
    ui.painter().text(
        Pos2::new(rect.left() + 36.0, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        if selected {
            type_display(12.0)
        } else {
            FontId::proportional(12.0)
        },
        if selected {
            phase::text()
        } else {
            phase::text_secondary()
        },
    );
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response
}

fn readiness_row(
    ui: &mut Ui,
    icon: MiniIcon,
    title: &str,
    value: &str,
    ready: bool,
    action: &str,
) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 52.0), Sense::click());
    let hover = hover_t(ui, response.id, response.hovered());
    if hover > 0.0 {
        ui.painter().rect_filled(
            rect,
            Rounding::same(8.0),
            color_with_alpha(phase::surface_hover(), 0.44 * hover),
        );
    }
    draw_icon_at(
        ui.painter(),
        Rect::from_center_size(
            Pos2::new(rect.left() + 14.0, rect.center().y),
            Vec2::splat(17.0),
        ),
        icon,
        phase::text_muted(),
    );
    ui.painter().text(
        Pos2::new(rect.left() + 32.0, rect.center().y - 8.0),
        Align2::LEFT_CENTER,
        title,
        type_display(12.0),
        phase::text(),
    );
    let value_rect = Rect::from_min_max(
        Pos2::new(rect.left() + 32.0, rect.center().y + 1.0),
        Pos2::new(rect.right() - 70.0, rect.bottom()),
    );
    ui.painter().with_clip_rect(value_rect).text(
        Pos2::new(value_rect.left(), rect.center().y + 10.0),
        Align2::LEFT_CENTER,
        value,
        FontId::proportional(10.5),
        phase::text_muted(),
    );
    let state_color = if ready {
        phase::green()
    } else {
        phase::warning()
    };
    ui.painter().circle_filled(
        Pos2::new(rect.right() - 60.0, rect.center().y),
        3.5,
        state_color,
    );
    ui.painter().text(
        Pos2::new(rect.right() - 8.0, rect.center().y),
        Align2::RIGHT_CENTER,
        action,
        type_label(),
        phase::text_secondary(),
    );
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response
}

fn progress_rail(ui: &mut Ui, progress: f32, color: Color32) {
    let (track, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 4.0), Sense::hover());
    ui.painter().rect_filled(
        track,
        Rounding::same(2.0),
        color_with_alpha(phase::line(), 0.52),
    );
    let fill = Rect::from_min_size(
        track.min,
        Vec2::new(track.width() * progress.clamp(0.03, 1.0), track.height()),
    );
    ui.painter().rect_filled(fill, Rounding::same(2.0), color);
}
