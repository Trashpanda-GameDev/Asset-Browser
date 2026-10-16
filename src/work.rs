//! What the background threads share: starting one, locking what they share, the list of work
//! they take from, and the call that wakes the window when something is ready.

use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;

use crate::diag;

/// Asks the window to draw again: a background thread calls it when something it made is ready.
pub type Wake = Arc<dyn Fn() + Send + Sync>;

/// Starts a named thread. If the OS won't start one, that's reported and the work that needed it
/// doesn't happen: the app carries on without it.
pub fn spawn(name: &str, work: impl FnOnce() + Send + 'static) -> Option<JoinHandle<()>> {
    std::thread::Builder::new()
        .name(name.to_owned())
        .spawn(work)
        .map_err(|err| diag::report(&format!("Starting the {name} thread"), err))
        .ok()
}

/// Locks `mutex`, also after a thread panicked while holding it: what the threads share (queues,
/// counters, caches) stays usable, and one bad file mustn't stop everything else.
pub fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Waits on `condvar`, the same way.
pub fn wait<'a, T>(condvar: &Condvar, guard: MutexGuard<'a, T>) -> MutexGuard<'a, T> {
    condvar.wait(guard).unwrap_or_else(PoisonError::into_inner)
}

/// What a background thread does next. Each new list replaces whatever is still waiting, so work
/// nobody wants any more is never started; closing it ends the thread's wait for good.
pub struct WorkList<T> {
    state: Mutex<ListState<T>>,
    changed: Condvar,
}

struct ListState<T> {
    waiting: VecDeque<T>,
    closed: bool,
}

impl<T> WorkList<T> {
    pub fn new() -> Self {
        Self { state: Mutex::new(ListState { waiting: VecDeque::new(), closed: false }), changed: Condvar::new() }
    }

    /// Replaces what's waiting with `items`, in their order.
    pub fn replace(&self, items: impl IntoIterator<Item = T>) {
        let mut state = lock(&self.state);
        state.waiting.clear();
        state.waiting.extend(items);
        drop(state);
        self.changed.notify_all();
    }

    /// The next item, once there is one; `None` when the list is closed.
    pub fn take(&self) -> Option<T> {
        let mut state = lock(&self.state);
        loop {
            if state.closed {
                return None;
            }
            if let Some(item) = state.waiting.pop_front() {
                return Some(item);
            }
            state = wait(&self.changed, state);
        }
    }

    /// Whether the item taken last is out of date: something newer is waiting, or the list is
    /// closed. A long job checks it to give up early.
    pub fn superseded(&self) -> bool {
        let state = lock(&self.state);
        !state.waiting.is_empty() || state.closed
    }

    /// Ends `take` on every thread, now and from then on.
    pub fn close(&self) {
        lock(&self.state).closed = true;
        self.changed.notify_all();
    }
}

impl<T> Default for WorkList<T> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn a_new_list_replaces_what_is_waiting() {
        let list = WorkList::new();
        list.replace([1, 2, 3]);
        assert_eq!(list.take(), Some(1));
        list.replace([7]);
        assert!(list.superseded(), "7 is waiting");
        assert_eq!(list.take(), Some(7), "2 and 3 aren't wanted any more");
        assert!(!list.superseded());
    }

    #[test]
    fn closing_wakes_a_waiting_thread() {
        let list = Arc::new(WorkList::<u32>::new());
        let (tx, rx) = std::sync::mpsc::channel();
        let waiting = {
            let list = list.clone();
            std::thread::spawn(move || tx.send(list.take()).unwrap())
        };
        assert!(rx.recv_timeout(Duration::from_millis(100)).is_err(), "nothing to take yet");
        list.close();
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), None);
        waiting.join().unwrap();
        assert!(list.superseded(), "a closed list supersedes whatever was being done");
        list.replace([1]);
        assert_eq!(list.take(), None, "nothing is taken once it's closed");
    }

    #[test]
    fn a_lock_survives_a_panic() {
        let shared = Arc::new(Mutex::new(5));
        let holder = shared.clone();
        let _ = std::thread::spawn(move || {
            let _guard = holder.lock().unwrap();
            panic!("a decoder fell over");
        })
        .join();
        assert!(shared.is_poisoned());
        assert_eq!(*lock(&shared), 5);
    }
}
