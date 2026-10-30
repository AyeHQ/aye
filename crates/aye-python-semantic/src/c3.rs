//! C3 linearization: the order Python gives a class's method resolution
//! order (`__mro__`), from its bases' own.

/// The bases' linearizations don't merge: Python refuses to make the class
/// ("Cannot create a consistent method resolution order").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InconsistentMro;

/// `class`, then the C3 merge of `sequences`: its bases' linearizations,
/// then its bases themselves, in order. Each step takes the first head of
/// a sequence that is in no sequence's tail, and drops it from the heads.
/// What stands for a class, and when two are the same, is the caller's.
pub fn linearize<K: PartialEq + Clone>(
    class: K,
    mut sequences: Vec<Vec<K>>,
) -> Result<Vec<K>, InconsistentMro> {
    let mut out = vec![class];
    loop {
        sequences.retain(|s| !s.is_empty());
        if sequences.is_empty() {
            return Ok(out);
        }
        let candidate = sequences
            .iter()
            .map(|s| &s[0])
            .find(|head| !sequences.iter().any(|s| s[1..].contains(head)))
            .cloned();
        let Some(next) = candidate else {
            return Err(InconsistentMro);
        };
        for sequence in &mut sequences {
            if sequence[0] == next {
                sequence.remove(0);
            }
        }
        out.push(next);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Classes defined one after another, each linearized from its bases'
    /// linearizations, as Python does when it runs the class statements.
    #[derive(Default)]
    struct Classes(Vec<Vec<&'static str>>);

    impl Classes {
        fn class(
            &mut self,
            name: &'static str,
            bases: &[&'static str],
        ) -> Result<Vec<&'static str>, InconsistentMro> {
            let mut sequences: Vec<Vec<&str>> = bases
                .iter()
                .map(|base| self.0.iter().find(|mro| mro[0] == *base).unwrap().clone())
                .collect();
            sequences.push(bases.to_vec());
            let mro = linearize(name, sequences)?;
            self.0.push(mro.clone());
            Ok(mro)
        }
    }

    #[test]
    fn merges_as_python_does() {
        let mut classes = Classes::default();
        assert_eq!(classes.class("O", &[]), Ok(vec!["O"]));
        for name in ["A", "B", "C", "D", "E"] {
            classes.class(name, &["O"]).unwrap();
        }
        assert_eq!(
            classes.class("AB", &["A", "B"]),
            Ok(vec!["AB", "A", "B", "O"])
        );
        // The example of `The Python 2.3 Method Resolution Order`.
        classes.class("K1", &["A", "B", "C"]).unwrap();
        classes.class("K2", &["D", "B", "E"]).unwrap();
        classes.class("K3", &["D", "A"]).unwrap();
        assert_eq!(
            classes.class("Z", &["K1", "K2", "K3"]),
            Ok(vec!["Z", "K1", "K2", "K3", "D", "A", "B", "C", "E", "O"])
        );
    }

    #[test]
    fn bases_in_opposite_orders_do_not_merge() {
        let mut classes = Classes::default();
        classes.class("O", &[]).unwrap();
        classes.class("X", &["O"]).unwrap();
        classes.class("Y", &["O"]).unwrap();
        classes.class("XY", &["X", "Y"]).unwrap();
        classes.class("YX", &["Y", "X"]).unwrap();
        assert_eq!(classes.class("Z", &["XY", "YX"]), Err(InconsistentMro));
        assert_eq!(classes.class("W", &["O", "X"]), Err(InconsistentMro));
    }
}
