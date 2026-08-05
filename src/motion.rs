#[derive(Clone, Copy, Debug)]
pub struct Spring {
    pub stiffness: f32,
    pub damping: f32,
    pub epsilon: f32,
}

impl Spring {
    pub const fn expressive() -> Self {
        Self {
            stiffness: 420.0,
            damping: 34.0,
            epsilon: 0.001,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct SpringValue {
    value: f32,
    velocity: f32,
}

impl SpringValue {
    pub const fn new(value: f32) -> Self {
        Self {
            value,
            velocity: 0.0,
        }
    }

    pub fn value(self) -> f32 {
        self.value
    }

    pub fn step(&mut self, target: f32, dt: f32, spring: Spring) -> bool {
        let dt = dt.clamp(0.0, 1.0 / 30.0);
        let displacement = self.value - target;
        let force = -spring.stiffness * displacement - spring.damping * self.velocity;
        self.velocity += force * dt;
        self.value += self.velocity * dt;

        let settled =
            (self.value - target).abs() <= spring.epsilon && self.velocity.abs() <= spring.epsilon;
        if settled {
            self.value = target;
            self.velocity = 0.0;
        }
        !settled
    }
}

#[derive(Clone, Copy, Debug)]
pub struct PagerMotion<T>
where
    T: Copy + PartialEq,
{
    target: T,
    previous: T,
    progress: f32,
    direction: f32,
}

impl<T> PagerMotion<T>
where
    T: Copy + PartialEq,
{
    pub const fn new(initial: T) -> Self {
        Self {
            target: initial,
            previous: initial,
            progress: 1.0,
            direction: 1.0,
        }
    }

    pub fn set_target(&mut self, target: T, direction: f32) {
        if self.target == target {
            return;
        }
        self.previous = self.target;
        self.target = target;
        self.progress = 0.0;
        self.direction = if direction < 0.0 { -1.0 } else { 1.0 };
    }

    pub fn step(&mut self, dt: f32, duration: f32) -> PagerFrame {
        if self.progress < 1.0 {
            self.progress = (self.progress + dt / duration.max(0.001)).min(1.0);
        }
        let eased = ease_out_cubic(self.progress);
        PagerFrame {
            opacity: eased,
            offset: self.direction * (1.0 - eased) * 18.0,
            running: self.progress < 1.0,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct PagerFrame {
    pub opacity: f32,
    pub offset: f32,
    pub running: bool,
}

pub fn ease_out_cubic(t: f32) -> f32 {
    1.0 - (1.0 - t.clamp(0.0, 1.0)).powi(3)
}

use eframe::egui::{Color32, Painter, Pos2, Rect, Shape, Stroke, Vec2};
use std::time::{Duration, Instant};

pub fn ease_in_out_cubic(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    if t < 0.5 {
        4.0 * t * t * t
    } else {
        1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
    }
}

/// Out-back: overshoots the target slightly, then settles. Used for pop
/// entrances (toasts, hover halos) where a little bounce reads as playful.
pub fn ease_out_overshoot(t: f32) -> f32 {
    const C: f32 = 1.70158;
    let t = t.clamp(0.0, 1.0) - 1.0;
    1.0 + (C + 1.0) * t * t * t + C * t * t
}

pub fn ease_out_elastic(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    if t <= 0.0 || t >= 1.0 {
        return t;
    }
    let c4 = (2.0 * std::f32::consts::PI) / 3.0;
    2f32.powf(-10.0 * t) * ((t * 10.0 - 0.75) * c4).sin() + 1.0
}

/// 0..1 sine pulse at `period` seconds. Feeds glows, status dots, breathing.
pub fn pulse01(time: f32, period: f32) -> f32 {
    0.5 - 0.5 * (time * std::f32::consts::TAU / period.max(0.001)).cos()
}

/// Linear 0..1 loop phase for shine sweeps and shimmer bands.
pub fn shine_phase(time: f32, period: f32) -> f32 {
    (time / period.max(0.001)).fract()
}

const CONFETTI_LIFETIME: f32 = 2.9;
const CONFETTI_GRAVITY: f32 = 620.0;

#[derive(Clone, Copy, Debug)]
struct ConfettiParticle {
    pos: Pos2,
    vel: Vec2,
    color: Color32,
    size: f32,
    rotation: f32,
    spin: f32,
    flutter_phase: f32,
    flutter_speed: f32,
    circle: bool,
    lifetime: f32,
    age: f32,
}

/// Deterministic one-shot confetti burst. Seeded with an xorshift64 so the
/// celebration is reproducible and needs no `rand` dependency.
#[derive(Clone, Debug)]
pub struct Confetti {
    particles: Vec<ConfettiParticle>,
}

impl Confetti {
    pub fn burst(origin: Pos2, count: usize, seed: u64, palette: &[Color32]) -> Self {
        let mut state = seed.max(1);
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let unit = |bits: u64| (bits % 10_000) as f32 / 10_000.0;
        let particles = (0..count)
            .map(|_| {
                let angle = -std::f32::consts::FRAC_PI_2
                    + (unit(next()) - 0.5) * std::f32::consts::PI * 0.9;
                let speed = 240.0 + unit(next()) * 560.0;
                ConfettiParticle {
                    pos: origin,
                    vel: Vec2::new(angle.cos() * speed, angle.sin() * speed),
                    color: palette[(next() % palette.len().max(1) as u64) as usize],
                    size: 3.0 + unit(next()) * 5.0,
                    rotation: unit(next()) * std::f32::consts::TAU,
                    spin: (unit(next()) - 0.5) * 12.0,
                    flutter_phase: unit(next()) * std::f32::consts::TAU,
                    flutter_speed: 4.0 + unit(next()) * 7.0,
                    circle: next() % 3 == 0,
                    lifetime: CONFETTI_LIFETIME * (0.72 + unit(next()) * 0.28),
                    age: 0.0,
                }
            })
            .collect();
        Self { particles }
    }

    pub fn step(&mut self, dt: f32) {
        for particle in &mut self.particles {
            particle.age += dt;
            particle.vel.y += CONFETTI_GRAVITY * dt;
            // Air drag + sideways flutter so pieces tumble instead of falling
            // in straight lines.
            particle.vel.x *= 1.0 - (0.6 * dt).min(0.5);
            particle.pos.x += (particle.vel.x
                + (particle.age * particle.flutter_speed + particle.flutter_phase).sin() * 42.0)
                * dt;
            particle.pos.y += particle.vel.y * dt;
            particle.rotation += particle.spin * dt;
        }
    }

    pub fn is_alive(&self) -> bool {
        self.particles
            .iter()
            .any(|particle| particle.age < particle.lifetime)
    }

    pub fn paint(&self, painter: &Painter) {
        for particle in &self.particles {
            if particle.age >= particle.lifetime {
                continue;
            }
            let fade = 1.0 - (particle.age / particle.lifetime).powi(2);
            let color = Color32::from_rgba_unmultiplied(
                particle.color.r(),
                particle.color.g(),
                particle.color.b(),
                (fade * 255.0) as u8,
            );
            if particle.circle {
                painter.circle_filled(particle.pos, particle.size * 0.5, color);
            } else {
                let half = particle.size * 0.5;
                let (sin, cos) = particle.rotation.sin_cos();
                let corner = |dx: f32, dy: f32| {
                    Pos2::new(
                        particle.pos.x + dx * cos - dy * sin,
                        particle.pos.y + dx * sin + dy * cos,
                    )
                };
                painter.add(Shape::convex_polygon(
                    vec![
                        corner(-half, -half * 0.6),
                        corner(half, -half * 0.6),
                        corner(half, half * 0.6),
                        corner(-half, half * 0.6),
                    ],
                    color,
                    Stroke::NONE,
                ));
            }
        }
    }
}

const RIPPLE_DURATION: Duration = Duration::from_millis(460);
const RIPPLE_CAP: usize = 4;

/// Material-style expanding ink rings, clipped to the widget that was
/// clicked. Caller keeps one per widget id and pushes the click position.
#[derive(Clone, Debug, Default)]
pub struct Ripples {
    live: Vec<(Pos2, Instant)>,
}

impl Ripples {
    pub fn push(&mut self, pos: Pos2) {
        if self.live.len() >= RIPPLE_CAP {
            self.live.remove(0);
        }
        self.live.push((pos, Instant::now()));
    }

    pub fn is_active(&self) -> bool {
        self.live
            .iter()
            .any(|(_, started)| started.elapsed() < RIPPLE_DURATION)
    }

    /// Paint expanding rings inside `clip` and drop finished ones. Returns
    /// whether any ring is still animating so the caller can keep repainting.
    pub fn paint(&mut self, painter: &Painter, clip: Rect, color: Color32) -> bool {
        self.live
            .retain(|(_, started)| started.elapsed() < RIPPLE_DURATION);
        if self.live.is_empty() {
            return false;
        }
        // Radius needs to reach the farthest corner from the click point.
        let reach = |pos: Pos2| {
            let dx = (pos.x - clip.left())
                .abs()
                .max((pos.x - clip.right()).abs());
            let dy = (pos.y - clip.top())
                .abs()
                .max((pos.y - clip.bottom()).abs());
            (dx * dx + dy * dy).sqrt()
        };
        let clipped = painter.with_clip_rect(clip);
        for (pos, started) in &self.live {
            let t =
                (started.elapsed().as_secs_f32() / RIPPLE_DURATION.as_secs_f32()).clamp(0.0, 1.0);
            let eased = ease_out_cubic(t);
            clipped.circle_filled(
                *pos,
                reach(*pos) * eased,
                Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 26)
                    .linear_multiply(1.0 - t),
            );
            clipped.circle_stroke(
                *pos,
                reach(*pos) * eased,
                Stroke::new(
                    1.5,
                    Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 60)
                        .linear_multiply(1.0 - t),
                ),
            );
        }
        true
    }
}
