//! A borrowed run of values indexed by one index type.

use std::fmt;
use std::marker::PhantomData;
use std::ops::Index;

use crate::Idx;

/// A slice indexed by `I`: what an [`IndexVec`] lends. Without `unsafe` a
/// slice can't be wrapped in place, so this is a view holding the slice,
/// `Copy` like a shared reference, and its getters return references that
/// live as long as the slice rather than the view.
///
/// [`IndexVec`]: crate::IndexVec
pub struct IndexSlice<'a, I, T> {
    raw: &'a [T],
    index: PhantomData<fn(&I)>,
}

impl<'a, I: Idx, T> IndexSlice<'a, I, T> {
    /// `raw`, indexed by `I` from its first value.
    pub const fn from_raw(raw: &'a [T]) -> Self {
        Self {
            raw,
            index: PhantomData,
        }
    }

    /// The values, as a plain slice.
    pub const fn as_raw(self) -> &'a [T] {
        self.raw
    }

    pub const fn len(self) -> usize {
        self.raw.len()
    }

    pub const fn is_empty(self) -> bool {
        self.raw.is_empty()
    }

    pub fn get(self, index: I) -> Option<&'a T> {
        self.raw.get(index.index())
    }

    pub const fn first(self) -> Option<&'a T> {
        self.raw.first()
    }

    pub const fn last(self) -> Option<&'a T> {
        self.raw.last()
    }

    /// The index of the last value; `None` when there is none.
    pub fn last_index(self) -> Option<I> {
        self.raw.len().checked_sub(1).map(I::new)
    }

    pub fn iter(self) -> std::slice::Iter<'a, T> {
        self.raw.iter()
    }

    /// Each value with its index, in order.
    pub fn iter_enumerated(
        self,
    ) -> impl DoubleEndedIterator<Item = (I, &'a T)> + ExactSizeIterator {
        self.raw
            .iter()
            .enumerate()
            .map(|(index, value)| (I::new(index), value))
    }

    /// Every index, in order.
    pub fn indices(self) -> impl DoubleEndedIterator<Item = I> + ExactSizeIterator {
        (0..self.raw.len()).map(I::new)
    }
}

impl<I, T> Clone for IndexSlice<'_, I, T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<I, T> Copy for IndexSlice<'_, I, T> {}

impl<I: Idx, T> Index<I> for IndexSlice<'_, I, T> {
    type Output = T;

    fn index(&self, index: I) -> &T {
        &self.raw[index.index()]
    }
}

impl<'a, I: Idx, T> IntoIterator for IndexSlice<'a, I, T> {
    type Item = &'a T;
    type IntoIter = std::slice::Iter<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.raw.iter()
    }
}

impl<I, T: PartialEq> PartialEq for IndexSlice<'_, I, T> {
    fn eq(&self, other: &Self) -> bool {
        self.raw == other.raw
    }
}

impl<I, T: Eq> Eq for IndexSlice<'_, I, T> {}

impl<I, T: fmt::Debug> fmt::Debug for IndexSlice<'_, I, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.raw, f)
    }
}

#[cfg(test)]
mod tests {
    use crate::IndexSlice;

    crate::newtype_index! {
        struct Row;
    }

    #[test]
    fn a_slice_is_indexed_by_its_own_type() {
        let raw = ["a", "b", "c"];
        let rows = IndexSlice::<Row, _>::from_raw(&raw);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[Row::from_u32(1)], "b");
        assert_eq!(rows.get(Row::from_u32(3)), None);
        assert_eq!(rows.last_index(), Some(Row::from_u32(2)));
        assert_eq!(
            rows.iter_enumerated().collect::<Vec<_>>(),
            [
                (Row::from_u32(0), &"a"),
                (Row::from_u32(1), &"b"),
                (Row::from_u32(2), &"c")
            ]
        );
        assert_eq!(rows.indices().next_back(), Some(Row::from_u32(2)));
        assert_eq!(format!("{rows:?}"), r#"["a", "b", "c"]"#);

        let none = IndexSlice::<Row, &str>::from_raw(&[]);
        assert!(none.is_empty());
        assert_eq!(none.last_index(), None);
        assert_eq!(none.first(), None);
    }
}
