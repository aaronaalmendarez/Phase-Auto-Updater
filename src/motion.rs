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
