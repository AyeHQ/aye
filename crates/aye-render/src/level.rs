//! How much the text and JSON renderers show ([`Level`]).

/// How much [`Text`](crate::Text) and [`JsonLines`](crate::JsonLines) show:
/// the number of `-v` less the number of `-q`, clamped once, to -1..=3. Each
/// renderer's docs say what it shows at each level.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(i8)]
pub enum Level {
    /// `-q`: the summary.
    Quiet = -1,
    /// Neither `-v` nor `-q`.
    #[default]
    Default = 0,
    /// `-v`.
    Verbose = 1,
    /// `-vv`.
    VeryVerbose = 2,
    /// `-vvv`: everything, what the engine did with it.
    Engine = 3,
}

impl Level {
    /// Every level, from the least shown.
    pub const ALL: [Self; 5] = [
        Self::Quiet,
        Self::Default,
        Self::Verbose,
        Self::VeryVerbose,
        Self::Engine,
    ];

    /// The level `count` asks for, the number of `-v` less the number of
    /// `-q`: below -1 is [`Level::Quiet`], above 3 [`Level::Engine`].
    pub const fn from_count(count: i16) -> Self {
        match count {
            ..=-1 => Self::Quiet,
            0 => Self::Default,
            1 => Self::Verbose,
            2 => Self::VeryVerbose,
            _ => Self::Engine,
        }
    }

    /// The count, from -1 to 3.
    pub const fn count(self) -> i8 {
        self as i8
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_count_is_clamped_to_the_ladder() {
        for (count, level) in [
            (i16::MIN, Level::Quiet),
            (-2, Level::Quiet),
            (-1, Level::Quiet),
            (0, Level::Default),
            (1, Level::Verbose),
            (2, Level::VeryVerbose),
            (3, Level::Engine),
            (4, Level::Engine),
            (i16::MAX, Level::Engine),
        ] {
            assert_eq!(Level::from_count(count), level, "{count}");
        }
        assert_eq!(Level::ALL.map(Level::count), [-1, 0, 1, 2, 3]);
        assert_eq!(Level::default(), Level::Default);
    }
}
