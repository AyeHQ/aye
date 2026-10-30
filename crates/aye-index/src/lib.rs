//! Index newtypes and the collections they index.
//!
//! A position held as a bare `usize` (collection's files and recognizers,
//! `aye show`'s records) or a `u32` alias (0.0.1's `WorkerId` and
//! `BatchId`) says nothing about which collection it is a position in, and
//! mixes with every other number. An index made with [`newtype_index!`] is a type of its own,
//! and an [`IndexVec`] or an [`IndexSlice`] takes only its own index type:
//! indexing the workers with a batch's index doesn't compile.
//!
//! The pattern is ruff's (`ruff_index`), written again here: a
//! `macro_rules!` rather than a proc-macro, and no `unsafe`, so an
//! [`IndexSlice`] is a small `Copy` view of a slice rather than a
//! `repr(transparent)` one.
//!
//! Owns: the [`Idx`] trait, [`newtype_index!`], [`IndexVec`] and
//! [`IndexSlice`]. Does not own: any index type, each of which lives with
//! the collection it indexes, nor how an index is written on the wire.
//!
//! Depends on: the standard library only. Must not depend on any aye crate.

mod idx;
mod slice;
mod vec;

pub use idx::Idx;
pub use slice::IndexSlice;
pub use vec::IndexVec;
