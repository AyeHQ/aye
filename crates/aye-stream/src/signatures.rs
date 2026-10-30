//! The first bad outcome of each failure signature ([`FirstBySignature`]),
//! which the ones after it are shown as the same as.
//!
//! The engine gives a failure its signature; one without is never
//! grouped. What each renderer keeps of the first (a key and a message, an
//! annotation's place) is its own.

use std::collections::HashMap;

use aye_events::FailureSignature;

/// What a renderer keeps of the first failure of each signature.
#[derive(Debug)]
pub struct FirstBySignature<T> {
    first: HashMap<FailureSignature, T>,
}

impl<T> Default for FirstBySignature<T> {
    fn default() -> Self {
        Self {
            first: HashMap::new(),
        }
    }
}

impl<T> FirstBySignature<T> {
    pub fn new() -> Self {
        Self::default()
    }

    /// What was kept of the first failure with `signature`, when one came
    /// before; else `first()` is kept, as this one's, and `None` returned.
    pub fn first_or_keep(
        &mut self,
        signature: &FailureSignature,
        first: impl FnOnce() -> T,
    ) -> Option<&T> {
        if self.first.contains_key(signature) {
            return self.first.get(signature);
        }
        self.first.insert(signature.clone(), first());
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_of_a_signature_is_kept() {
        let (a, b) = (
            FailureSignature::new("a").unwrap(),
            FailureSignature::new("b").unwrap(),
        );
        let mut first = FirstBySignature::new();
        assert_eq!(first.first_or_keep(&a, || "3fae6"), None);
        assert_eq!(first.first_or_keep(&b, || "4d4f6"), None);
        assert_eq!(first.first_or_keep(&a, || "c3d4e"), Some(&"3fae6"));
    }
}
