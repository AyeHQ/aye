//! An owned run of values indexed by one index type.

use std::fmt;
use std::marker::PhantomData;
use std::ops::{Index, IndexMut};

use crate::{Idx, IndexSlice};

/// A `Vec` indexed by `I`: each value's index is the position it was
/// pushed at, and [`IndexVec::push`] returns it.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct IndexVec<I, T> {
    raw: Vec<T>,
    index: PhantomData<fn(&I)>,
}

impl<I: Idx, T> IndexVec<I, T> {
    pub const fn new() -> Self {
        Self::from_raw(Vec::new())
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self::from_raw(Vec::with_capacity(capacity))
    }

    /// `raw`, indexed by `I` from its first value.
    pub const fn from_raw(raw: Vec<T>) -> Self {
        Self {
            raw,
            index: PhantomData,
        }
    }

    /// The values, as a plain `Vec`.
    pub fn into_raw(self) -> Vec<T> {
        self.raw
    }

    /// The values, as a slice indexed by `I`.
    pub fn as_slice(&self) -> IndexSlice<'_, I, T> {
        IndexSlice::from_raw(&self.raw)
    }

    /// Adds `value` at the end, and returns its index.
    pub fn push(&mut self, value: T) -> I {
        let index = self.next_index();
        self.raw.push(value);
        index
    }

    /// The index the next value pushed will have.
    pub fn next_index(&self) -> I {
        I::new(self.raw.len())
    }

    pub const fn len(&self) -> usize {
        self.raw.len()
    }

    pub const fn is_empty(&self) -> bool {
        self.raw.is_empty()
    }

    pub fn get(&self, index: I) -> Option<&T> {
        self.as_slice().get(index)
    }

    pub fn get_mut(&mut self, index: I) -> Option<&mut T> {
        self.raw.get_mut(index.index())
    }

    /// The index of the last value; `None` when there is none.
    pub fn last_index(&self) -> Option<I> {
        self.as_slice().last_index()
    }

    pub fn iter(&self) -> std::slice::Iter<'_, T> {
        self.raw.iter()
    }

    pub fn iter_mut(&mut self) -> std::slice::IterMut<'_, T> {
        self.raw.iter_mut()
    }

    /// Each value with its index, in order.
    pub fn iter_enumerated(&self) -> impl DoubleEndedIterator<Item = (I, &T)> + ExactSizeIterator {
        self.as_slice().iter_enumerated()
    }

    /// Every index, in order.
    pub fn indices(&self) -> impl DoubleEndedIterator<Item = I> + ExactSizeIterator + use<I, T> {
        (0..self.raw.len()).map(I::new)
    }
}

impl<I: Idx, T> Default for IndexVec<I, T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<I: Idx, T> Index<I> for IndexVec<I, T> {
    type Output = T;

    fn index(&self, index: I) -> &T {
        &self.raw[index.index()]
    }
}

impl<I: Idx, T> IndexMut<I> for IndexVec<I, T> {
    fn index_mut(&mut self, index: I) -> &mut T {
        &mut self.raw[index.index()]
    }
}

impl<I: Idx, T> FromIterator<T> for IndexVec<I, T> {
    fn from_iter<V: IntoIterator<Item = T>>(values: V) -> Self {
        Self::from_raw(values.into_iter().collect())
    }
}

impl<I: Idx, T> Extend<T> for IndexVec<I, T> {
    fn extend<V: IntoIterator<Item = T>>(&mut self, values: V) {
        self.raw.extend(values);
    }
}

impl<I: Idx, T> IntoIterator for IndexVec<I, T> {
    type Item = T;
    type IntoIter = std::vec::IntoIter<T>;

    fn into_iter(self) -> Self::IntoIter {
        self.raw.into_iter()
    }
}

impl<'a, I: Idx, T> IntoIterator for &'a IndexVec<I, T> {
    type Item = &'a T;
    type IntoIter = std::slice::Iter<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.raw.iter()
    }
}

impl<'a, I: Idx, T> IntoIterator for &'a mut IndexVec<I, T> {
    type Item = &'a mut T;
    type IntoIter = std::slice::IterMut<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.raw.iter_mut()
    }
}

impl<I, T: fmt::Debug> fmt::Debug for IndexVec<I, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.raw, f)
    }
}

#[cfg(test)]
mod tests {
    use crate::IndexVec;

    crate::newtype_index! {
        struct File;
    }

    #[test]
    fn push_returns_the_index_of_what_it_pushed() {
        let mut files = IndexVec::<File, &str>::new();
        assert_eq!(files.next_index(), File::from_u32(0));
        let a = files.push("a.py");
        let b = files.push("b.py");
        assert_eq!((a, b), (File::from_u32(0), File::from_u32(1)));
        assert_eq!(files[b], "b.py");
        files[a] = "c.py";
        assert_eq!(files.get(a), Some(&"c.py"));
        assert_eq!(files.get(File::from_u32(2)), None);
        assert_eq!(files.last_index(), Some(b));
        assert_eq!(files.indices().collect::<Vec<_>>(), [a, b]);
        assert_eq!(
            files
                .iter_enumerated()
                .map(|(i, f)| (i, *f))
                .collect::<Vec<_>>(),
            [(a, "c.py"), (b, "b.py")]
        );
        assert_eq!(files.as_slice()[a], "c.py");
        assert_eq!(format!("{files:?}"), r#"["c.py", "b.py"]"#);
        assert_eq!(files.clone().into_raw(), ["c.py", "b.py"]);
    }

    #[test]
    fn collected_and_extended_in_order() {
        let mut files: IndexVec<File, u8> = (0..3).collect();
        files.extend([3, 4]);
        assert_eq!(files.len(), 5);
        assert_eq!(files[File::from_u32(4)], 4);
        for value in &mut files {
            *value *= 2;
        }
        assert_eq!(files.into_iter().collect::<Vec<_>>(), [0, 2, 4, 6, 8]);
        assert!(IndexVec::<File, u8>::default().is_empty());
    }
}
