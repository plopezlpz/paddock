//! A thread of its own for slow facts (git's view of a folder, what a
//! transcript says), so the popup never waits for one: asked for by key,
//! answered through a callback. A key already asked for and not yet taken up
//! is not queued twice.

use std::collections::HashSet;
use std::hash::Hash;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread;

pub struct Worker<K> {
    tx: Sender<K>,
    pending: Arc<Mutex<HashSet<K>>>,
}

impl<K: Clone + Eq + Hash + Send + 'static> Worker<K> {
    /// WORK each key asked for, in order, and give ANSWER the key and result.
    pub fn new<V>(work: impl Fn(&K) -> V + Send + 'static, answer: impl Fn(K, V) + Send + 'static) -> Worker<K> {
        let (tx, rx) = mpsc::channel::<K>();
        let pending = Arc::new(Mutex::new(HashSet::new()));
        let taken = Arc::clone(&pending);
        thread::spawn(move || {
            for key in rx {
                // Worked out from now on: asked again meanwhile, it is queued
                // again, as what it stands for may have changed since.
                taken.lock().unwrap_or_else(|e| e.into_inner()).remove(&key);
                let value = work(&key);
                answer(key, value);
            }
        });
        Worker { tx, pending }
    }

    pub fn ask(&self, key: K) {
        if self.pending.lock().unwrap_or_else(|e| e.into_inner()).insert(key.clone()) {
            let _ = self.tx.send(key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_each_key_asked_for() {
        let (tx, rx) = mpsc::channel();
        let w = Worker::new(|n: &u32| n * 2, move |k, v| tx.send((k, v)).unwrap());
        w.ask(1);
        w.ask(2);
        assert_eq!((rx.recv().unwrap(), rx.recv().unwrap()), ((1, 2), (2, 4)));
    }
}
