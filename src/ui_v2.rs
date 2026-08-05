use super::*;

const SHELL_X: f32 = 28.0;
const SHELL_Y: f32 = 12.0;
const TOPBAR_HEIGHT: f32 = 50.0;
const SHELL_GAP: f32 = 8.0;
const RAIL_GAP: f32 = 24.0;
const RAIL_WIDTH: f32 = 292.0;
const WIDE_LAYOUT_BREAKPOINT: f32 = 620.0;
const WORKSPACE_HEADER_HEIGHT: f32 = 68.0;
const DOCK_HEIGHT: f32 = 46.0;
const DOCK_TOP_GAP: f32 = 16.0;
const WORKSPACE_GAP: f32 = 12.0;
const WORKSPACE_ROUNDING: f32 = 20.0;
const GOLDEN_RATIO: f32 = 1.618_034;
const NEWS_PAGE_COUNT: usize = 2;
const NEWS_AUTO_ADVANCE: Duration = Duration::from_millis(6500);
const NEWS_FADE: f32 = 0.45;

impl PhaseInstallerApp {
    pub(super) fn render_companion_root(&mut self, ui: &mut Ui) {
        self.paint_theme_background(ui);

        let entrance =
            ease_out_cubic((self.ui_started_at.elapsed().as_secs_f32() / 0.62).clamp(0.0, 1.0));
        if entrance < 1.0 {
            ui.ctx().request_repaint_after(Duration::from_millis(16));
        }

        let canvas = ui.available_rect_before_wrap();
        let x_pad = SHELL_X.min((canvas.width() * 0.035).max(18.0));
        let y_pad = SHELL_Y.min((canvas.height() * 0.02).max(8.0));
        let mut shell = canvas.shrink2(Vec2::new(x_pad, y_pad));
        shell = shell.translate(Vec2::new(0.0, (1.0 - entrance) * 12.0));
        ui.allocate_rect(canvas, Sense::hover());

        ui.allocate_ui_at_rect(shell, |ui| {
            ui.set_opacity(0.28 + entrance * 0.72);
            let paired_compact = (WIDE_LAYOUT_BREAKPOINT..820.0).contains(&ui.available_width());
            if paired_compact {
                self.companion_paired_compact_body(ui);
            } else {
                self.companion_topbar(ui, None);
                ui.add_space(SHELL_GAP);
            }

            if !paired_compact && ui.available_width() >= WIDE_LAYOUT_BREAKPOINT {
                self.companion_wide_body(ui);
            } else if !paired_compact {
                self.companion_compact_body(ui);
            }
        });
    }

    fn companion_topbar(&mut self, ui: &mut Ui, profile_width_override: Option<f32>) {
        let width = ui.available_width();
        let (rect, _) = ui.allocate_exact_size(Vec2::new(width, TOPBAR_HEIGHT), Sense::hover());
        let painter = ui.painter().clone();

        let profile_width =
            profile_width_override.unwrap_or_else(|| (width * 0.285).clamp(180.0, RAIL_WIDTH));
        let capsule_inset = if profile_width_override.is_some() {
            0.0
        } else if TOPBAR_HEIGHT < 60.0 {
            3.0
        } else {
            5.0
        };
        let profile_rect = Rect::from_min_max(
            Pos2::new(rect.right() - profile_width, rect.top() + capsule_inset),
            Pos2::new(rect.right(), rect.bottom() - capsule_inset),
        );
        let profile_response = ui.interact(
            profile_rect,
            ui.make_persistent_id("companion-profile-capsule"),
            Sense::click(),
        );
        let profile_active = self.active_tab == ViewTab::Account;
        profile_response.widget_info(|| {
            egui::WidgetInfo::selected(
                egui::WidgetType::Button,
                profile_active,
                "Open accounts and access",
            )
        });
        let profile_hover = hover_t(
            ui,
            profile_response.id,
            profile_response.hovered() || profile_response.has_focus(),
        );
        let lifted_profile = profile_rect.translate(Vec2::new(0.0, -profile_hover));

        // Vivid launcher pill: bright magenta on the left easing into the deep
        // theme purple on the right, rasterized once per size/palette.
        let pill_from = glass_fill(
            if profile_active {
                phase::accent()
            } else {
                lerp_color(phase::accent_dim(), phase::accent(), 0.8)
            },
            if profile_active {
                0.72
            } else {
                GLASS_PANEL_STRONG_ALPHA
            },
        );
        let pill_to = glass_fill(
            if profile_active {
                lerp_color(phase::accent_dim(), phase::accent(), 0.35)
            } else {
                phase::accent_dim()
            },
            if profile_active {
                0.64
            } else {
                GLASS_PANEL_ALPHA
            },
        );
        let pill_texture = rounded_gradient_texture(
            &mut self.gradient_cache,
            ui.ctx(),
            "profile-pill",
            lifted_profile.size(),
            lifted_profile.height() * 0.5,
            GradientDirection::Horizontal,
            pill_from,
            pill_to,
            None,
        );
        painter.image(
            pill_texture.id(),
            lifted_profile,
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
            Color32::WHITE,
        );
        if profile_hover > 0.0 {
            painter.rect_filled(
                lifted_profile,
                Rounding::same(lifted_profile.height() * 0.5),
                color_with_alpha(Color32::WHITE, 0.09 * profile_hover),
            );
        }
        painter.rect_stroke(
            lifted_profile,
            Rounding::same(lifted_profile.height() * 0.5),
            Stroke::new(
                1.0,
                color_with_alpha(
                    lerp_color(phase::accent_hover(), phase::text(), 0.35),
                    0.5 + profile_hover * 0.35,
                ),
            ),
        );
        top_highlight(
            &painter,
            lifted_profile,
            lifted_profile.height() * 0.5,
            color_with_alpha(Color32::WHITE, 0.24),
        );

        let avatar_size = (profile_rect.height() - 6.0).clamp(34.0, 44.0);
        let avatar_rect = Rect::from_center_size(
            Pos2::new(
                profile_rect.right() - profile_rect.height() * 0.5,
                profile_rect.center().y - profile_hover,
            ),
            Vec2::splat(avatar_size),
        );
        paint_companion_avatar(
            &painter,
            avatar_rect,
            self.phase_avatar.as_ref(),
            self.logo.as_ref(),
            lerp_color(phase::accent_hover(), phase::text(), 0.55),
            &self.phase_identity_name(),
        );

        let compact_profile = profile_width < 200.0;
        let phase_name = self
            .linked_user
            .as_ref()
            .map(|user| user.username.trim().trim_start_matches('@'))
            .filter(|username| !username.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| "Connect account".to_owned());
        let phase_detail = if self.plugin_token.is_some() {
            "Companion account"
        } else {
            "Phase companion"
        };
        let text_right = avatar_rect.left() - 12.0;
        let brand_size = if compact_profile { 18.0 } else { 21.0 };
        let brand_rect = Rect::from_center_size(
            Pos2::new(
                profile_rect.left() + if compact_profile { 17.0 } else { 21.0 },
                profile_rect.center().y - profile_hover,
            ),
            Vec2::splat(brand_size),
        );
        if let Some(texture) = self.phase_brand_logo.as_ref() {
            paint_texture_circle(&painter, brand_rect, texture, Color32::WHITE);
            painter.circle_stroke(
                brand_rect.center(),
                brand_rect.width() * 0.5,
                Stroke::new(0.8, color_with_alpha(Color32::WHITE, 0.54)),
            );
        }
        let text_left = brand_rect.right() + if compact_profile { 6.0 } else { 8.0 };
        let available_text = (text_right - text_left).max(40.0);
        let profile_text_painter = painter.with_clip_rect(Rect::from_min_max(
            Pos2::new(text_left, profile_rect.top()),
            Pos2::new(text_right, profile_rect.bottom()),
        ));
        profile_text_painter.text(
            Pos2::new(text_left, profile_rect.center().y - 8.0 - profile_hover),
            Align2::LEFT_CENTER,
            compact_middle(
                &phase_name,
                (available_text / if compact_profile { 6.1 } else { 7.4 }).max(8.0) as usize,
            ),
            type_display(if compact_profile { 11.0 } else { 13.0 }),
            phase::text(),
        );
        profile_text_painter.text(
            Pos2::new(text_left, profile_rect.center().y + 9.0 - profile_hover),
            Align2::LEFT_CENTER,
            phase_detail,
            FontId::proportional(if compact_profile { 9.5 } else { 10.5 }),
            color_with_alpha(phase::text(), 0.78),
        );

        if width >= 820.0 {
            let greeting_right = profile_rect.left() - 22.0;
            let brand_right = rect.left();
            if greeting_right > brand_right + 220.0 {
                let greeting_center =
                    Pos2::new((brand_right + greeting_right) * 0.5, rect.center().y);
                painter.text(
                    greeting_center,
                    Align2::CENTER_CENTER,
                    format!("{}, {}!", daypart_greeting(), self.phase_identity_name()),
                    type_display(26.0),
                    phase::text(),
                );
            }
        }

        if profile_response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if profile_response.clicked() {
            self.select_tab(ViewTab::Account);
        }
    }

    fn companion_wide_body(&mut self, ui: &mut Ui) {
        let available = ui.available_size();
        let rail_gap = if available.x < 820.0 { 14.0 } else { RAIL_GAP };
        let rail_width = (available.x * 0.285).clamp(180.0, RAIL_WIDTH);
        let workspace_width = (available.x - rail_width - rail_gap).max(360.0);

        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            ui.allocate_ui_with_layout(
                Vec2::new(workspace_width, available.y),
                egui::Layout::top_down(Align::Min),
                |ui| self.companion_workspace(ui, workspace_width, available.y),
            );
            ui.add_space(rail_gap);
            ui.allocate_ui_with_layout(
                Vec2::new(rail_width, available.y),
                egui::Layout::top_down(Align::Min),
                |ui| self.companion_rail(ui, rail_width, available.y, false),
            );
        });
    }

    fn companion_paired_compact_body(&mut self, ui: &mut Ui) {
        let bounds = ui.available_rect_before_wrap();
        ui.allocate_rect(bounds, Sense::hover());

        // The minimum/default window uses the reference composition directly:
        // workspace at the top-left, profile floating over the right portrait,
        // and a deliberate gutter separating the two planes.
        let left_inset = 21.0;
        let rail_gap = 34.0;
        let workspace_width = (bounds.width() / GOLDEN_RATIO).max(360.0);
        let rail_width = (bounds.width() - left_inset - rail_gap - workspace_width).max(176.0);
        let content_top = bounds.top() + TOPBAR_HEIGHT + SHELL_GAP;
        let workspace_bottom_margin = 34.0;
        let rail_top = content_top + DOCK_HEIGHT + DOCK_TOP_GAP;

        let workspace_rect = Rect::from_min_max(
            Pos2::new(bounds.left() + left_inset, content_top),
            Pos2::new(
                bounds.left() + left_inset + workspace_width,
                bounds.bottom() - workspace_bottom_margin,
            ),
        );
        let rail_rect = Rect::from_min_max(
            Pos2::new(workspace_rect.right() + rail_gap, rail_top),
            Pos2::new(bounds.right(), workspace_rect.bottom()),
        );
        let topbar_rect = Rect::from_min_size(
            Pos2::new(bounds.left(), bounds.top() + 10.0),
            Vec2::new(bounds.width(), TOPBAR_HEIGHT),
        );

        let greeting = format!("{}, {}!", daypart_greeting(), self.phase_identity_name());
        ui.painter().text(
            Pos2::new(workspace_rect.center().x, bounds.top() + 24.0),
            Align2::CENTER_CENTER,
            greeting,
            type_display(22.0),
            phase::text(),
        );

        ui.allocate_ui_at_rect(workspace_rect, |ui| {
            self.companion_workspace(ui, workspace_rect.width(), workspace_rect.height())
        });
        ui.allocate_ui_at_rect(rail_rect, |ui| {
            self.companion_rail(ui, rail_rect.width(), rail_rect.height(), true)
        });
        ui.allocate_ui_at_rect(topbar_rect, |ui| {
            self.companion_topbar(ui, Some(rail_width))
        });
    }

    fn companion_compact_body(&mut self, ui: &mut Ui) {
        let available = ui.available_size();
        self.companion_workspace(ui, available.x, available.y);
    }

    fn companion_workspace(&mut self, ui: &mut Ui, width: f32, height: f32) {
        let (bounds, _) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
        // The Install home replaces the kicker/title header with the news hero
        // card, so its body spans the full workspace height.
        let show_header = self.active_tab != ViewTab::Install;
        let header_height = if show_header {
            WORKSPACE_HEADER_HEIGHT + WORKSPACE_GAP
        } else {
            0.0
        };
        let header_rect =
            Rect::from_min_size(bounds.min, Vec2::new(width, WORKSPACE_HEADER_HEIGHT));
        let dock_rect = Rect::from_min_max(
            Pos2::new(bounds.left(), bounds.bottom() - DOCK_HEIGHT),
            bounds.right_bottom(),
        );
        let body_rect = Rect::from_min_max(
            Pos2::new(bounds.left(), bounds.top() + header_height),
            Pos2::new(bounds.right(), dock_rect.top() - DOCK_TOP_GAP),
        );
        self.workspace_body_height = body_rect.height();

        if show_header {
            ui.allocate_ui_at_rect(header_rect, |ui| self.companion_workspace_header(ui));
            self.paint_workspace_surface(ui, body_rect);
        }
        let inner = if show_header {
            body_rect.shrink2(Vec2::new(18.0, 16.0))
        } else {
            body_rect.shrink2(Vec2::new(2.0, 0.0))
        };
        ui.allocate_ui_at_rect(inner, |ui| self.companion_workspace_content(ui));
        ui.allocate_ui_at_rect(dock_rect, |ui| self.companion_dock(ui));
    }

    fn companion_workspace_header(&self, ui: &mut Ui) {
        let rect = ui.available_rect_before_wrap();
        ui.allocate_rect(rect, Sense::hover());
        let painter = ui.painter();
        let (kicker, title, detail) = match self.active_tab {
            ViewTab::Install => ("HOME", "News", "The latest from Phase Animator."),
            ViewTab::Account => (
                "IDENTITY",
                "Accounts & access",
                "Connect Phase and verify the Roblox account that owns the plugin.",
            ),
            ViewTab::Folders => (
                "DESTINATION",
                "Plugin folders",
                "Choose where Phase Animator is installed and backed up.",
            ),
            ViewTab::Video => (
                "REFERENCE",
                "Video sync",
                "Keep a local or YouTube reference aligned with the Studio timeline.",
            ),
            ViewTab::Options => (
                "PREFERENCES",
                "Companion settings",
                "Themes, updates, recovery, and connection diagnostics.",
            ),
        };

        painter.text(
            Pos2::new(rect.left(), rect.top() + 6.0),
            Align2::LEFT_TOP,
            kicker,
            type_display(10.0),
            color_with_alpha(phase::accent_hover(), 0.85),
        );
        painter.text(
            Pos2::new(rect.left(), rect.top() + 24.0),
            Align2::LEFT_TOP,
            title,
            type_display(24.0),
            phase::text(),
        );
        painter.text(
            Pos2::new(rect.left(), rect.top() + 55.0),
            Align2::LEFT_TOP,
            detail,
            FontId::proportional(11.5),
            lerp_color(phase::text_muted(), phase::text(), 0.32),
        );
    }

    fn paint_workspace_surface(&self, ui: &mut Ui, rect: Rect) {
        let painter = ui.painter();
        let fill = glass_fill(
            lerp_color(phase::surface(), phase::background(), 0.12),
            GLASS_PANEL_ALPHA,
        );
        painter.rect_filled(
            rect.translate(Vec2::new(0.0, 7.0)),
            Rounding::same(WORKSPACE_ROUNDING),
            Color32::from_black_alpha(40),
        );
        painter.rect_filled(rect, Rounding::same(WORKSPACE_ROUNDING), fill);
        painter.rect_stroke(
            rect,
            Rounding::same(WORKSPACE_ROUNDING),
            Stroke::new(
                1.0,
                color_with_alpha(
                    lerp_color(phase::line(), Color32::WHITE, 0.22),
                    GLASS_RIM_ALPHA,
                ),
            ),
        );
        top_highlight(
            painter,
            rect,
            WORKSPACE_ROUNDING,
            color_with_alpha(Color32::WHITE, 0.2),
        );
    }

    fn companion_workspace_content(&mut self, ui: &mut Ui) {
        let inner_width = (ui.available_width() - BODY_RIGHT_INSET).max(1.0);
        set_content_width(inner_width);
        let mut body_scroll = egui::ScrollArea::vertical()
            .id_source(("companion-workspace", self.active_tab.index()))
            .auto_shrink([false, false]);
        if self.active_tab == ViewTab::Install {
            body_scroll = body_scroll
                .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden);
        }
        if self.reset_body_scroll {
            body_scroll = body_scroll.vertical_scroll_offset(0.0);
        }

        body_scroll.show(ui, |ui| {
            ui.set_width(inner_width);
            let dt = ui.input(|input| input.stable_dt).clamp(0.0, 1.0 / 30.0);
            let frame = self.tab_page_motion.step(dt, 0.22);
            if frame.running {
                ui.ctx().request_repaint();
            }
            ui.scope(|ui| {
                ui.set_opacity(frame.opacity);
                ui.add_space(frame.offset.abs() * 0.4);
                match self.active_tab {
                    ViewTab::Install => self.companion_install_view(ui),
                    ViewTab::Account => self.account_tab(ui),
                    ViewTab::Folders => self.folders_tab(ui),
                    ViewTab::Video => self.video_tab(ui),
                    ViewTab::Options => self.options_tab(ui),
                }
                ui.add_space(PAGE_BOTTOM_INSET);
            });
        });
        self.reset_body_scroll = false;
    }

    fn companion_install_view(&mut self, ui: &mut Ui) {
        // News carousel: auto-advance every few seconds with a short fade-in.
        if self.news_changed_at.elapsed() >= NEWS_AUTO_ADVANCE {
            self.news_page = (self.news_page + 1) % NEWS_PAGE_COUNT;
            self.news_changed_at = Instant::now();
        }
        let fade = (self.news_changed_at.elapsed().as_secs_f32() / NEWS_FADE).clamp(0.0, 1.0);
        if fade < 1.0 {
            ui.ctx().request_repaint();
        } else {
            ui.ctx().request_repaint_after(Duration::from_millis(500));
        }

        let latest = self
            .release
            .as_ref()
            .map(|release| release.latest_version.clone())
            .unwrap_or_else(|| "Checking".to_owned());
        let (headline, story) = self.news_page_copy();

        // The install action lives in the floating dock. The news plane can
        // therefore occupy this entire visual beat without a utility bar
        // competing underneath it.
        let compact_height = self.workspace_body_height < 420.0;
        let hero_height = self.workspace_body_height.max(190.0);
        let hero_width = ui.available_width();
        let (hero_rect, _) =
            ui.allocate_exact_size(Vec2::new(hero_width, hero_height), Sense::hover());
        let painter = ui.painter().clone();

        painter.rect_filled(
            hero_rect.translate(Vec2::new(0.0, 7.0)),
            Rounding::same(WORKSPACE_ROUNDING),
            Color32::from_black_alpha(42),
        );
        let hero_texture = rounded_gradient_texture(
            &mut self.gradient_cache,
            ui.ctx(),
            "news-hero",
            hero_rect.size(),
            WORKSPACE_ROUNDING,
            GradientDirection::Vertical,
            glass_fill(
                lerp_color(phase::accent_dim(), phase::accent(), 0.18),
                GLASS_PANEL_STRONG_ALPHA,
            ),
            glass_fill(
                lerp_color(phase::accent_dim(), phase::background(), 0.3),
                GLASS_PANEL_ALPHA,
            ),
            Some((
                Pos2::new(hero_rect.width() * 0.42, -hero_rect.height() * 0.06),
                hero_rect.width() * 0.55,
                glass_fill(lerp_color(phase::accent_dim(), phase::text(), 0.28), 0.72),
                0.42,
            )),
        );
        painter.image(
            hero_texture.id(),
            hero_rect,
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
            Color32::WHITE,
        );
        painter.rect_stroke(
            hero_rect,
            Rounding::same(WORKSPACE_ROUNDING),
            Stroke::new(
                1.0,
                color_with_alpha(
                    lerp_color(phase::accent_hover(), Color32::WHITE, 0.34),
                    GLASS_RIM_ALPHA,
                ),
            ),
        );
        top_highlight(
            &painter,
            hero_rect,
            WORKSPACE_ROUNDING,
            color_with_alpha(Color32::WHITE, 0.24),
        );

        // "News:" eyebrow, with the latest version tucked against the right edge.
        let hero_side = if compact_height { 20.0 } else { 30.0 };
        painter.text(
            Pos2::new(
                hero_rect.left() + hero_side,
                hero_rect.top() + if compact_height { 16.0 } else { 26.0 },
            ),
            Align2::LEFT_TOP,
            "News:",
            type_display(if compact_height { 17.0 } else { 20.0 }),
            phase::text(),
        );
        painter.text(
            Pos2::new(
                hero_rect.right() - hero_side,
                hero_rect.top() + if compact_height { 19.0 } else { 30.0 },
            ),
            Align2::RIGHT_TOP,
            format!("LATEST  ·  {}", latest.to_uppercase()),
            FontId::proportional(if compact_height { 9.0 } else { 11.0 }),
            color_with_alpha(phase::text(), 0.55),
        );

        // Centered glowing headline + story, cross-fading between pages.
        let text_area = Rect::from_min_max(
            Pos2::new(
                hero_rect.left() + hero_side,
                hero_rect.top() + if compact_height { 46.0 } else { 62.0 },
            ),
            Pos2::new(
                hero_rect.right() - hero_side,
                hero_rect.bottom() - if compact_height { 34.0 } else { 56.0 },
            ),
        );
        let headline_font = type_display(if compact_height { 22.0 } else { 29.0 });
        let body_font = FontId::proportional(if compact_height { 11.5 } else { 14.0 });
        let wrap = text_area.width();
        let body_wrap = if compact_height {
            wrap * 0.88
        } else {
            (wrap * 0.78).max(260.0)
        };
        let fade_color = |color: Color32| color_with_alpha(color, fade);
        let headline_galley = painter.layout(
            headline.clone(),
            headline_font,
            fade_color(phase::text()),
            wrap,
        );
        let body_galley = painter.layout(
            story.clone(),
            body_font,
            fade_color(lerp_color(phase::text_secondary(), phase::text(), 0.55)),
            body_wrap,
        );
        let copy_gap = if compact_height { 9.0 } else { 14.0 };
        let block_height = headline_galley.size().y + copy_gap + body_galley.size().y;
        let rise = (1.0 - fade) * 8.0;
        let y = (text_area.center().y - block_height * 0.5 - 6.0 + rise).max(text_area.top());

        let headline_pos = Pos2::new(text_area.center().x - headline_galley.size().x * 0.5, y);
        let headline_height = headline_galley.size().y;
        painter.galley(headline_pos, headline_galley, fade_color(phase::text()));

        let body_pos = Pos2::new(
            text_area.center().x - body_galley.size().x * 0.5,
            headline_pos.y + headline_height + copy_gap,
        );
        painter.galley(body_pos, body_galley, Color32::PLACEHOLDER);

        // Carousel dots.
        let dots_y = hero_rect.bottom() - if compact_height { 18.0 } else { 30.0 };
        let mut dot_clicked = None;
        for index in 0..NEWS_PAGE_COUNT {
            let centered_index = index as f32 - (NEWS_PAGE_COUNT.saturating_sub(1) as f32 * 0.5);
            let x = hero_rect.center().x + centered_index * 22.0;
            let active = index == self.news_page;
            let radius = if active { 5.5 } else { 4.0 };
            let dot_rect = Rect::from_center_size(Pos2::new(x, dots_y), Vec2::splat(18.0));
            let response = ui.interact(
                dot_rect,
                ui.make_persistent_id(("news-dot", index)),
                Sense::click(),
            );
            response.widget_info(|| {
                egui::WidgetInfo::selected(
                    egui::WidgetType::Button,
                    active,
                    format!("Show news page {}", index + 1),
                )
            });
            let hover = hover_t(ui, response.id, response.hovered());
            let color = if active {
                phase::text()
            } else {
                color_with_alpha(phase::text(), 0.35 + hover * 0.35)
            };
            painter.circle_filled(Pos2::new(x, dots_y), radius + hover * 0.8, color);
            if response.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            if response.clicked() {
                dot_clicked = Some(index);
            }
        }
        if let Some(index) = dot_clicked {
            if index != self.news_page {
                self.news_page = index;
                self.news_changed_at = Instant::now();
            }
        }
    }

    /// Headline + body for the current news page. Page one stays wired to the
    /// live release state; the other two spotlight shipped companion features.
    fn news_page_copy(&self) -> (String, String) {
        match self.news_page {
            1 => (
                "Video reference sync".to_owned(),
                "Keep YouTube or local video references locked to the Studio timeline. Play, pause, seek, and rate stay in sync."
                    .to_owned(),
            ),
            _ => {
                let checking = self.release.is_none() && self.release_error.is_none();
                let available = self.release.as_ref().is_some_and(|release| {
                    release.download_available && !release.blocked && !self.local_release_current
                });
                let headline = if self.phase == InstallPhase::Error {
                    "Phase needs your attention"
                } else if self.release_error.is_some() {
                    "We couldn't reach Phase"
                } else if checking {
                    "Checking the latest release"
                } else if available {
                    "A new Phase build is ready"
                } else {
                    "You’re ready to animate"
                };
                (headline.to_owned(), self.release_story_copy())
            }
        }
    }

    fn companion_dock(&mut self, ui: &mut Ui) {
        let width = ui.available_width();
        let (band, _) = ui.allocate_exact_size(Vec2::new(width, DOCK_HEIGHT), Sense::hover());
        let painter = ui.painter().clone();

        let destinations = [
            (ViewTab::Install, "Install"),
            (ViewTab::Video, "Video"),
            (ViewTab::Folders, "Folders"),
            (ViewTab::Options, "Settings"),
        ];
        let pill_width = (band.width() * 0.85).clamp(280.0, band.width());
        let pill_radius = DOCK_HEIGHT * 0.5;
        let pill = Rect::from_center_size(band.center(), Vec2::new(pill_width, DOCK_HEIGHT));

        // One quiet floating plane. Selection is expressed with typography and
        // a short indicator instead of another nested pill.
        painter.rect_filled(
            pill.translate(Vec2::new(0.0, 4.0)),
            Rounding::same(pill_radius),
            Color32::from_black_alpha(30),
        );
        painter.rect_filled(
            pill,
            Rounding::same(pill_radius),
            glass_fill(
                lerp_color(phase::surface(), phase::accent_dim(), 0.62),
                GLASS_PANEL_ALPHA,
            ),
        );
        painter.rect_stroke(
            pill,
            Rounding::same(pill_radius),
            Stroke::new(
                1.0,
                color_with_alpha(
                    lerp_color(phase::line(), Color32::WHITE, 0.28),
                    GLASS_RIM_ALPHA,
                ),
            ),
        );
        top_highlight(
            &painter,
            pill,
            pill_radius,
            color_with_alpha(Color32::WHITE, 0.18),
        );

        let cells_left = pill.left() + 6.0;
        let cell_width = (pill.width() - 12.0) / destinations.len() as f32;
        let target_index = destinations
            .iter()
            .position(|(tab, _)| *tab == self.active_tab);
        if let Some(index) = target_index {
            let dt = ui.input(|input| input.stable_dt).clamp(0.0, 1.0 / 30.0);
            if self
                .tab_indicator
                .step(index as f32, dt, motion::Spring::expressive())
            {
                ui.ctx().request_repaint();
            }
            let indicator_center = Pos2::new(
                cells_left + (self.tab_indicator.value() + 0.5) * cell_width,
                pill.bottom() - 6.0,
            );
            let active_rect = Rect::from_center_size(indicator_center, Vec2::new(28.0, 2.5));
            painter.rect_filled(
                active_rect,
                Rounding::same(1.5),
                color_with_alpha(phase::text(), 0.88),
            );
        }

        let mut clicked = None;
        for (index, (tab, label)) in destinations.iter().enumerate() {
            let cell = Rect::from_min_max(
                Pos2::new(cells_left + index as f32 * cell_width, pill.top()),
                Pos2::new(cells_left + (index + 1) as f32 * cell_width, pill.bottom()),
            );
            let response = ui.interact(
                cell,
                ui.make_persistent_id(("companion-dock", index)),
                Sense::click(),
            );
            let selected = *tab == self.active_tab;
            response.widget_info(|| {
                let description = if *tab == ViewTab::Install && selected {
                    "Install or update Phase"
                } else {
                    *label
                };
                egui::WidgetInfo::selected(egui::WidgetType::Button, selected, description)
            });
            let hovered = hover_t(ui, response.id, response.hovered() || response.has_focus());
            if hovered > 0.0 && !selected {
                painter.rect_filled(
                    cell.shrink2(Vec2::new(3.0, 6.0)),
                    Rounding::same((cell.height() - 12.0) * 0.5),
                    color_with_alpha(phase::surface_hover(), 0.32 * hovered),
                );
            }
            let color = if selected {
                phase::text()
            } else {
                lerp_color(phase::text_secondary(), phase::text(), hovered)
            };
            painter.text(
                cell.center(),
                Align2::CENTER_CENTER,
                *label,
                if selected {
                    type_display(13.0)
                } else {
                    FontId::proportional(13.0)
                },
                color,
            );
            if response.has_focus() {
                painter.rect_stroke(
                    cell.shrink2(Vec2::new(4.0, 5.0)),
                    Rounding::same((cell.height() - 10.0) * 0.5),
                    Stroke::new(1.0, phase::accent_hover()),
                );
            }
            if response.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            if response.clicked() {
                clicked = Some(*tab);
            }
        }
        if let Some(tab) = clicked {
            if tab == ViewTab::Install && self.active_tab == ViewTab::Install {
                if !self.is_busy() {
                    self.primary_action();
                }
            } else {
                self.select_tab(tab);
            }
        }

        if self.is_busy() {
            let track = Rect::from_min_max(
                Pos2::new(pill.left() + 18.0, pill.bottom() - 2.0),
                Pos2::new(pill.right() - 18.0, pill.bottom()),
            );
            painter.rect_filled(
                track,
                Rounding::same(1.0),
                color_with_alpha(phase::text(), 0.16),
            );
            let fill = Rect::from_min_size(
                track.min,
                Vec2::new(
                    track.width() * self.progress.clamp(0.04, 1.0),
                    track.height(),
                ),
            );
            painter.rect_filled(fill, Rounding::same(1.0), phase_color(self.phase));
        }
    }

    fn companion_rail(&mut self, ui: &mut Ui, width: f32, height: f32, extend_stage: bool) {
        let (rect, _) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
        let compact_stage = width < 220.0 || height < 440.0;
        let stage_top = if compact_stage {
            rect.top()
        } else {
            rect.top() + (height * 0.09).clamp(30.0, 56.0)
        };
        let stage_height = if extend_stage {
            (height - 8.0).max(240.0)
        } else if compact_stage {
            (height - DOCK_HEIGHT - DOCK_TOP_GAP).max(240.0)
        } else {
            (height - (stage_top - rect.top()) - 8.0)
                .min(440.0)
                .max(270.0)
        };
        let stage = Rect::from_min_max(
            Pos2::new(rect.left(), stage_top),
            Pos2::new(rect.right(), (stage_top + stage_height).min(rect.bottom())),
        );
        let painter = ui.painter().clone();

        painter.rect_filled(
            stage.translate(Vec2::new(0.0, 7.0)),
            Rounding::same(WORKSPACE_ROUNDING),
            Color32::from_black_alpha(40),
        );
        let stage_texture = rounded_gradient_texture(
            &mut self.gradient_cache,
            ui.ctx(),
            "rail-stage",
            stage.size(),
            WORKSPACE_ROUNDING,
            GradientDirection::Diagonal,
            glass_fill(
                lerp_color(phase::accent_dim(), phase::accent(), 0.85),
                GLASS_PANEL_STRONG_ALPHA,
            ),
            glass_fill(
                lerp_color(phase::accent_dim(), phase::background(), 0.15),
                GLASS_PANEL_ALPHA,
            ),
            Some((
                Pos2::new(stage.width() * 0.22, stage.height() * 0.12),
                stage.width() * 0.7,
                glass_fill(lerp_color(phase::accent(), phase::text(), 0.3), 0.7),
                0.26,
            )),
        );
        painter.image(
            stage_texture.id(),
            stage,
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
            Color32::WHITE,
        );
        painter.rect_stroke(
            stage,
            Rounding::same(WORKSPACE_ROUNDING),
            Stroke::new(
                1.0,
                color_with_alpha(
                    lerp_color(phase::accent_hover(), Color32::WHITE, 0.28),
                    GLASS_RIM_ALPHA,
                ),
            ),
        );
        top_highlight(
            &painter,
            stage,
            WORKSPACE_ROUNDING,
            color_with_alpha(Color32::WHITE, 0.24),
        );

        let character_scale =
            ease_out_cubic((self.ui_started_at.elapsed().as_secs_f32() / 0.78).clamp(0.0, 1.0));
        let character_area = Rect::from_min_max(
            Pos2::new(stage.left() - 32.0, stage.top() + 12.0),
            Pos2::new(stage.right() + 32.0, stage.bottom() - 58.0),
        );
        let animated_character_area = Rect::from_center_size(
            character_area.center(),
            character_area.size() * (0.94 + character_scale * 0.06),
        );
        paint_roblox_character(
            &painter,
            animated_character_area,
            self.roblox_avatar.as_ref(),
            self.logo.as_ref(),
        );

        let roblox_name = self.roblox_identity_name();
        let roblox_label = if self.roblox_user_id.trim().is_empty() {
            "Connect account".to_owned()
        } else {
            roblox_name.trim_start_matches('@').to_owned()
        };
        let narrow_stage = stage.width() < 200.0;
        let identity_inset = (stage.width() * 0.065).clamp(12.0, 18.0);
        let icon_size = if narrow_stage { 16.0 } else { 18.0 };
        let identity_gap = if narrow_stage { 7.0 } else { 8.0 };
        let identity_y = stage.bottom() - 27.0;
        let name_font = type_display(if narrow_stage { 10.5 } else { 12.0 });
        let max_name_width =
            (stage.width() - identity_inset * 2.0 - icon_size - identity_gap).max(40.0);
        let compact_name = compact_middle(
            &roblox_label,
            (max_name_width / if narrow_stage { 5.7 } else { 6.2 }).max(8.0) as usize,
        );
        let name_galley = painter.layout_no_wrap(compact_name, name_font, phase::text());
        let identity_width = icon_size + identity_gap + name_galley.size().x;
        let identity_left = stage.center().x - identity_width * 0.5;
        if let Some(texture) = self.roblox_brand_logo.as_ref() {
            let icon_rect = Rect::from_center_size(
                Pos2::new(identity_left + icon_size * 0.5, identity_y),
                Vec2::splat(icon_size),
            );
            painter.image(
                texture.id(),
                icon_rect,
                Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                Color32::WHITE,
            );
        }
        painter.galley(
            Pos2::new(
                identity_left + icon_size + identity_gap,
                identity_y - name_galley.size().y * 0.5,
            ),
            name_galley,
            phase::text(),
        );

        let stage_response = ui.interact(
            stage,
            ui.make_persistent_id("companion-portrait-stage"),
            Sense::click(),
        );
        stage_response.widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::WidgetType::Button,
                "Open Roblox identity and account settings",
            )
        });
        if stage_response.hovered() || stage_response.has_focus() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            painter.rect_stroke(
                stage.expand(1.0),
                Rounding::same(WORKSPACE_ROUNDING + 1.0),
                Stroke::new(1.0, color_with_alpha(phase::accent_hover(), 0.72)),
            );
        }
        if stage_response.clicked() {
            self.select_tab(ViewTab::Account);
        }
    }

    fn phase_identity_name(&self) -> String {
        self.linked_user
            .as_ref()
            .map(display_linked_user)
            .unwrap_or_else(|| "Animator".to_owned())
    }

    fn roblox_identity_name(&self) -> String {
        self.roblox_username
            .clone()
            .filter(|name| !name.trim().is_empty())
            .or_else(|| {
                (!self.roblox_user_id.trim().is_empty()).then(|| self.roblox_user_id.clone())
            })
            .unwrap_or_else(|| "Your Roblox avatar".to_owned())
    }

    fn release_story_copy(&self) -> String {
        let raw = (self.phase == InstallPhase::Error)
            .then(|| self.activity.last().map(|line| line.text.as_str()))
            .flatten()
            .or(self.release_error.as_deref())
            .or_else(|| {
                self.release.as_ref().and_then(|release| {
                    (!release.notes.trim().is_empty())
                        .then_some(release.notes.as_str())
                        .or_else(|| {
                            (!release.message.trim().is_empty()).then_some(release.message.as_str())
                        })
                })
            })
            .unwrap_or("Phase is checking your plugin, account, and local install status.");
        let compact = raw.split_whitespace().collect::<Vec<_>>().join(" ");
        if compact.chars().count() <= 170 {
            compact
        } else {
            format!("{}…", compact.chars().take(167).collect::<String>())
        }
    }
}

fn paint_companion_avatar(
    painter: &egui::Painter,
    rect: Rect,
    avatar: Option<&TextureHandle>,
    fallback_logo: Option<&TextureHandle>,
    ring: Color32,
    name: &str,
) {
    painter.circle_filled(
        rect.center(),
        rect.width() * 0.5,
        color_with_alpha(phase::background(), 0.42),
    );
    if let Some(texture) = avatar.or(fallback_logo) {
        painter.image(
            texture.id(),
            rect.shrink(3.0),
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
            Color32::WHITE,
        );
    } else {
        painter.text(
            rect.center(),
            Align2::CENTER_CENTER,
            initials(name),
            FontId::proportional((rect.width() * 0.28).clamp(13.0, 34.0)),
            phase::text(),
        );
    }
    painter.circle_stroke(
        rect.center(),
        rect.width() * 0.5,
        Stroke::new((rect.width() * 0.02).clamp(1.5, 3.0), ring),
    );
}

fn paint_texture_circle(
    painter: &egui::Painter,
    rect: Rect,
    texture: &TextureHandle,
    tint: Color32,
) {
    const SEGMENTS: u32 = 32;
    let mut mesh = egui::Mesh::with_texture(texture.id());
    mesh.vertices.push(egui::epaint::Vertex {
        pos: rect.center(),
        uv: Pos2::new(0.5, 0.5),
        color: tint,
    });
    for segment in 0..=SEGMENTS {
        let angle = std::f32::consts::TAU * segment as f32 / SEGMENTS as f32;
        let direction = Vec2::new(angle.cos(), angle.sin());
        mesh.vertices.push(egui::epaint::Vertex {
            pos: rect.center() + direction * (rect.width() * 0.5),
            uv: Pos2::new(0.5 + direction.x * 0.5, 0.5 + direction.y * 0.5),
            color: tint,
        });
    }
    for segment in 0..SEGMENTS {
        mesh.indices
            .extend_from_slice(&[0, segment + 1, segment + 2]);
    }
    painter.add(egui::Shape::mesh(mesh));
}

fn paint_roblox_character(
    painter: &egui::Painter,
    rect: Rect,
    avatar: Option<&TextureHandle>,
    fallback_logo: Option<&TextureHandle>,
) {
    if let Some(texture) = avatar {
        let source = texture.size_vec2();
        let scale = (rect.width() / source.x.max(1.0)).min(rect.height() / source.y.max(1.0));
        let size = source * scale;
        let image_rect = Rect::from_min_size(
            Pos2::new(rect.center().x - size.x * 0.5, rect.bottom() - size.y),
            size,
        );
        painter.image(
            texture.id(),
            image_rect,
            Rect {
                min: Pos2::new(1.0, 0.0),
                max: Pos2::new(0.0, 1.0),
            },
            Color32::WHITE,
        );
    } else if let Some(logo) = fallback_logo {
        let logo_rect = Rect::from_center_size(rect.center(), Vec2::splat(76.0));
        painter.image(
            logo.id(),
            logo_rect,
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
            Color32::from_white_alpha(155),
        );
    }
}
