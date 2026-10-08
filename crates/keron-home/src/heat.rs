//! Loose-ends heat: 0 is off (done, dismissed, expired), 1 to 4 are quiet,
//! warm, hot and burning, drawn as one to four rising bars in one hue.
//! Burning pulses slowly (not under Reduce Motion).

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Heat {
    Off,
    Quiet,
    Warm,
    Hot,
    Burning,
}

impl Heat {
    /// 0..=4; anything above 4 is burning.
    pub fn from_level(level: u8) -> Self {
        match level {
            0 => Heat::Off,
            1 => Heat::Quiet,
            2 => Heat::Warm,
            3 => Heat::Hot,
            _ => Heat::Burning,
        }
    }

    pub fn level(self) -> u8 {
        match self {
            Heat::Off => 0,
            Heat::Quiet => 1,
            Heat::Warm => 2,
            Heat::Hot => 3,
            Heat::Burning => 4,
        }
    }

    /// "off", "quiet", "warm", "hot", "burning".
    pub fn word(self) -> &'static str {
        match self {
            Heat::Off => "off",
            Heat::Quiet => "quiet",
            Heat::Warm => "warm",
            Heat::Hot => "hot",
            Heat::Burning => "burning",
        }
    }

    /// How many of the four bars are lit (0 to 4).
    pub fn bars(self) -> u8 {
        self.level()
    }

    /// Only burning pulses.
    pub fn pulses(self) -> bool {
        self == Heat::Burning
    }
}
