//! Kleene's three-valued logic, the verdict selection gives each case.

use std::ops::{BitAnd, BitOr, Not};

/// Kleene's three-valued logic. `Unknown` is what static analysis cannot
/// decide; a case is dropped only when its selection is `False`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Truth {
    False,
    Unknown,
    True,
}

impl Truth {
    pub const fn is_false(self) -> bool {
        matches!(self, Self::False)
    }
}

impl From<bool> for Truth {
    fn from(value: bool) -> Self {
        if value { Self::True } else { Self::False }
    }
}

impl Not for Truth {
    type Output = Self;

    fn not(self) -> Self {
        match self {
            Self::False => Self::True,
            Self::Unknown => Self::Unknown,
            Self::True => Self::False,
        }
    }
}

impl BitAnd for Truth {
    type Output = Self;

    fn bitand(self, other: Self) -> Self {
        self.min(other)
    }
}

impl BitOr for Truth {
    type Output = Self;

    fn bitor(self, other: Self) -> Self {
        self.max(other)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truth_tables() {
        use Truth::{False as F, True as T, Unknown as U};
        assert_eq!([!F, !U, !T], [T, U, F]);
        for (a, b, and, or) in [
            (F, F, F, F),
            (F, U, F, U),
            (F, T, F, T),
            (U, U, U, U),
            (U, T, U, T),
            (T, T, T, T),
        ] {
            assert_eq!((a & b, b & a), (and, and), "{a:?} and {b:?}");
            assert_eq!((a | b, b | a), (or, or), "{a:?} or {b:?}");
        }
        assert_eq!(Truth::from(true), T);
        assert!(Truth::from(false).is_false());
        assert!(!U.is_false());
    }
}
