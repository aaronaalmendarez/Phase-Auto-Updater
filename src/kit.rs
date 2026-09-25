//! Reusable widgets for the companion UI.
//!
//! Every color derives from the active `phase` palette so marketplace themes
//! restyle the whole interface. Surfaces stay slightly translucent so theme
//! artwork shows through.
use super::*;

pub(super) const TEXT_SIZE: f32 = 14.0;
pub(super) const CAPTION_SIZE: f32 = 12.5;
pub(super) const CONTROL_HEIGHT: f32 = 38.0;
pub(super) const CARD_RADIUS: f32 = 16.0;
pub(super) const CONTROL_RADIUS: f32 = 10.0;

// ---------------------------------------------------------------------------
// Motion

/// Spring presets: `SNAPPY` overshoots slightly, `GENTLE` settles softly.
const SNAPPY: motion::Spring = motion::Spring {
    stiffness: 420.0,
    damping: 34.0,
    epsilon: 0.001,
};
const GENTLE: motion::Spring = motion::Spring {
    stiffness: 240.0,
    damping: 26.0,
    epsilon: 0.01,
};

fn step_spring(ctx: &Context, id: egui::Id, target: f32, spring: motion::Spring) -> f32 {
    let dt = ctx.input(|i| i.stable_dt).min(1.0 / 30.0);
    let (value, moving) = ctx.data_mut(|d| {
        let state = d.get_temp_mut_or_insert_with(id, || motion::SpringValue::new(target));
        let moving = state.step(target, dt, spring);
        (state.value(), moving)
    });
    if moving {
        ctx.request_repaint();
    }
    value
}

/// Spring-animated value keyed by `id`. Starts at its first target.
pub(super) fn spring(ctx: &Context, id: egui::Id, target: f32) -> f32 {
    step_spring(ctx, id, target, SNAPPY)
}

pub(super) fn gentle_spring(ctx: &Context, id: egui::Id, target: f32) -> f32 {
    step_spring(ctx, id, target, GENTLE)
}

/// Jump a spring to `value` so it animates back toward its next target.
pub(super) fn spring_kick(ctx: &Context, id: egui::Id, value: f32) {
    ctx.data_mut(|d| d.insert_temp(id, motion::SpringValue::new(value)));
}

/// 0 → 1 over a short fade whenever `key` changes; 1 when unchanged.
pub(super) fn change_fade(ctx: &Context, id: egui::Id, key: u64) -> f32 {
    let now = ctx.input(|i| i.time);
    let started = ctx.data_mut(|d| {
        let entry = d.get_temp_mut_or_insert_with(id, || (key, f64::NEG_INFINITY));
        if entry.0 != key {
            *entry = (key, now);
        }
        entry.1
    });
    let t = ((now - started) / 0.32).clamp(0.0, 1.0) as f32;
    if t < 1.0 {
        ctx.request_repaint();
    }
    ease_out_cubic(t)
}

pub(super) fn hash_of(value: &impl std::hash::Hash) -> u64 {
    use std::hash::Hasher;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

/// Press depth for a response: 0 at rest, 1 fully pressed, sprung.
fn press(ui: &Ui, response: &egui::Response) -> f32 {
    let down = ui.is_enabled() && response.is_pointer_button_down_on();
    spring(
        ui.ctx(),
        response.id.with("press"),
        if down { 1.0 } else { 0.0 },
    )
}

fn pressed_rect(rect: Rect, press: f32) -> Rect {
    Rect::from_center_size(rect.center(), rect.size() * (1.0 - 0.035 * press))
}

// ---------------------------------------------------------------------------
// Input mode

/// Focus rings only appear after keyboard navigation, like macOS.
pub(super) fn track_input_mode(ctx: &Context) {
    let (tabbed, clicked) = ctx.input(|i| {
        (
            i.key_pressed(egui::Key::Tab)
                || i.key_pressed(egui::Key::ArrowDown)
                || i.key_pressed(egui::Key::ArrowUp),
            i.pointer.any_pressed(),
        )
    });
    if tabbed || clicked {
        ctx.data_mut(|d| d.insert_temp(egui::Id::new("keyboard-mode"), tabbed));
    }
}

fn keyboard_mode(ctx: &Context) -> bool {
    ctx.data(|d| d.get_temp::<bool>(egui::Id::new("keyboard-mode")))
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Tokens

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Tone {
    Good,
    Attention,
    Busy,
    Accent,
    Neutral,
    Danger,
}

pub(super) fn tone_color(tone: Tone) -> Color32 {
    readable(match tone {
        Tone::Good => phase::green(),
        Tone::Attention => phase::warning(),
        Tone::Busy => phase::blue(),
        Tone::Accent => phase::accent(),
        Tone::Neutral => phase::text_muted(),
        Tone::Danger => phase::red(),
    })
}

fn relative_luminance(color: Color32) -> f32 {
    let channel = |c: u8| {
        let c = c as f32 / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * channel(color.r()) + 0.7152 * channel(color.g()) + 0.0722 * channel(color.b())
}

fn contrast(a: Color32, b: Color32) -> f32 {
    let (la, lb) = (relative_luminance(a), relative_luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// Some marketplace palettes use their status colors as dark background
/// tints. Keep the hue but move brightness away from the canvas until the
/// color reaches 3:1 against it, so status marks stay visible.
pub(super) fn readable(color: Color32) -> Color32 {
    let background = phase::background();
    if contrast(color, background) >= 3.0 {
        return color;
    }
    let dark_canvas = relative_luminance(background) < 0.4;
    let mut hsva = egui::ecolor::Hsva::from(color);
    hsva.s = hsva.s.max(0.35);
    for _ in 0..20 {
        hsva.v = if dark_canvas {
            (hsva.v + 0.05).min(1.0)
        } else {
            (hsva.v - 0.05).max(0.0)
        };
        let candidate = Color32::from(hsva);
        if contrast(candidate, background) >= 3.0 {
            return candidate;
        }
    }
    Color32::from(hsva)
}

pub(super) fn semibold(size: f32) -> FontId {
    type_display(size)
}

pub(super) fn body(size: f32) -> FontId {
    FontId::proportional(size)
}

fn icon_font(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(PHOSPHOR_FONT.into()))
}

static SURFACE_TINT: std::sync::RwLock<Option<Color32>> = std::sync::RwLock::new(None);

/// Tint surfaces toward the theme artwork's hue so cards feel part of it.
pub(super) fn set_surface_tint(tint: Option<Color32>) {
    if let Ok(mut current) = SURFACE_TINT.write() {
        *current = tint;
    }
}

/// Keep `base`'s brightness but borrow hue from the active artwork.
pub(super) fn tinted(base: Color32) -> Color32 {
    let Some(tint) = SURFACE_TINT.read().ok().and_then(|t| *t) else {
        return base;
    };
    let mut hsva = egui::ecolor::Hsva::from(tint);
    let base_hsva = egui::ecolor::Hsva::from(base);
    hsva.s *= 0.6;
    hsva.v = base_hsva.v;
    hsva.a = base_hsva.a;
    lerp_color(base, Color32::from(hsva), 0.4)
}

pub(super) fn card_fill() -> Color32 {
    tinted(color_with_alpha(
        lerp_color(phase::background(), phase::surface(), 0.72),
        0.80,
    ))
}

pub(super) fn card_stroke() -> Stroke {
    Stroke::new(1.0, color_with_alpha(phase::line(), 0.42))
}

fn rim() -> Color32 {
    color_with_alpha(Color32::WHITE, 0.07)
}

fn soft_shadow() -> egui::epaint::Shadow {
    egui::epaint::Shadow {
        offset: Vec2::new(0.0, 8.0),
        blur: 24.0,
        spread: -6.0,
        color: Color32::from_black_alpha(60),
    }
}

// ---------------------------------------------------------------------------
// Text

pub(super) fn note(ui: &mut Ui, value: impl Into<String>) -> egui::Response {
    ui.add(
        egui::Label::new(
            RichText::new(value)
                .size(TEXT_SIZE)
                .color(phase::text_secondary()),
        )
        .wrap(true),
    )
}

pub(super) fn caption(ui: &mut Ui, value: impl Into<String>) -> egui::Response {
    ui.add(
        egui::Label::new(
            RichText::new(value)
                .size(CAPTION_SIZE)
                .color(phase::text_muted()),
        )
        .wrap(true),
    )
}

pub(super) fn title(ui: &mut Ui, value: impl Into<String>, size: f32) -> egui::Response {
    ui.add(
        egui::Label::new(
            RichText::new(value)
                .font(semibold(size))
                .line_height(Some((size * 1.18).round()))
                .color(phase::text()),
        )
        .wrap(true),
    )
}

pub(super) fn page_header(
    ui: &mut Ui,
    heading: &str,
    detail: &str,
    trailing: impl FnOnce(&mut Ui),
) {
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 4.0;
            title(ui, heading, 26.0);
            note(ui, detail);
        });
        ui.with_layout(egui::Layout::right_to_left(Align::Min), trailing);
    });
    ui.add_space(22.0);
}

pub(super) fn section_label(ui: &mut Ui, label: &str) {
    ui.add_space(6.0);
    ui.label(
        RichText::new(label)
            .font(semibold(CAPTION_SIZE))
            .color(color_with_alpha(phase::text_secondary(), 0.85)),
    );
    ui.add_space(2.0);
}

/// One-line text that ends in an ellipsis when it does not fit.
pub(super) fn truncated(
    ui: &Ui,
    value: &str,
    font: FontId,
    color: Color32,
    width: f32,
) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple(value.to_owned(), font, color, width.max(1.0));
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    ui.fonts(|f| f.layout_job(job))
}

// ---------------------------------------------------------------------------
// Surfaces

pub(super) fn card_frame() -> egui::Frame {
    egui::Frame::none()
        .fill(card_fill())
        .stroke(card_stroke())
        .rounding(Rounding::same(CARD_RADIUS))
        .inner_margin(Margin::same(20.0))
        .shadow(soft_shadow())
}

pub(super) fn card<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> egui::InnerResponse<R> {
    let response = card_frame().show(ui, |ui| {
        ui.set_width(ui.available_width());
        add(ui)
    });
    top_highlight(ui.painter(), response.response.rect, CARD_RADIUS, rim());
    response
}

/// A card for grouped rows: tight vertical padding, rows separated by hairlines.
pub(super) fn group<R>(ui: &mut Ui, label: &str, add: impl FnOnce(&mut Ui) -> R) -> R {
    section_label(ui, label);
    let inner = egui::Frame::none()
        .fill(color_with_alpha(card_fill(), 0.7))
        .rounding(Rounding::same(CARD_RADIUS))
        .inner_margin(Margin::symmetric(18.0, 6.0))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        });
    ui.add_space(22.0);
    inner.inner
}

pub(super) fn divider(ui: &mut Ui) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), Sense::hover());
    ui.painter().hline(
        rect.x_range(),
        rect.center().y,
        Stroke::new(1.0, color_with_alpha(phase::line(), 0.32)),
    );
}

pub(super) fn inspector_frame() -> egui::Frame {
    egui::Frame::none()
        .fill(lerp_color(phase::background(), phase::surface(), 0.85))
        .inner_margin(Margin::same(22.0))
        .rounding(Rounding::same(20.0))
        .stroke(Stroke::new(1.0, color_with_alpha(phase::line(), 0.6)))
        .shadow(egui::epaint::Shadow {
            offset: Vec2::new(0.0, 18.0),
            blur: 48.0,
            spread: 0.0,
            color: Color32::from_black_alpha(120),
        })
}

/// Dim everything below a modal and swallow its clicks. Returns true when the
/// scrim itself was clicked.
pub(super) fn scrim(ctx: &Context, id: egui::Id) -> bool {
    egui::Area::new(id.with("scrim"))
        .order(egui::Order::Middle)
        .fixed_pos(Pos2::ZERO)
        .interactable(true)
        .show(ctx, |ui| {
            let rect = ctx.screen_rect();
            let response = ui.allocate_rect(rect, Sense::click());
            ui.painter()
                .rect_filled(rect, Rounding::ZERO, Color32::from_black_alpha(150));
            response.clicked()
        })
        .inner
}

/// Centered dialog above a scrim. Returns `false` once dismissed.
pub(super) fn modal(
    ctx: &Context,
    id: &str,
    heading: &str,
    width: f32,
    add: impl FnOnce(&mut Ui),
) -> bool {
    let id = egui::Id::new(id);
    let mut open = !scrim(ctx, id);
    let width = width.min(ctx.screen_rect().width() - 48.0).max(240.0);
    egui::Area::new(id)
        .order(egui::Order::Foreground)
        .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
        .show(ctx, |ui| {
            inspector_frame().show(ui, |ui| {
                ui.set_width(width - 44.0);
                ui.horizontal(|ui| {
                    title(ui, heading, 17.0);
                    ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                        if close_button(ui).clicked() {
                            open = false;
                        }
                    });
                });
                ui.add_space(14.0);
                egui::ScrollArea::vertical()
                    .max_height(ctx.screen_rect().height() - 160.0)
                    .show(ui, add);
            });
        });
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        open = false;
    }
    open
}

fn close_button(ui: &mut Ui) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(28.0), Sense::click());
    let hover = ui
        .ctx()
        .animate_bool_with_time(response.id, response.hovered(), 0.12);
    ui.painter().circle_filled(
        rect.center(),
        14.0,
        color_with_alpha(phase::surface_hover(), 0.35 + 0.5 * hover),
    );
    let c = rect.center();
    let stroke = Stroke::new(
        1.6,
        lerp_color(phase::text_secondary(), phase::text(), hover),
    );
    ui.painter()
        .line_segment([c + Vec2::new(-4.5, -4.5), c + Vec2::new(4.5, 4.5)], stroke);
    ui.painter()
        .line_segment([c + Vec2::new(4.5, -4.5), c + Vec2::new(-4.5, 4.5)], stroke);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, "Close"));
    response.on_hover_text("Close")
}

// ---------------------------------------------------------------------------
// Icons and badges

pub(super) fn glyph(painter: &egui::Painter, center: Pos2, icon: Icon, size: f32, color: Color32) {
    painter.text(
        center,
        Align2::CENTER_CENTER,
        icon.glyph(),
        icon_font(size),
        color,
    );
}

/// A rounded, tinted square carrying an icon.
pub(super) fn icon_chip(ui: &mut Ui, icon: Icon, tint: Color32, size: f32) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    paint_icon_chip(ui.painter(), rect, icon, tint);
    response
}

pub(super) fn paint_icon_chip(painter: &egui::Painter, rect: Rect, icon: Icon, tint: Color32) {
    let radius = (rect.height() * 0.3).round();
    painter.rect_filled(rect, radius, color_with_alpha(tint, 0.18));
    painter.rect_stroke(rect, radius, Stroke::new(1.0, color_with_alpha(tint, 0.30)));
    glyph(
        painter,
        rect.center(),
        icon,
        rect.height() * 0.52,
        lerp_color(tint, phase::text(), 0.55),
    );
}

/// Colored dot plus label.
pub(super) fn status_pill(ui: &mut Ui, tone: Tone, label: &str) -> egui::Response {
    let galley = ui.painter().layout_no_wrap(
        label.to_owned(),
        body(CAPTION_SIZE + 0.5),
        phase::text_secondary(),
    );
    let size = Vec2::new(galley.size().x + 14.0, 22.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::hover());
    ui.painter().circle_filled(
        Pos2::new(rect.left() + 3.5, rect.center().y),
        3.5,
        tone_color(tone),
    );
    ui.painter().galley(
        Pos2::new(rect.left() + 14.0, rect.center().y - galley.size().y / 2.0),
        galley,
        phase::text_secondary(),
    );
    response
}

pub(super) fn early_access_badge(ui: &mut Ui) {
    status_pill(ui, Tone::Busy, "Early Access")
        .on_hover_text("Early Access verified by your Phase account");
}

/// Circular avatar with an initials fallback.
pub(super) fn avatar(
    ui: &mut Ui,
    texture: Option<&TextureHandle>,
    name: &str,
    size: f32,
    sense: Sense,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), sense);
    paint_avatar(ui, rect, texture, name);
    response
}

pub(super) fn paint_avatar(ui: &Ui, rect: Rect, texture: Option<&TextureHandle>, name: &str) {
    let r = rect.width() / 2.0;
    if let Some(texture) = texture {
        ui.painter().circle_filled(
            rect.center(),
            r,
            color_with_alpha(phase::surface_hover(), 0.8),
        );
        egui::Image::new(texture)
            .uv(square_uv(texture.size_vec2()))
            .rounding(r)
            .paint_at(ui, rect);
    } else {
        ui.painter().circle_filled(
            rect.center(),
            r,
            lerp_color(phase::accent_dim(), phase::accent(), 0.35),
        );
        ui.painter().text(
            rect.center(),
            Align2::CENTER_CENTER,
            initials(name),
            semibold((rect.width() * 0.38).max(10.0)),
            phase::text(),
        );
    }
    ui.painter().circle_stroke(
        rect.center(),
        r + 0.5,
        Stroke::new(1.0, color_with_alpha(Color32::WHITE, 0.12)),
    );
}

/// UV rect for a square crop: tall images keep their top (the head of a
/// full-body avatar), wide images keep their center.
fn square_uv(size: Vec2) -> Rect {
    if size.y > size.x {
        Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, size.x / size.y))
    } else if size.x > size.y {
        let inset = (1.0 - size.y / size.x) / 2.0;
        Rect::from_min_max(Pos2::new(inset, 0.0), Pos2::new(1.0 - inset, 1.0))
    } else {
        Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0))
    }
}

/// Softly pulsing placeholder for content that is still loading.
pub(super) fn skeleton(ui: &mut Ui, size: Vec2, radius: f32) {
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let t = ui.input(|i| i.time) as f32;
    let pulse = 0.5 + 0.5 * (t * 2.4).sin();
    ui.painter().rect_filled(
        rect,
        radius,
        color_with_alpha(phase::text(), 0.06 + 0.05 * pulse),
    );
    ui.ctx()
        .request_repaint_after(std::time::Duration::from_millis(33));
}

/// Rotating arc used for in-progress states.
pub(super) fn spinner(ui: &mut Ui, size: f32, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    let t = ui.input(|i| i.time) as f32;
    let start = t * 5.0;
    let sweep = 1.6 + 0.9 * (t * 2.1).sin();
    let radius = size * 0.4;
    let points: Vec<Pos2> = (0..=24)
        .map(|i| {
            let a = start + sweep * i as f32 / 24.0;
            rect.center() + Vec2::new(a.cos(), a.sin()) * radius
        })
        .collect();
    ui.painter().circle_stroke(
        rect.center(),
        radius,
        Stroke::new(2.0, color_with_alpha(color, 0.18)),
    );
    ui.painter()
        .add(egui::Shape::line(points, Stroke::new(2.2, color)));
    ui.ctx()
        .request_repaint_after(std::time::Duration::from_millis(16));
}

// ---------------------------------------------------------------------------
// Buttons

fn focus_ring(ui: &Ui, rect: Rect, radius: f32) {
    if !keyboard_mode(ui.ctx()) {
        return;
    }
    ui.painter().rect_stroke(
        rect.expand(3.0),
        radius + 3.0,
        Stroke::new(1.5, color_with_alpha(phase::accent(), 0.7)),
    );
}

fn button_content(
    ui: &Ui,
    rect: Rect,
    icon: Option<Icon>,
    label: &str,
    font: FontId,
    color: Color32,
    trailing_arrow: bool,
    nudge: f32,
) {
    let galley = ui.painter().layout_no_wrap(label.to_owned(), font, color);
    let icon_w = if icon.is_some() { 26.0 } else { 0.0 };
    let arrow_w = if trailing_arrow { 22.0 } else { 0.0 };
    let total = galley.size().x + icon_w + arrow_w;
    let mut x = rect.center().x - total / 2.0;
    if let Some(icon) = icon {
        glyph(
            ui.painter(),
            Pos2::new(x + 9.0, rect.center().y),
            icon,
            17.0,
            color,
        );
        x += icon_w;
    }
    let text_w = galley.size().x;
    ui.painter().galley(
        Pos2::new(x, rect.center().y - galley.size().y / 2.0),
        galley,
        color,
    );
    if trailing_arrow {
        glyph(
            ui.painter(),
            Pos2::new(x + text_w + 14.0 + nudge, rect.center().y),
            Icon::ArrowRight,
            15.0,
            color,
        );
    }
}

fn measure(ui: &Ui, label: &str, font: FontId) -> f32 {
    ui.painter()
        .layout_no_wrap(label.to_owned(), font, Color32::WHITE)
        .size()
        .x
}

/// Accent pill for the main action on a surface.
pub(super) fn primary_button(ui: &mut Ui, icon: Option<Icon>, label: &str) -> egui::Response {
    let font = semibold(TEXT_SIZE);
    let arrow = icon.is_none();
    let width =
        (measure(ui, label, font.clone()) + if icon.is_some() { 26.0 } else { 22.0 } + 48.0)
            .max(150.0)
            .min(ui.available_width());
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, 46.0), Sense::click());
    let enabled = ui.is_enabled();
    let hover = ui.ctx().animate_bool_with_time(
        response.id.with("hover"),
        enabled && response.hovered(),
        0.16,
    );
    let draw = pressed_rect(rect, press(ui, &response));
    let radius = draw.height() / 2.0;
    let opacity = if enabled { 1.0 } else { 0.4 };
    let fill = if enabled {
        lerp_color(phase::accent(), phase::accent_hover(), hover)
    } else {
        color_with_alpha(phase::surface_hover(), 0.7)
    };
    ui.painter().rect_filled(draw, radius, fill);
    ui.painter().rect_stroke(
        draw,
        radius,
        Stroke::new(1.0, color_with_alpha(Color32::WHITE, 0.14 * opacity)),
    );
    top_highlight(
        ui.painter(),
        draw,
        radius,
        color_with_alpha(Color32::WHITE, 0.45 * opacity),
    );
    button_content(
        ui,
        draw,
        icon,
        label,
        font,
        if enabled {
            phase::text_on_accent()
        } else {
            phase::text_muted()
        },
        arrow,
        3.0 * hover,
    );
    if response.has_focus() {
        focus_ring(ui, rect, radius);
    }
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, label));
    response
}

/// Outlined, translucent button for secondary actions.
pub(super) fn secondary_button(ui: &mut Ui, icon: Option<Icon>, label: &str) -> egui::Response {
    styled_button(ui, icon, label, 38.0, false)
}

/// Borderless text button for tertiary actions.
pub(super) fn quiet_button(ui: &mut Ui, icon: Option<Icon>, label: &str) -> egui::Response {
    styled_button(ui, icon, label, 34.0, true)
}

fn styled_button(
    ui: &mut Ui,
    icon: Option<Icon>,
    label: &str,
    height: f32,
    quiet: bool,
) -> egui::Response {
    let font = body(TEXT_SIZE);
    let width = (measure(ui, label, font.clone())
        + if icon.is_some() { 26.0 } else { 0.0 }
        + if quiet { 20.0 } else { 32.0 })
    .min(ui.available_width());
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, height), Sense::click());
    let enabled = ui.is_enabled();
    let hover = ui.ctx().animate_bool_with_time(
        response.id.with("hover"),
        enabled && response.hovered(),
        0.12,
    );
    let rect = pressed_rect(rect, press(ui, &response));
    let opacity = if enabled { 1.0 } else { 0.4 };
    let radius = CONTROL_RADIUS;
    if quiet {
        if hover > 0.0 {
            ui.painter().rect_filled(
                rect,
                radius,
                color_with_alpha(phase::surface_hover(), 0.55 * hover),
            );
        }
    } else {
        let fill = lerp_color(
            color_with_alpha(phase::surface_hover(), 0.45),
            color_with_alpha(phase::surface_hover(), 0.85),
            hover,
        );
        ui.painter()
            .rect_filled(rect, radius, color_with_alpha(fill, opacity));
        ui.painter().rect_stroke(
            rect,
            radius,
            Stroke::new(
                1.0,
                color_with_alpha(
                    lerp_color(phase::line(), phase::accent_hover(), hover * 0.6),
                    0.6 * opacity,
                ),
            ),
        );
        top_highlight(
            ui.painter(),
            rect,
            radius,
            color_with_alpha(Color32::WHITE, 0.06),
        );
    }
    let color = if quiet {
        lerp_color(phase::text_secondary(), phase::text(), hover)
    } else {
        phase::text()
    };
    button_content(
        ui,
        rect,
        icon,
        label,
        font,
        color_with_alpha(color, opacity),
        false,
        0.0,
    );
    if response.has_focus() {
        focus_ring(ui, rect, radius);
    }
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, label));
    response
}

/// Circular icon-only button.
pub(super) fn icon_button(
    ui: &mut Ui,
    icon: Icon,
    tooltip: &str,
    size: f32,
    accent: bool,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    let enabled = ui.is_enabled();
    let hover = ui.ctx().animate_bool_with_time(
        response.id.with("hover"),
        enabled && response.hovered(),
        0.12,
    );
    let opacity = if enabled { 1.0 } else { 0.4 };
    let r = size / 2.0 * (1.0 - 0.06 * press(ui, &response));
    if accent {
        ui.painter().circle_filled(
            rect.center(),
            r,
            color_with_alpha(
                lerp_color(phase::accent(), phase::accent_hover(), hover),
                opacity,
            ),
        );
        glyph(
            ui.painter(),
            rect.center(),
            icon,
            size * 0.42,
            color_with_alpha(phase::text_on_accent(), opacity),
        );
    } else {
        ui.painter().circle_filled(
            rect.center(),
            r,
            color_with_alpha(phase::surface_hover(), (0.35 + 0.5 * hover) * opacity),
        );
        glyph(
            ui.painter(),
            rect.center(),
            icon,
            size * 0.46,
            color_with_alpha(
                lerp_color(phase::text_secondary(), phase::text(), hover),
                opacity,
            ),
        );
    }
    if response.has_focus() {
        focus_ring(ui, rect, size / 2.0);
    }
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, tooltip));
    response.on_hover_text(tooltip)
}

// ---------------------------------------------------------------------------
// Navigation

/// The sidebar's single selection pill; it slides between items.
pub(super) fn nav_highlight(rect: Rect) -> egui::Shape {
    egui::Shape::Vec(vec![
        egui::Shape::rect_filled(rect, 12.0, color_with_alpha(phase::accent(), 0.15)),
        egui::Shape::rect_stroke(
            rect,
            12.0,
            Stroke::new(1.0, color_with_alpha(phase::accent(), 0.22)),
        ),
    ])
}

pub(super) fn nav_item(
    ui: &mut Ui,
    icon: Icon,
    label: &str,
    selected: bool,
    compact: bool,
) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 42.0), Sense::click());
    let hover =
        ui.ctx()
            .animate_bool_with_time(response.id.with("hover"), response.hovered(), 0.12);
    let sel = ui
        .ctx()
        .animate_bool_with_time(response.id.with("selected"), selected, 0.2);
    let painter = ui.painter();
    if hover > 0.0 {
        painter.rect_filled(
            rect,
            12.0,
            color_with_alpha(phase::surface_hover(), 0.45 * hover * (1.0 - sel)),
        );
    }
    let icon_color = lerp_color(
        lerp_color(phase::text_muted(), phase::text_secondary(), hover),
        phase::accent_hover(),
        sel,
    );
    let text_color = lerp_color(phase::text_secondary(), phase::text(), sel.max(hover));
    if compact {
        glyph(painter, rect.center(), icon, 20.0, icon_color);
    } else {
        glyph(
            painter,
            Pos2::new(rect.left() + 22.0, rect.center().y),
            icon,
            19.0,
            icon_color,
        );
        painter.text(
            Pos2::new(rect.left() + 44.0, rect.center().y),
            Align2::LEFT_CENTER,
            label,
            if selected {
                semibold(TEXT_SIZE)
            } else {
                body(TEXT_SIZE)
            },
            text_color,
        );
    }
    if response.has_focus() {
        focus_ring(ui, rect, 12.0);
    }
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, selected, label)
    });
    if compact {
        response.on_hover_text(label)
    } else {
        response
    }
}

/// Avatar, name and caption; the sidebar's account entry.
pub(super) fn account_chip(
    ui: &mut Ui,
    texture: Option<&TextureHandle>,
    name: &str,
    detail: &str,
    compact: bool,
) -> egui::Response {
    let height = 52.0;
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), height), Sense::click());
    let hover =
        ui.ctx()
            .animate_bool_with_time(response.id.with("hover"), response.hovered(), 0.12);
    ui.painter().rect_filled(
        rect,
        14.0,
        color_with_alpha(phase::surface_hover(), 0.18 + 0.35 * hover),
    );
    let avatar_rect = if compact {
        Rect::from_center_size(rect.center(), Vec2::splat(32.0))
    } else {
        Rect::from_center_size(
            Pos2::new(rect.left() + 26.0, rect.center().y),
            Vec2::splat(32.0),
        )
    };
    paint_avatar(ui, avatar_rect, texture, name);
    if !compact {
        let width = rect.right() - avatar_rect.right() - 20.0;
        let name_g = truncated(ui, name, semibold(13.0), phase::text(), width);
        let detail_g = truncated(ui, detail, body(12.0), phase::text_muted(), width);
        let x = avatar_rect.right() + 10.0;
        let top = rect.center().y - (name_g.size().y + detail_g.size().y) / 2.0;
        ui.painter()
            .galley(Pos2::new(x, top), name_g.clone(), phase::text());
        ui.painter().galley(
            Pos2::new(x, top + name_g.size().y),
            detail_g,
            phase::text_muted(),
        );
    }
    if response.has_focus() {
        focus_ring(ui, rect, 14.0);
    }
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, name));
    if compact {
        response.on_hover_text(format!("{name}\n{detail}"))
    } else {
        response
    }
}

// ---------------------------------------------------------------------------
// Composite widgets

/// Clickable overview tile: icon, label and a one-line value.
pub(super) fn status_tile(
    ui: &mut Ui,
    icon: Icon,
    label: &str,
    value: &str,
    tone: Tone,
) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 112.0), Sense::click());
    let hover =
        ui.ctx()
            .animate_bool_with_time(response.id.with("hover"), response.hovered(), 0.14);
    let draw = rect.translate(Vec2::new(0.0, -2.0 * hover));
    let painter = ui.painter();
    painter.add(egui::Shape::mesh(
        soft_shadow().tessellate(draw, CARD_RADIUS),
    ));
    painter.rect_filled(
        draw,
        CARD_RADIUS,
        lerp_color(
            card_fill(),
            color_with_alpha(phase::surface_hover(), 0.85),
            hover * 0.5,
        ),
    );
    painter.rect_stroke(
        draw,
        CARD_RADIUS,
        Stroke::new(
            1.0,
            lerp_color(
                card_stroke().color,
                color_with_alpha(phase::accent(), 0.45),
                hover,
            ),
        ),
    );
    top_highlight(painter, draw, CARD_RADIUS, rim());
    let color = tone_color(tone);
    let chip = Rect::from_min_size(draw.min + Vec2::new(16.0, 16.0), Vec2::splat(34.0));
    paint_icon_chip(painter, chip, icon, phase::accent_hover());
    // Status mark, top-right.
    let mark = Pos2::new(draw.right() - 24.0, draw.top() + 24.0);
    match tone {
        Tone::Good => {
            painter.circle_filled(mark, 10.0, color);
            glyph(painter, mark, Icon::Check, 12.0, phase::background());
        }
        Tone::Attention | Tone::Danger => {
            painter.circle_filled(mark, 10.0, color);
            glyph(painter, mark, Icon::Warning, 12.0, phase::background());
        }
        _ => {
            painter.circle_filled(mark, 4.0, color);
        }
    }
    let width = draw.width() - 32.0;
    let label_g = truncated(ui, label, body(CAPTION_SIZE), phase::text_muted(), width);
    let value_g = truncated(ui, value, semibold(TEXT_SIZE + 1.0), phase::text(), width);
    let painter = ui.painter();
    painter.galley(
        draw.min + Vec2::new(16.0, 62.0),
        label_g,
        phase::text_muted(),
    );
    painter.galley(draw.min + Vec2::new(16.0, 80.0), value_g, phase::text());
    // Chevron fades in on hover to signal the tile is actionable.
    if hover > 0.0 {
        glyph(
            painter,
            Pos2::new(
                draw.right() - 20.0 + 2.0 * (1.0 - hover),
                draw.bottom() - 22.0,
            ),
            Icon::CaretRight,
            14.0,
            color_with_alpha(phase::text_secondary(), hover),
        );
    }
    if response.has_focus() {
        focus_ring(ui, rect, CARD_RADIUS);
    }
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, format!("{label}: {value}"))
    });
    response
}

/// Tinted notice with an icon; used for errors and important context.
pub(super) fn banner(ui: &mut Ui, tone: Tone, icon: Icon, message: &str) {
    let color = tone_color(tone);
    egui::Frame::none()
        .fill(color_with_alpha(color, 0.10))
        .stroke(Stroke::new(1.0, color_with_alpha(color, 0.30)))
        .rounding(Rounding::same(CONTROL_RADIUS + 2.0))
        .inner_margin(Margin::symmetric(14.0, 11.0))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                let (rect, _) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::hover());
                glyph(ui.painter(), rect.center(), icon, 17.0, color);
                ui.add(
                    egui::Label::new(RichText::new(message).size(13.5).color(phase::text()))
                        .wrap(true),
                );
            });
        });
}

/// Rounded progress track with an accent fill.
pub(super) fn progress(ui: &mut Ui, value: f32, width: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 8.0), Sense::hover());
    ui.painter()
        .rect_filled(rect, 4.0, color_with_alpha(phase::text(), 0.10));
    let t = value.clamp(0.0, 1.0);
    if t > 0.0 {
        let fill = Rect::from_min_size(
            rect.min,
            Vec2::new((rect.width() * t).max(8.0), rect.height()),
        );
        ui.painter().rect_filled(fill, 4.0, phase::accent());
        // A travelling highlight keeps long downloads feeling alive.
        let time = ui.input(|i| i.time) as f32;
        let x = fill.left() + ((time * 0.8).fract()) * fill.width();
        let shine = Rect::from_center_size(
            Pos2::new(x, fill.center().y),
            Vec2::new(28.0, fill.height()),
        )
        .intersect(fill);
        if shine.width() > 0.0 {
            ui.painter()
                .rect_filled(shine, 4.0, color_with_alpha(Color32::WHITE, 0.22));
        }
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(16));
    }
}

/// A row inside a `group`: icon chip, title/detail, trailing controls.
pub(super) fn row(
    ui: &mut Ui,
    icon: Icon,
    heading: &str,
    detail: &str,
    trailing_width: f32,
    trailing: impl FnOnce(&mut Ui),
) {
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 12.0;
        icon_chip(ui, icon, phase::accent_hover(), 34.0);
        let text_width = (ui.available_width() - trailing_width - 12.0).max(80.0);
        ui.vertical(|ui| {
            ui.set_width(text_width);
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.label(
                RichText::new(heading)
                    .font(semibold(TEXT_SIZE))
                    .color(phase::text()),
            );
            if !detail.is_empty() {
                ui.add(
                    egui::Label::new(
                        RichText::new(detail)
                            .size(CAPTION_SIZE + 0.5)
                            .color(phase::text_secondary()),
                    )
                    .wrap(true),
                );
            }
        });
        ui.with_layout(egui::Layout::right_to_left(Align::Center), trailing);
    });
    ui.add_space(10.0);
}

/// An inspector line: label and hint on the left, a control on the right.
pub(super) fn setting_line(
    ui: &mut Ui,
    label: &str,
    detail: &str,
    trailing_width: f32,
    trailing: impl FnOnce(&mut Ui),
) {
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        let text_width = (ui.available_width() - trailing_width - 16.0).max(90.0);
        ui.vertical(|ui| {
            ui.set_width(text_width);
            ui.spacing_mut().item_spacing.y = 1.0;
            ui.label(
                RichText::new(label)
                    .font(semibold(TEXT_SIZE))
                    .color(phase::text()),
            );
            if !detail.is_empty() {
                ui.add(
                    egui::Label::new(
                        RichText::new(detail)
                            .size(CAPTION_SIZE)
                            .color(phase::text_muted()),
                    )
                    .wrap(true),
                );
            }
        });
        ui.with_layout(egui::Layout::right_to_left(Align::Center), trailing);
    });
    ui.add_space(8.0);
}

/// Compact field for a single value, with an optional unit inside it.
/// Numbers sit right-aligned against their unit.
pub(super) fn value_field(
    ui: &mut Ui,
    value: &mut String,
    hint: &str,
    unit: &str,
    width: f32,
    numeric: bool,
    secret: bool,
) -> egui::Response {
    let edit_id = ui.id().with(("value-field", hint, unit));
    let focused = ui.memory(|m| m.focused() == Some(edit_id));
    let focus = ui
        .ctx()
        .animate_bool_with_time(edit_id.with("focus"), focused, 0.12);
    egui::Frame::none()
        .fill(color_with_alpha(phase::input(), 0.9))
        .stroke(Stroke::new(
            1.0,
            lerp_color(color_with_alpha(phase::line(), 0.5), phase::accent(), focus),
        ))
        .rounding(Rounding::same(8.0))
        .inner_margin(Margin::symmetric(10.0, 0.0))
        .show(ui, |ui| {
            ui.set_width(width - 20.0);
            // Explicit direction: rows lay their controls out right-to-left.
            ui.allocate_ui_with_layout(
                Vec2::new(width - 20.0, 34.0),
                egui::Layout::left_to_right(Align::Center),
                |ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    let unit_width = if unit.is_empty() {
                        0.0
                    } else {
                        measure(ui, unit, body(CAPTION_SIZE)) + 6.0
                    };
                    let response = ui.add(
                        egui::TextEdit::singleline(value)
                            .id(edit_id)
                            .frame(false)
                            .font(FontId::proportional(TEXT_SIZE))
                            .desired_width((width - 20.0 - unit_width).max(20.0))
                            .margin(Vec2::new(0.0, 8.0))
                            .horizontal_align(if numeric { Align::RIGHT } else { Align::LEFT })
                            .password(secret)
                            .hint_text(RichText::new(hint).color(phase::text_muted())),
                    );
                    if !unit.is_empty() {
                        ui.label(
                            RichText::new(unit)
                                .size(CAPTION_SIZE)
                                .color(phase::text_muted()),
                        );
                    }
                    response
                },
            )
            .inner
        })
        .inner
}

/// Pill-style single choice with a sliding selection.
pub(super) fn segmented<T: Copy + PartialEq>(
    ui: &mut Ui,
    id: &str,
    options: &[(T, &str)],
    value: &mut T,
) -> bool {
    let font = body(13.0);
    let widths: Vec<f32> = options
        .iter()
        .map(|(_, label)| measure(ui, label, font.clone()) + 26.0)
        .collect();
    let total: f32 = widths.iter().sum::<f32>() + 8.0;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(total, 36.0), Sense::hover());
    ui.painter()
        .rect_filled(rect, 11.0, color_with_alpha(phase::input(), 0.9));
    ui.painter().rect_stroke(
        rect,
        11.0,
        Stroke::new(1.0, color_with_alpha(phase::line(), 0.45)),
    );
    let mut x = rect.left() + 4.0;
    let mut changed = false;
    let mut target = x;
    let mut target_w = widths.first().copied().unwrap_or(0.0);
    let mut segments = Vec::new();
    for ((option, label), w) in options.iter().zip(&widths) {
        let seg = Rect::from_min_size(Pos2::new(x, rect.top() + 4.0), Vec2::new(*w, 28.0));
        if *option == *value {
            target = x;
            target_w = *w;
        }
        segments.push((seg, *option, *label));
        x += w;
    }
    let base = egui::Id::new(("segmented", id));
    // Spring in local coordinates so resizing the window doesn't animate.
    let sx = rect.left() + spring(ui.ctx(), base.with("x"), target - rect.left());
    let sw = spring(ui.ctx(), base.with("w"), target_w);
    if options.iter().any(|(option, _)| *option == *value) {
        let pill = Rect::from_min_size(Pos2::new(sx, rect.top() + 4.0), Vec2::new(sw, 28.0));
        ui.painter().rect_filled(pill, 8.0, phase::surface_active());
        ui.painter().rect_stroke(
            pill,
            8.0,
            Stroke::new(1.0, color_with_alpha(phase::accent(), 0.35)),
        );
    }
    for (seg, option, label) in segments {
        let response = ui.interact(seg, base.with(label), Sense::click());
        let selected = option == *value;
        let hover = response.hovered() && !selected;
        ui.painter().text(
            seg.center(),
            Align2::CENTER_CENTER,
            label,
            if selected {
                semibold(13.0)
            } else {
                font.clone()
            },
            if selected || hover {
                phase::text()
            } else {
                phase::text_secondary()
            },
        );
        if response.clicked() && !selected {
            *value = option;
            changed = true;
        }
        response.widget_info(|| {
            egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, selected, label)
        });
    }
    changed
}

/// Build a closed polyline around a rounded rectangle.
fn rounded_outline(rect: Rect, radius: f32) -> Vec<Pos2> {
    let r = radius.min(rect.width() / 2.0).min(rect.height() / 2.0);
    let corners = [
        (Pos2::new(rect.right() - r, rect.top() + r), -90.0_f32),
        (Pos2::new(rect.right() - r, rect.bottom() - r), 0.0),
        (Pos2::new(rect.left() + r, rect.bottom() - r), 90.0),
        (Pos2::new(rect.left() + r, rect.top() + r), 180.0),
    ];
    let mut points = Vec::new();
    for (center, start) in corners {
        for step in 0..=8 {
            let a = (start + 90.0 * step as f32 / 8.0).to_radians();
            points.push(center + Vec2::new(a.cos(), a.sin()) * r);
        }
    }
    points.push(points[0]);
    points
}

/// Large dashed drop target for video files.
pub(super) fn drop_zone(ui: &mut Ui) -> egui::Response {
    let hovering_file = ui.input(|i| !i.raw.hovered_files.is_empty());
    let height = (ui.available_height() * 0.42).clamp(170.0, 230.0);
    let (slot, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), height), Sense::click());
    let active = response.hovered() || response.has_focus() || hovering_file;
    let hover = ui
        .ctx()
        .animate_bool_with_time(response.id.with("hover"), active, 0.18);
    // A dragged file lifts the target toward the pointer.
    let lift = spring(
        ui.ctx(),
        response.id.with("lift"),
        if hovering_file { 1.0 } else { 0.0 },
    );
    let rect = pressed_rect(slot, press(ui, &response))
        .translate(Vec2::new(0.0, -4.0 * lift))
        .expand(3.0 * lift);
    let painter = ui.painter();
    if lift > 0.01 {
        painter.add(egui::Shape::mesh(
            egui::epaint::Shadow {
                offset: Vec2::new(0.0, 10.0 * lift),
                blur: 28.0,
                spread: -8.0,
                color: Color32::from_black_alpha((80.0 * lift.clamp(0.0, 1.0)) as u8),
            }
            .tessellate(rect, 20.0),
        ));
    }
    painter.rect_filled(
        rect,
        20.0,
        lerp_color(
            color_with_alpha(phase::surface(), 0.38),
            color_with_alpha(phase::surface_hover(), 0.45),
            hover,
        ),
    );
    let march = if hovering_file {
        ui.ctx().request_repaint();
        (ui.input(|i| i.time) as f32 * 18.0) % 13.0
    } else {
        0.0
    };
    painter.extend(egui::Shape::dashed_line_with_offset(
        &rounded_outline(rect.shrink(0.5), 20.0),
        Stroke::new(
            1.5,
            lerp_color(
                color_with_alpha(phase::line(), 0.8),
                phase::accent_hover(),
                hover,
            ),
        ),
        &[7.0],
        &[6.0],
        march,
    ));
    let center = rect.center() - Vec2::new(0.0, 26.0);
    painter.circle_filled(center, 28.0, color_with_alpha(phase::surface_hover(), 0.7));
    glyph(painter, center, Icon::UploadSimple, 24.0, phase::text());
    painter.text(
        rect.center() + Vec2::new(0.0, 26.0),
        Align2::CENTER_CENTER,
        if hovering_file {
            "Release to load"
        } else {
            "Drop a video here"
        },
        semibold(16.0),
        phase::text(),
    );
    painter.text(
        rect.center() + Vec2::new(0.0, 50.0),
        Align2::CENTER_CENTER,
        "MP4, MOV, M4V or WebM  ·  or click to browse",
        body(CAPTION_SIZE + 0.5),
        phase::text_muted(),
    );
    if response.has_focus() {
        focus_ring(ui, rect, 20.0);
    }
    response
        .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, "Choose a video file"));
    response
}

/// Timeline scrubber. Returns true when the user moved it.
pub(super) fn scrubber(ui: &mut Ui, value: &mut f64, max: f64) -> bool {
    let (rect, response) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), 24.0),
        Sense::click_and_drag(),
    );
    let track = Rect::from_center_size(rect.center(), Vec2::new(rect.width() - 14.0, 6.0));
    let mut changed = false;
    if let Some(pos) = response.interact_pointer_pos()
        && (response.dragged() || response.clicked())
    {
        let t = ((pos.x - track.left()) / track.width()).clamp(0.0, 1.0) as f64;
        *value = t * max;
        changed = true;
    }
    let t = if max > 0.0 {
        (*value / max).clamp(0.0, 1.0) as f32
    } else {
        0.0
    };
    let hover = ui.ctx().animate_bool_with_time(
        response.id.with("hover"),
        response.hovered() || response.dragged(),
        0.12,
    );
    ui.painter()
        .rect_filled(track, 3.0, color_with_alpha(phase::text(), 0.12));
    let fill = Rect::from_min_size(track.min, Vec2::new(track.width() * t, track.height()));
    ui.painter().rect_filled(fill, 3.0, phase::accent());
    let knob = Pos2::new(track.left() + track.width() * t, track.center().y);
    ui.painter()
        .circle_filled(knob, 11.0 * hover, color_with_alpha(phase::accent(), 0.2));
    ui.painter()
        .circle_filled(knob, 7.0 + 1.0 * hover, phase::text());
    ui.painter()
        .circle_stroke(knob, 7.0 + 1.0 * hover, Stroke::new(2.0, phase::accent()));
    response
        .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Slider, "Playback position"));
    changed
}

// ---------------------------------------------------------------------------
// Inputs

/// Rounded search/link field with a leading icon. `trailing` renders inside
/// the field on the right.
pub(super) fn field_with_icon(
    ui: &mut Ui,
    icon: Icon,
    value: &mut String,
    hint: &str,
    trailing: impl FnOnce(&mut Ui) -> bool,
) -> (egui::Response, bool) {
    let focused_id = ui.id().with(("field", hint));
    let focus = ui.ctx().animate_bool_with_time(
        focused_id,
        ui.memory(|m| m.focused().is_some_and(|f| f == focused_id.with("edit"))),
        0.14,
    );
    let mut submitted = false;
    let inner = egui::Frame::none()
        .fill(color_with_alpha(phase::input(), 0.92))
        .stroke(Stroke::new(
            1.0,
            lerp_color(
                color_with_alpha(phase::line(), 0.55),
                phase::accent(),
                focus,
            ),
        ))
        .rounding(Rounding::same(14.0))
        .inner_margin(Margin {
            left: 14.0,
            right: 6.0,
            top: 6.0,
            bottom: 6.0,
        })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                let (rect, _) = ui.allocate_exact_size(Vec2::new(18.0, 34.0), Sense::hover());
                glyph(ui.painter(), rect.center(), icon, 17.0, phase::text_muted());

                ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                    submitted = trailing(ui);
                    ui.add(
                        egui::TextEdit::singleline(value)
                            .id(focused_id.with("edit"))
                            .frame(false)
                            .font(FontId::proportional(TEXT_SIZE))
                            .desired_width(ui.available_width())
                            .margin(Vec2::new(0.0, 8.0))
                            .hint_text(RichText::new(hint).color(phase::text_muted())),
                    )
                })
                .inner
            })
            .inner
        });
    (inner.inner, submitted)
}

pub(super) fn toggle(
    ui: &mut Ui,
    value: &mut bool,
    label: &str,
    detail: &str,
    enabled: bool,
) -> bool {
    let mut changed = false;
    ui.add_enabled_ui(enabled, |ui| {
        let width = ui.available_width();
        let text_width = (width - 64.0).max(1.0);
        let opacity = if ui.is_enabled() { 1.0 } else { 0.45 };
        let heading = ui.painter().layout(
            label.to_owned(),
            semibold(TEXT_SIZE),
            color_with_alpha(phase::text(), opacity),
            text_width,
        );
        let description = ui.painter().layout(
            detail.to_owned(),
            body(CAPTION_SIZE + 0.5),
            color_with_alpha(phase::text_secondary(), opacity),
            text_width,
        );
        let height = heading.size().y
            + if detail.is_empty() {
                0.0
            } else {
                description.size().y + 2.0
            };
        let (rect, mut response) = ui.allocate_exact_size(
            Vec2::new(width, height.max(CONTROL_HEIGHT) + 20.0),
            Sense::click(),
        );
        if response.clicked() && ui.is_enabled() {
            response.request_focus();
            *value = !*value;
            changed = true;
            response.mark_changed();
        }
        response
            .widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, *value, label));
        let top = rect.center().y - height / 2.0;
        ui.painter()
            .galley(Pos2::new(rect.left(), top), heading.clone(), phase::text());
        if !detail.is_empty() {
            ui.painter().galley(
                Pos2::new(rect.left(), top + heading.size().y + 2.0),
                description,
                phase::text_secondary(),
            );
        }
        let on = spring(
            ui.ctx(),
            response.id.with("on"),
            if *value { 1.0 } else { 0.0 },
        );
        let hover = ui.ctx().animate_bool_with_time(
            response.id.with("hover"),
            response.hovered() && ui.is_enabled(),
            0.12,
        );
        let track = Rect::from_center_size(
            Pos2::new(rect.right() - 24.0, rect.center().y),
            Vec2::new(44.0, 26.0),
        );
        ui.painter().rect_filled(
            track,
            13.0,
            color_with_alpha(
                lerp_color(
                    lerp_color(phase::surface_hover(), phase::surface_active(), hover),
                    phase::accent(),
                    on,
                ),
                opacity,
            ),
        );
        ui.painter().rect_stroke(
            track,
            13.0,
            Stroke::new(1.0, color_with_alpha(Color32::WHITE, 0.08 * opacity)),
        );
        let knob_x = egui::lerp(track.left() + 13.0..=track.right() - 13.0, on);
        let on = on.clamp(0.0, 1.0);
        ui.painter().circle_filled(
            Pos2::new(knob_x, track.center().y + 1.0),
            10.0,
            Color32::from_black_alpha((50.0 * opacity) as u8),
        );
        ui.painter().circle_filled(
            Pos2::new(knob_x, track.center().y),
            10.0,
            color_with_alpha(
                lerp_color(phase::text_secondary(), phase::text_on_accent(), on)
                    .gamma_multiply(1.0),
                opacity,
            ),
        );
        if response.has_focus() {
            focus_ring(ui, track, 13.0);
        }
    });
    changed
}

// ---------------------------------------------------------------------------
// Pickers

#[cfg(test)]
pub(super) fn dropdown_choices(ui: &mut Ui) {
    // Native combo popups already reserve the selector width minus their frame.
    compact_menu_width(ui, ui.available_width());
}

fn compact_menu_width(ui: &mut Ui, width: f32) {
    let row_rounding = ui.visuals().menu_rounding;
    let widgets = &mut ui.visuals_mut().widgets;
    for widget in [
        &mut widgets.noninteractive,
        &mut widgets.inactive,
        &mut widgets.hovered,
        &mut widgets.active,
        &mut widgets.open,
    ] {
        widget.rounding = row_rounding;
    }
    ui.set_min_width(width);
    ui.set_max_width(width);
    ui.style_mut().wrap = Some(true);
    ui.spacing_mut().menu_width = width;
    ui.spacing_mut().interact_size.y = 30.0;
    ui.spacing_mut().button_padding = Vec2::new(8.0, 5.0);
    ui.spacing_mut().item_spacing.y = 3.0;
}

/// A selector button: optional thumbnail, value and caret.
pub(super) fn select_button(
    ui: &mut Ui,
    thumbnail: Option<&TextureHandle>,
    swatch: Option<Color32>,
    value: &str,
    width: f32,
    open: bool,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, 40.0), Sense::click());
    let hover = ui.ctx().animate_bool_with_time(
        response.id.with("hover"),
        response.hovered() || open,
        0.12,
    );
    ui.painter().rect_filled(
        rect,
        CONTROL_RADIUS + 1.0,
        color_with_alpha(phase::input(), 0.92),
    );
    ui.painter().rect_stroke(
        rect,
        CONTROL_RADIUS + 1.0,
        Stroke::new(
            1.0,
            lerp_color(
                color_with_alpha(phase::line(), 0.55),
                phase::accent(),
                hover * 0.8,
            ),
        ),
    );
    let mut x = rect.left() + 10.0;
    let thumb = Rect::from_min_size(Pos2::new(x, rect.center().y - 11.0), Vec2::new(34.0, 22.0));
    if let Some(texture) = thumbnail {
        egui::Image::new(texture).rounding(5.0).paint_at(ui, thumb);
        x = thumb.right() + 10.0;
    } else if let Some(color) = swatch {
        ui.painter().rect_filled(thumb, 5.0, color);
        ui.painter().rect_stroke(
            thumb,
            5.0,
            Stroke::new(1.0, color_with_alpha(Color32::WHITE, 0.15)),
        );
        x = thumb.right() + 10.0;
    } else {
        x += 2.0;
    }
    let galley = truncated(
        ui,
        value,
        body(TEXT_SIZE),
        phase::text(),
        rect.right() - x - 34.0,
    );
    ui.painter().galley(
        Pos2::new(x, rect.center().y - galley.size().y / 2.0),
        galley,
        phase::text(),
    );
    glyph(
        ui.painter(),
        Pos2::new(rect.right() - 18.0, rect.center().y),
        Icon::CaretDown,
        14.0,
        phase::text_secondary(),
    );
    if response.has_focus() {
        focus_ring(ui, rect, CONTROL_RADIUS + 1.0);
    }
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::ComboBox, value));
    response
}

/// Unlike egui 0.27's combo popup, clicks inside the picker keep it open.
pub(super) fn picker<R>(
    ui: &Ui,
    trigger: &egui::Response,
    id: egui::Id,
    contents: impl FnOnce(&mut Ui) -> R,
) -> Option<R> {
    if !ui.memory(|m| m.is_popup_open(id)) {
        return None;
    }
    let popup = egui::Area::new(id)
        .order(egui::Order::Foreground)
        .fixed_pos(trigger.rect.left_bottom() + Vec2::new(0.0, 6.0))
        .constrain(true)
        .constrain_to(ui.ctx().screen_rect().shrink(8.0))
        .show(ui.ctx(), |ui| {
            let frame = egui::Frame::popup(ui.style())
                .fill(lerp_color(phase::background(), phase::surface(), 0.9))
                .rounding(Rounding::same(14.0))
                .inner_margin(Margin::same(8.0))
                .shadow(egui::epaint::Shadow {
                    offset: Vec2::new(0.0, 12.0),
                    blur: 32.0,
                    spread: 0.0,
                    color: Color32::from_black_alpha(110),
                });
            let width = (trigger.rect.width() - frame.total_margin().sum().x).max(1.0);
            frame
                .show(ui, |ui| {
                    ui.with_layout(egui::Layout::top_down(Align::LEFT), |ui| {
                        compact_menu_width(ui, width);
                        contents(ui)
                    })
                    .inner
                })
                .inner
        });
    let dismiss = ui.input(|i| {
        i.key_pressed(egui::Key::Escape)
            || (i.pointer.any_click()
                && i.pointer
                    .interact_pos()
                    .is_some_and(|p| !trigger.rect.contains(p) && !popup.response.rect.contains(p)))
    });
    if dismiss {
        ui.memory_mut(|m| m.close_popup());
    }
    Some(popup.inner)
}

pub(super) fn theme_search(ui: &mut Ui, text: &mut String) -> egui::Response {
    egui::Frame::none()
        .fill(phase::input())
        .stroke(Stroke::new(1.0, color_with_alpha(phase::line(), 0.6)))
        .rounding(Rounding::same(CONTROL_RADIUS))
        .inner_margin(Margin::symmetric(10.0, 5.0))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                draw_icon(ui, Icon::Search, Vec2::splat(16.0), phase::text_muted());
                ui.add_sized(
                    Vec2::new(ui.available_width(), 24.0),
                    egui::TextEdit::singleline(text)
                        .frame(false)
                        .hint_text(RichText::new("Find a theme").color(phase::text_muted())),
                )
            })
            .inner
        })
        .inner
}

pub(super) fn theme_choice(
    ui: &mut Ui,
    title: &str,
    preview: Option<&TextureHandle>,
    selected: bool,
    fallback: Color32,
) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 48.0), Sense::click());
    if selected || response.hovered() || response.has_focus() {
        ui.painter().rect_filled(
            rect,
            CONTROL_RADIUS,
            if selected {
                color_with_alpha(phase::accent(), 0.16)
            } else {
                color_with_alpha(phase::surface_hover(), 0.7)
            },
        );
    }
    let image_rect = Rect::from_min_size(rect.min + Vec2::new(6.0, 8.0), Vec2::new(52.0, 32.0));
    if let Some(texture) = preview {
        egui::Image::new(texture)
            .rounding(6.0)
            .paint_at(ui, image_rect);
    } else {
        ui.painter().rect_filled(image_rect, 6.0, fallback);
    }
    ui.painter().rect_stroke(
        image_rect,
        6.0,
        Stroke::new(1.0, color_with_alpha(Color32::WHITE, 0.12)),
    );
    let galley = truncated(
        ui,
        title,
        body(TEXT_SIZE),
        phase::text(),
        rect.width() - 100.0,
    );
    ui.painter().galley(
        Pos2::new(rect.left() + 70.0, rect.center().y - galley.size().y / 2.0),
        galley,
        phase::text(),
    );
    if selected {
        glyph(
            ui.painter(),
            Pos2::new(rect.right() - 18.0, rect.center().y),
            Icon::Check,
            15.0,
            phase::accent_hover(),
        );
    }
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, selected, title)
    });
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dropdown_panels_match_their_selectors() {
        for width in [180.0, 220.0, 320.0] {
            for custom in [false, true] {
                let ctx = Context::default();
                configure_style(&ctx);
                let mut trigger_rect = Rect::NOTHING;
                let mut popup_id = egui::Id::NULL;
                for _ in 0..3 {
                    let _ = ctx.run(
                        egui::RawInput {
                            screen_rect: Some(Rect::from_min_size(
                                Pos2::ZERO,
                                Vec2::new(560.0, 480.0),
                            )),
                            ..Default::default()
                        },
                        |ctx| {
                            egui::CentralPanel::default().show(ctx, |ui| {
                                if custom {
                                    popup_id = ui.make_persistent_id("custom-width");
                                    ui.memory_mut(|m| m.open_popup(popup_id));
                                    let trigger =
                                        select_button(ui, None, None, "Theme", width, true);
                                    trigger_rect = trigger.rect;
                                    picker(ui, &trigger, popup_id, |ui| {
                                        theme_search(ui, &mut String::new());
                                        theme_choice(
                                            ui,
                                            "A theme with a long display name",
                                            None,
                                            true,
                                            phase::surface(),
                                        );
                                    });
                                } else {
                                    popup_id = ui
                                        .make_persistent_id(egui::Id::new("native-width"))
                                        .with("popup");
                                    ui.memory_mut(|m| m.open_popup(popup_id));
                                    trigger_rect = egui::ComboBox::from_id_source("native-width")
                                        .width(width)
                                        .selected_text("Crop")
                                        .show_ui(ui, |ui| {
                                            dropdown_choices(ui);
                                            for label in ["Crop", "Fit", "Stretch"] {
                                                ui.selectable_label(label == "Crop", label);
                                            }
                                        })
                                        .response
                                        .rect;
                                }
                            });
                        },
                    );
                }
                let popup = ctx.memory(|m| m.area_rect(popup_id).unwrap());
                assert!(
                    (popup.width() - trigger_rect.width()).abs() <= 1.0,
                    "custom={custom}, selector={trigger_rect:?}, panel={popup:?}"
                );
            }
        }
    }

    #[test]
    fn theme_picker_search_stays_open_while_typing_and_closes_outside() {
        let ctx = Context::default();
        configure_style(&ctx);
        let id = egui::Id::new("search-regression");
        ctx.memory_mut(|m| m.open_popup(id));
        let mut text = String::new();
        let render = |events: Vec<egui::Event>, text: &mut String| {
            let mut search = Rect::NOTHING;
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(560.0, 480.0))),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        let trigger = select_button(ui, None, None, "Theme", 220.0, true);
                        picker(ui, &trigger, id, |ui| {
                            search = theme_search(ui, text).rect;
                        });
                    });
                },
            );
            search
        };
        render(vec![], &mut text);
        let search = render(vec![], &mut text);
        let click = |pos| {
            vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: Default::default(),
                },
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: Default::default(),
                },
            ]
        };
        render(click(search.center()), &mut text);
        assert!(
            ctx.memory(|m| m.is_popup_open(id)),
            "search click must not dismiss picker"
        );
        render(vec![egui::Event::Text("Tusk".into())], &mut text);
        assert_eq!(text, "Tusk");
        assert!(ctx.memory(|m| m.is_popup_open(id)));
        render(click(Pos2::new(540.0, 450.0)), &mut text);
        assert!(
            !ctx.memory(|m| m.is_popup_open(id)),
            "outside click dismisses picker"
        );
    }

    #[test]
    fn switches_support_pointer_keyboard_and_disabled_state() {
        let ctx = Context::default();
        configure_style(&ctx);
        let mut value = false;
        let render = |events: Vec<egui::Event>, value: &mut bool, enabled: bool| {
            let mut bounds = Rect::NOTHING;
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(400.0, 200.0))),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        bounds = ui
                            .scope(|ui| {
                                toggle(ui, value, "Sync playback", "", enabled);
                            })
                            .response
                            .rect;
                    });
                },
            );
            bounds
        };
        let bounds = render(Vec::new(), &mut value, true);
        let pos = Pos2::new(bounds.right() - 24.0, bounds.center().y);
        let click = || {
            vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::default(),
                },
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::default(),
                },
            ]
        };
        render(click(), &mut value, true);
        assert!(value, "pointer click should turn the switch on");
        render(
            vec![egui::Event::Key {
                key: egui::Key::Space,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::default(),
            }],
            &mut value,
            true,
        );
        assert!(!value, "space should activate the focused switch");
        render(click(), &mut value, false);
        assert!(!value, "disabled switches must ignore pointer input");
    }

    #[test]
    fn controls_fit_at_all_supported_widths() {
        for width in [300.0, 420.0, 680.0, 900.0] {
            let ctx = Context::default();
            configure_style(&ctx);
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(width, 1200.0))),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        let right = ui.available_rect_before_wrap().right();
                        let mut value = "Very long path/".repeat(40);
                        assert!(
                            value_field(ui, &mut value, "", "s", 96.0, true, false)
                                .rect
                                .right()
                                <= right + 1.0
                        );
                        assert!(
                            primary_button(ui, None, "Update phase").rect.right() <= right + 1.0
                        );
                        assert!(
                            secondary_button(ui, Some(Icon::Refresh), "Check again")
                                .rect
                                .right()
                                <= right + 1.0
                        );
                        assert!(
                            status_tile(ui, Icon::Folder, "Location", &value, Tone::Good)
                                .rect
                                .right()
                                <= right + 1.0
                        );
                        let mut checked = false;
                        let short = ui
                            .scope(|ui| {
                                toggle(ui, &mut checked, "Backup", "Short description", true);
                            })
                            .response
                            .rect;
                        let long = ui
                            .scope(|ui| {
                                toggle(
                                    ui,
                                    &mut checked,
                                    "Backup",
                                    &"Long description ".repeat(30),
                                    true,
                                );
                            })
                            .response
                            .rect;
                        assert!(long.height() > short.height());
                        assert!(long.right() <= right + 1.0);
                    });
                },
            );
        }
    }

    #[test]
    fn status_colors_stay_visible_on_themes_that_use_them_as_tints() {
        // Blackhole Halo: Green #242E24 on Background #0A0A09.
        let background = Color32::from_rgb(0x0A, 0x0A, 0x09);
        phase::reset_palette();
        let dim = Color32::from_rgb(0x24, 0x2E, 0x24);
        assert!(contrast(dim, background) < 3.0);
        assert!(contrast(readable(dim), phase::background()) >= 3.0);
        let lifted = egui::ecolor::Hsva::from(readable(dim));
        let original = egui::ecolor::Hsva::from(dim);
        assert!((lifted.h - original.h).abs() < 0.02, "hue is preserved");
        // Already-readable colors pass through untouched.
        assert_eq!(readable(phase::text()), phase::text());
    }

    #[test]
    fn segmented_control_selects_clicked_option() {
        let ctx = Context::default();
        configure_style(&ctx);
        let mut value = 0;
        let mut bounds = Rect::NOTHING;
        let mut render = |events: Vec<egui::Event>, value: &mut i32| {
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(400.0, 200.0))),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        bounds = ui
                            .scope(|ui| {
                                segmented(
                                    ui,
                                    "t",
                                    &[(0, "Crop"), (1, "Fit"), (2, "Stretch")],
                                    value,
                                );
                            })
                            .response
                            .rect;
                    });
                },
            );
            bounds
        };
        let rect = render(vec![], &mut value);
        let pos = Pos2::new(rect.right() - 20.0, rect.center().y);
        render(
            vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: Default::default(),
                },
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: Default::default(),
                },
            ],
            &mut value,
        );
        assert_eq!(value, 2);
    }
}
