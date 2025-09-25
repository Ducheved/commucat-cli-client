use std::time::Duration;

#[derive(Clone)]
pub struct AnimationFrame {
    pub duration: Duration,
    pub content: String,
}

#[derive(Clone)]
pub struct Animation {
    frames: Vec<AnimationFrame>,
    current: usize,
    elapsed: Duration,
}

impl Animation {
    pub fn new(frames: Vec<AnimationFrame>) -> Self {
        assert!(!frames.is_empty(), "animation requires at least one frame");
        Animation {
            frames,
            current: 0,
            elapsed: Duration::ZERO,
        }
    }

    pub fn tick(&mut self, delta: Duration) -> &str {
        if !delta.is_zero() {
            self.elapsed += delta;
            while self.elapsed >= self.frames[self.current].duration {
                self.elapsed -= self.frames[self.current].duration;
                self.current = (self.current + 1) % self.frames.len();
            }
        }
        &self.frames[self.current].content
    }

    pub fn reset(&mut self) {
        self.current = 0;
        self.elapsed = Duration::ZERO;
    }
}

pub fn create_loading_animation() -> Animation {
    Animation::new(vec![
        AnimationFrame {
            duration: Duration::from_millis(100),
            content: "⠋".to_string(),
        },
        AnimationFrame {
            duration: Duration::from_millis(100),
            content: "⠙".to_string(),
        },
        AnimationFrame {
            duration: Duration::from_millis(100),
            content: "⠹".to_string(),
        },
        AnimationFrame {
            duration: Duration::from_millis(100),
            content: "⠸".to_string(),
        },
        AnimationFrame {
            duration: Duration::from_millis(100),
            content: "⠼".to_string(),
        },
        AnimationFrame {
            duration: Duration::from_millis(100),
            content: "⠴".to_string(),
        },
        AnimationFrame {
            duration: Duration::from_millis(100),
            content: "⠦".to_string(),
        },
        AnimationFrame {
            duration: Duration::from_millis(100),
            content: "⠧".to_string(),
        },
        AnimationFrame {
            duration: Duration::from_millis(100),
            content: "⠇".to_string(),
        },
        AnimationFrame {
            duration: Duration::from_millis(100),
            content: "⠏".to_string(),
        },
    ])
}

pub fn create_pulse_animation() -> Animation {
    Animation::new(vec![
        AnimationFrame {
            duration: Duration::from_millis(200),
            content: "●".to_string(),
        },
        AnimationFrame {
            duration: Duration::from_millis(200),
            content: "◐".to_string(),
        },
        AnimationFrame {
            duration: Duration::from_millis(200),
            content: "◓".to_string(),
        },
        AnimationFrame {
            duration: Duration::from_millis(200),
            content: "◑".to_string(),
        },
        AnimationFrame {
            duration: Duration::from_millis(200),
            content: "◒".to_string(),
        },
    ])
}

pub fn create_wave_animation() -> Animation {
    Animation::new(vec![
        AnimationFrame {
            duration: Duration::from_millis(150),
            content: "≋".to_string(),
        },
        AnimationFrame {
            duration: Duration::from_millis(150),
            content: "≈".to_string(),
        },
        AnimationFrame {
            duration: Duration::from_millis(150),
            content: "≋".to_string(),
        },
        AnimationFrame {
            duration: Duration::from_millis(150),
            content: "~".to_string(),
        },
    ])
}

pub fn create_neko_walk() -> Animation {
    Animation::new(vec![
        AnimationFrame {
            duration: Duration::from_millis(300),
            content: "(=^･ω･^=)".to_string(),
        },
        AnimationFrame {
            duration: Duration::from_millis(300),
            content: "(=^･ェ･^=)".to_string(),
        },
        AnimationFrame {
            duration: Duration::from_millis(300),
            content: "(=^･ω･^=)".to_string(),
        },
        AnimationFrame {
            duration: Duration::from_millis(300),
            content: "(=^-ω-^=)".to_string(),
        },
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reset_returns_to_first_frame() {
        let mut animation = create_loading_animation();
        let first_frame = animation.tick(Duration::ZERO).to_string();
        animation.tick(Duration::from_secs(1));
        animation.reset();
        assert_eq!(animation.tick(Duration::ZERO), first_frame);
    }

    #[test]
    fn wave_animation_cycles_through_variants() {
        let mut animation = create_wave_animation();
        let first = animation.tick(Duration::from_millis(1)).to_string();
        let second = animation.tick(Duration::from_millis(200)).to_string();
        let third = animation.tick(Duration::from_millis(200)).to_string();
        assert!(first != second || second != third);
    }
}
