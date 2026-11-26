pub const PORT_COUNT: usize = 48;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Channel {
    Red,
    Green,
    Blue,
}

impl Channel {
    pub fn index(self) -> usize {
        match self {
            Self::Red => 0,
            Self::Green => 1,
            Self::Blue => 2,
        }
    }

    pub fn token(self) -> &'static str {
        match self {
            Self::Red => "r",
            Self::Green => "g",
            Self::Blue => "b",
        }
    }

    pub fn from_token(token: &str) -> Option<Self> {
        match token {
            "r" => Some(Self::Red),
            "g" => Some(Self::Green),
            "b" => Some(Self::Blue),
            _ => None,
        }
    }
}

#[derive(Clone)]
pub struct Leds {
    values: [[u16; 3]; PORT_COUNT],
}

impl Default for Leds {
    fn default() -> Self {
        Self {
            values: [[0; 3]; PORT_COUNT],
        }
    }
}

impl Leds {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(&mut self, port: usize, channel: Channel, value: u16) {
        if let Some(slot) = self.values.get_mut(port.wrapping_sub(1)) {
            slot[channel.index()] = value;
        }
    }

    pub fn get(&self, port: usize, channel: Channel) -> u16 {
        port.checked_sub(1)
            .and_then(|index| self.values.get(index))
            .map(|slot| slot[channel.index()])
            .unwrap_or(0)
    }

    pub fn port(&self, port: usize) -> [u16; 3] {
        port.checked_sub(1)
            .and_then(|index| self.values.get(index))
            .copied()
            .unwrap_or([0; 3])
    }

    pub fn clear(&mut self) {
        self.values = [[0; 3]; PORT_COUNT];
    }

    pub fn is_dark(&self) -> bool {
        self.values.iter().all(|slot| slot == &[0; 3])
    }
}

pub fn to_u8(value: u16) -> u8 {
    (value as u32 * 255 / 65535) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_and_get_ports() {
        let mut leds = Leds::new();
        leds.set(1, Channel::Red, 65535);
        leds.set(48, Channel::Blue, 514);
        assert_eq!(leds.get(1, Channel::Red), 65535);
        assert_eq!(leds.get(48, Channel::Blue), 514);
        assert_eq!(leds.port(1), [65535, 0, 0]);
        assert_eq!(leds.port(48), [0, 0, 514]);
        assert!(!leds.is_dark());
    }

    #[test]
    fn ignores_ports_outside_one_to_forty_eight() {
        let mut leds = Leds::new();
        leds.set(0, Channel::Red, 65535);
        leds.set(49, Channel::Red, 65535);
        assert!(leds.is_dark());
    }

    #[test]
    fn maps_levels_to_display_values() {
        assert_eq!(to_u8(65535), 255);
        assert_eq!(to_u8(10280), 40);
        assert_eq!(to_u8(0), 0);
    }
}
