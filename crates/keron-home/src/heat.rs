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
        let _ = level;
        todo!("keron-home: Heat::from_level")
    }

    pub fn level(self) -> u8 {
        todo!("keron-home: Heat::level")
    }

    /// "off", "quiet", "warm", "hot", "burning".
    pub fn word(self) -> &'static str {
        todo!("keron-home: Heat::word")
    }

    /// How many of the four bars are lit (0 to 4).
    pub fn bars(self) -> u8 {
        todo!("keron-home: Heat::bars")
    }

    /// Only burning pulses.
    pub fn pulses(self) -> bool {
        todo!("keron-home: Heat::pulses")
    }
}
