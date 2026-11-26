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

    pub fn port(&self, port: usize) -> [u16; 3] {
        port.checked_sub(1)
            .and_then(|index| self.values.get(index))
            .copied()
            .unwrap_or([0; 3])
    }

    pub fn clear(&mut self) {
        self.values = [[0; 3]; PORT_COUNT];
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LedLine {
    Color {
        port: usize,
        channel: Channel,
        value: u16,
    },
    Mode(u8),
}

pub fn parse_line(line: &str) -> Option<LedLine> {
    let line = line.trim_end_matches('\r');
    let fields: Vec<&str> = line.split_whitespace().collect();
    if fields.len() == 2 && fields[0] == "mode" {
        let value = fields[1].parse::<u8>().ok()?;
        return (value <= 1).then_some(LedLine::Mode(value));
    }
    if fields.len() != 3 {
        return None;
    }
    let port = fields[0].parse::<usize>().ok()?;
    if !(1..=PORT_COUNT).contains(&port) {
        return None;
    }
    let channel = Channel::from_token(fields[1])?;
    let value = fields[2].parse::<u16>().ok()?;
    Some(LedLine::Color {
        port,
        channel,
        value,
    })
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
        assert_eq!(leds.port(1), [65535, 0, 0]);
        assert_eq!(leds.port(48), [0, 0, 514]);
    }

    #[test]
    fn ignores_ports_outside_one_to_forty_eight() {
        let mut leds = Leds::new();
        leds.set(0, Channel::Red, 65535);
        leds.set(49, Channel::Red, 65535);
        assert_eq!(leds.port(1), [0, 0, 0]);
    }

    #[test]
    fn maps_levels_to_display_values() {
        assert_eq!(to_u8(65535), 255);
        assert_eq!(to_u8(10280), 40);
        assert_eq!(to_u8(0), 0);
    }

    #[test]
    fn parses_color_writes() {
        assert_eq!(
            parse_line("7 g 65535"),
            Some(LedLine::Color {
                port: 7,
                channel: Channel::Green,
                value: 65535,
            })
        );
        assert_eq!(
            parse_line("48 b 0\r"),
            Some(LedLine::Color {
                port: 48,
                channel: Channel::Blue,
                value: 0,
            })
        );
        assert_eq!(
            parse_line("  1   r   257  "),
            Some(LedLine::Color {
                port: 1,
                channel: Channel::Red,
                value: 257,
            })
        );
    }

    #[test]
    fn parses_mode_writes() {
        assert_eq!(parse_line("mode 0"), Some(LedLine::Mode(0)));
        assert_eq!(parse_line("mode 1"), Some(LedLine::Mode(1)));
        assert_eq!(parse_line("mode 2"), None);
    }

    #[test]
    fn ignores_non_protocol_lines() {
        for line in [
            "",
            "kitt: 24 columns / 48 LEDs, 22 ms/step",
            "kitt: stopped, LEDs back under firmware control.",
            "0 r 0",
            "49 r 0",
            "1 x 0",
            "1 r 65536",
            "1 r",
            "1 r 0 extra",
            "mode",
        ] {
            assert_eq!(parse_line(line), None, "{line:?}");
        }
    }
}
