//! What an index is, and the macro that makes one.

use std::fmt;
use std::hash::Hash;

/// A position in one collection, as a type of its own: what [`IndexVec`]
/// and [`IndexSlice`] are indexed by. Made with [`newtype_index!`].
///
/// [`IndexVec`]: crate::IndexVec
/// [`IndexSlice`]: crate::IndexSlice
/// [`newtype_index!`]: crate::newtype_index
pub trait Idx: Copy + Eq + Ord + Hash + fmt::Debug + 'static {
    /// The index of position `index`. Panics when the type can't hold it.
    fn new(index: usize) -> Self;

    /// The position the index stands for.
    fn index(self) -> usize;
}

/// Declares an index type: a `u32` from 0 to `u32::MAX - 1`, kept one
/// higher in a `NonZeroU32` so that an `Option` of it is no bigger than it
/// is. It is ordered as its position, and its `Debug` is `Name(3)`.
///
/// ```
/// aye_index::newtype_index! {
///     /// A worker's lane.
///     pub struct Lane;
/// }
///
/// let lane = Lane::from_u32(3);
/// assert_eq!(lane.as_usize(), 3);
/// assert_eq!(format!("{lane:?}"), "Lane(3)");
/// assert_eq!(size_of::<Option<Lane>>(), size_of::<Lane>());
/// ```
#[macro_export]
macro_rules! newtype_index {
    ($(#[$attr:meta])* $vis:vis struct $name:ident;) => {
        $(#[$attr])*
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        $vis struct $name(::core::num::NonZeroU32);

        #[allow(dead_code, reason = "an index type may not use every method")]
        impl $name {
            /// The highest index there can be.
            $vis const MAX: Self = Self::from_u32(u32::MAX - 1);

            /// The index of position `index`. Panics from `u32::MAX` on.
            $vis const fn from_u32(index: u32) -> Self {
                assert!(index < u32::MAX, concat!(stringify!($name), " out of range"));
                match ::core::num::NonZeroU32::new(index + 1) {
                    Some(stored) => Self(stored),
                    None => unreachable!(),
                }
            }

            /// The index of position `index`. Panics from `u32::MAX` on.
            $vis fn from_usize(index: usize) -> Self {
                match u32::try_from(index) {
                    Ok(index) => Self::from_u32(index),
                    Err(_) => panic!(concat!(stringify!($name), " out of range: {}"), index),
                }
            }

            /// The position, as a `u32`.
            $vis const fn as_u32(self) -> u32 {
                self.0.get() - 1
            }

            /// The position, as a `usize`.
            $vis const fn as_usize(self) -> usize {
                self.as_u32() as usize
            }
        }

        impl $crate::Idx for $name {
            fn new(index: usize) -> Self {
                Self::from_usize(index)
            }

            fn index(self) -> usize {
                self.as_usize()
            }
        }

        impl ::core::fmt::Debug for $name {
            fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                f.debug_tuple(stringify!($name)).field(&self.as_u32()).finish()
            }
        }

        impl ::core::convert::From<$name> for u32 {
            fn from(index: $name) -> Self {
                index.as_u32()
            }
        }

        impl ::core::convert::From<$name> for usize {
            fn from(index: $name) -> Self {
                index.as_usize()
            }
        }
    };
}

#[cfg(test)]
mod tests {
    use crate::Idx;

    crate::newtype_index! {
        struct Lane;
    }

    #[test]
    fn an_index_is_its_position() {
        for position in [0, 1, 41, u32::MAX - 1] {
            let lane = Lane::from_u32(position);
            assert_eq!(lane.as_u32(), position);
            assert_eq!(u32::from(lane), position);
            assert_eq!(Lane::from_usize(position as usize), lane);
            assert_eq!(Lane::new(lane.index()), lane);
        }
        assert_eq!(Lane::MAX.as_u32(), u32::MAX - 1);
        assert!(Lane::from_u32(1) < Lane::from_u32(2));
        assert_eq!(format!("{:?}", Lane::from_u32(7)), "Lane(7)");
        assert_eq!(size_of::<Option<Lane>>(), size_of::<u32>());
    }

    #[test]
    #[should_panic(expected = "Lane out of range")]
    fn u32_max_is_out_of_range() {
        let _ = Lane::from_u32(u32::MAX);
    }

    #[cfg(target_pointer_width = "64")]
    #[test]
    #[should_panic(expected = "Lane out of range: 4294967296")]
    fn a_usize_past_u32_is_out_of_range() {
        let _ = Lane::from_usize(1 << 32);
    }
}
