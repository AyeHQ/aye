//! How much the text and JSON renderers show ([`Level`]), and how much of
//! a bad outcome each shows at each level ([`DetailLevel`], [`Ladder`]).

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

/// How much of a bad outcome is shown, the same in both formats: L1, its
/// line (key, id, outcome, where, and why in one line); L2, its detail
/// (the pointed code, the comparison or the message, the hint, the setup
/// chain, history, the rerun); L3, everything (every frame, the captured
/// output, the locals).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DetailLevel {
    L1,
    L2,
    L3,
}

/// What a format shows of a bad outcome at each [`Level`], from `-q`:
/// `docs/architecture.md`'s ladder ("Output and history"), a table per
/// format. At a level with no [`DetailLevel`], bad outcomes are counted
/// (in JSON lines, named by key in the summary), not shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ladder([Option<DetailLevel>; 5]);

impl Ladder {
    /// `--output text`: L1 by default, L2 at `-v`, L3 from `-vv`.
    pub const TEXT: Self = Self([
        None,
        Some(DetailLevel::L1),
        Some(DetailLevel::L2),
        Some(DetailLevel::L3),
        Some(DetailLevel::L3),
    ]);

    /// `--output json`: L1 at `-v`, L2 at `-vv`, L3 at `-vvv`.
    pub const JSON: Self = Self([
        None,
        None,
        Some(DetailLevel::L1),
        Some(DetailLevel::L2),
        Some(DetailLevel::L3),
    ]);

    /// How much of a bad outcome `level` shows.
    pub const fn detail(self, level: Level) -> Option<DetailLevel> {
        let rung = match level {
            Level::Quiet => 0,
            Level::Default => 1,
            Level::Verbose => 2,
            Level::VeryVerbose => 3,
            Level::Engine => 4,
        };
        self.0[rung]
    }

    /// The first level that shows a bad outcome at `detail` or more: `aye
    /// show`'s details, without `-v` or `-q`, are at L2's (text's `-v`,
    /// JSON's `-vv`).
    pub fn first_at(self, detail: DetailLevel) -> Level {
        Level::ALL
            .into_iter()
            .find(|level| self.detail(*level) >= Some(detail))
            .unwrap_or(Level::Engine)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `docs/architecture.md`'s ladder: each row's detail in each format,
    /// a row that names none holding the one before it (each level adds
    /// to the one below).
    fn documented() -> [[Option<DetailLevel>; 5]; 2] {
        let doc = include_str!("../../../docs/architecture.md");
        let table = &doc[doc
            .find("| verbosity | `--output text` | `--output json` |")
            .unwrap()..];
        let rows: Vec<&str> = table
            .lines()
            .skip(2)
            .take_while(|line| line.starts_with('|'))
            .collect();
        assert_eq!(rows.len(), 5, "{rows:?}");
        let mut ladders = [[None; 5]; 2];
        for (rung, row) in rows.iter().enumerate() {
            let cells: Vec<&str> = row.split('|').collect();
            for (format, cell) in cells[2..4].iter().enumerate() {
                let named = [
                    ("at L1", DetailLevel::L1),
                    ("at L2", DetailLevel::L2),
                    ("at L3", DetailLevel::L3),
                ]
                .into_iter()
                .find_map(|(words, detail)| cell.contains(words).then_some(detail));
                let before = rung.checked_sub(1).and_then(|at| ladders[format][at]);
                ladders[format][rung] = named.or(before);
            }
        }
        ladders
    }

    #[test]
    fn each_format_shows_what_the_ladder_documents() {
        let [text, json] = documented();
        assert_eq!(Level::ALL.map(|level| Ladder::TEXT.detail(level)), text);
        assert_eq!(Level::ALL.map(|level| Ladder::JSON.detail(level)), json);
        assert_eq!(Ladder::TEXT.first_at(DetailLevel::L2), Level::Verbose);
        assert_eq!(Ladder::JSON.first_at(DetailLevel::L2), Level::VeryVerbose);
        assert_eq!(Ladder::JSON.first_at(DetailLevel::L1), Level::Verbose);
    }

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
