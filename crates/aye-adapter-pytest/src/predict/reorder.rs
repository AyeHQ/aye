//! pytest's `reorder_items` over one file's tests.

use std::collections::{HashMap, HashSet};

use crate::marks::Scoped;

/// pytest's `ParamArgKey` for a class- or module-scoped parameter: tests
/// sharing one share the parameter's setup, and pytest moves them
/// together. The file is implied (keys are per file).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) struct ParamKey {
    pub(super) scope: Scoped,
    pub(super) argname: String,
    pub(super) index: usize,
    /// For class scope, the class (its node id); `None` for functions
    /// outside classes.
    pub(super) class: Option<String>,
}

/// pytest's `reorder_items` over one file's tests, given each test's
/// class- and module-scoped parameter keys: the order they run and are
/// listed in. Keys never span files at these scopes, so the file's order
/// is the session's, restricted to it.
pub(super) fn reorder_items(keys: &[Vec<ParamKey>]) -> Vec<usize> {
    /// The keys of each item at a scope, and the items of each key.
    struct Index {
        by_item: Vec<Vec<ParamKey>>,
        by_key: HashMap<ParamKey, Vec<usize>>,
    }
    fn at_scope(items: &[usize], level: usize, indexes: &mut [Index]) -> Vec<usize> {
        if level >= indexes.len() || items.len() < 3 {
            return items.to_vec();
        }
        let members: HashSet<usize> = items.iter().copied().collect();
        let mut ignore: HashSet<ParamKey> = HashSet::new();
        let mut queue: std::collections::VecDeque<usize> = items.iter().copied().collect();
        let mut done: Vec<usize> = Vec::new();
        let mut done_set: HashSet<usize> = HashSet::new();
        while !queue.is_empty() {
            let mut no_key: Vec<usize> = Vec::new();
            let mut slicing: Option<ParamKey> = None;
            while let Some(item) = queue.pop_front() {
                if done_set.contains(&item) || no_key.contains(&item) {
                    continue;
                }
                let live: Vec<&ParamKey> = indexes[level].by_item[item]
                    .iter()
                    .filter(|k| !ignore.contains(*k))
                    .collect();
                // `dict.popitem()` takes the last one.
                let Some(key) = live.last().map(|k| (*k).clone()) else {
                    no_key.push(item);
                    continue;
                };
                let matching: Vec<usize> = indexes[level].by_key[&key]
                    .iter()
                    .copied()
                    .filter(|i| members.contains(i))
                    .collect();
                for &i in matching.iter().rev() {
                    queue.push_front(i);
                    // Keep every scope's lists in the new order.
                    for index in indexes.iter_mut() {
                        for k in index.by_item[i].clone() {
                            let list = index.by_key.get_mut(&k).expect("indexed");
                            list.retain(|x| *x != i);
                            list.insert(0, i);
                        }
                    }
                }
                slicing = Some(key);
                break;
            }
            if !no_key.is_empty() {
                for item in at_scope(&no_key, level + 1, indexes) {
                    if done_set.insert(item) {
                        done.push(item);
                    }
                }
            }
            if let Some(key) = slicing {
                ignore.insert(key);
            }
        }
        done
    }

    const SCOPES: [Scoped; 2] = [Scoped::Module, Scoped::Class];
    let mut indexes: Vec<Index> = SCOPES
        .iter()
        .map(|scope| {
            let mut by_item = Vec::with_capacity(keys.len());
            let mut by_key: HashMap<ParamKey, Vec<usize>> = HashMap::new();
            for (item, item_keys) in keys.iter().enumerate() {
                let mut own: Vec<ParamKey> = Vec::new();
                for key in item_keys.iter().filter(|k| k.scope == *scope) {
                    if !own.contains(key) {
                        own.push(key.clone());
                        by_key.entry(key.clone()).or_default().push(item);
                    }
                }
                by_item.push(own);
            }
            Index { by_item, by_key }
        })
        .collect();

    let all: Vec<usize> = (0..keys.len()).collect();
    at_scope(&all, 0, &mut indexes)
}
