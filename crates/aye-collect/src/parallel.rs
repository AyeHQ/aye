//! An order-preserving parallel map on scoped threads.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;

/// `items.iter().map(f).collect()`, spread over the available cores. Results
/// keep the order of `items`. A panic in `f` resumes on the caller.
pub(crate) fn map<T: Sync, R: Send>(items: &[T], f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let threads = thread::available_parallelism()
        .map_or(1, usize::from)
        .min(items.len());
    if threads <= 1 {
        return items.iter().map(f).collect();
    }
    let next = AtomicUsize::new(0);
    let mut out: Vec<Option<R>> = items.iter().map(|_| None).collect();
    thread::scope(|scope| {
        let workers: Vec<_> = (0..threads)
            .map(|_| {
                scope.spawn(|| {
                    let mut done = Vec::new();
                    loop {
                        let at = next.fetch_add(1, Ordering::Relaxed);
                        let Some(item) = items.get(at) else { break };
                        done.push((at, f(item)));
                    }
                    done
                })
            })
            .collect();
        for worker in workers {
            let done = worker
                .join()
                .unwrap_or_else(|panic| std::panic::resume_unwind(panic));
            for (at, result) in done {
                out[at] = Some(result);
            }
        }
    });
    out.into_iter()
        .map(|result| result.expect("every item is mapped"))
        .collect()
}
