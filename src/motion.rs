//! Damped springs for UI motion.

#[derive(Clone, Copy, Debug)]
pub struct Spring {
    pub stiffness: f32,
    pub damping: f32,
    /// Distance and speed below which the spring snaps to rest.
    pub epsilon: f32,
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

    /// Advance toward `target`. Returns true while still moving.
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
