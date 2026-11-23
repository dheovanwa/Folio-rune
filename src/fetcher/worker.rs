//! The background thread that turns planned chapters into cached ones.
//!
//! One thread, not a pool. The work is "fetch the next chapter while the reader reads this one",
//! which is one request at a time; a pool would only turn the per-host limiter into a queue of
//! threads all waiting on the same permit.
//!
//! The thread owns nothing the UI needs synchronously. The UI calls [`Prefetcher::plan`], which
//! is cheap and returns immediately, and later calls [`Prefetcher::take_completed`] to collect
//! what landed. All network I/O happens on the worker, which is the whole reason this module
//! exists: a frozen reading surface while chapter N+1 downloads is worse than no prefetch at all.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;

use super::http::{Cancel, Client, Failure};
use super::prefetch::{Job, Planner, Skipped};
use crate::cache::{Cache, Entry, Kind};

/// How a prefetch ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Landed {
    /// Fetched and written to the cache, under the URL that was asked for.
    Cached {
        url: String,
        /// Where the request actually landed after redirects. The extractor resolves relative
        /// links against this, so it is handed back rather than dropped.
        final_url: String,
        bytes: usize,
    },
    /// Already in the cache when the worker got to it — the reader opened it first.
    AlreadyCached { url: String },
    /// The fetch failed. `Failure` carries why.
    Failed { url: String, error: Failure },
    /// A newer chapter superseded this one before it started.
    Dropped { url: String },
}

impl Landed {
    pub fn url(&self) -> &str {
        match self {
            Landed::Cached { url, .. }
            | Landed::AlreadyCached { url }
            | Landed::Failed { url, .. }
            | Landed::Dropped { url } => url,
        }
    }
}

struct Shared {
    cache: Arc<Cache>,
    client: Arc<Client>,
    planner: Mutex<Planner>,
    /// Signalled when a job is queued or the worker is asked to stop.
    wake: Condvar,
    /// Guards `wake`'s predicate.
    signal: Mutex<bool>,
    completed: Mutex<Vec<Landed>>,
    /// The chapter the reader is on. Jobs from any other origin are abandoned.
    origin: Mutex<String>,
    /// The cancel flag for the fetch currently allowed to run. Replaced, not reused, when the
    /// reader moves on: a flag that stayed set would cancel every later fetch too.
    cancel: Mutex<Cancel>,
    shutdown: AtomicBool,
}

impl Shared {
    fn completed(&self) -> MutexGuard<'_, Vec<Landed>> {
        self.completed.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn plan(&self) -> MutexGuard<'_, Planner> {
        self.planner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn cancel(&self) -> MutexGuard<'_, Cancel> {
        self.cancel.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Snapshot the current cancel flag for one job.
    fn current_cancel(&self) -> Cancel {
        self.cancel().clone()
    }

    /// Abandon whatever is in flight and start a fresh epoch, so later fetches still run.
    fn restart_epoch(&self) {
        let mut cancel = self.cancel();
        cancel.cancel();
        *cancel = Cancel::new();
    }

    /// One job, or `None` when the queue is empty or the reader has moved on.
    fn next_job(&self) -> Option<Job> {
        let origin = self.origin.lock().unwrap_or_else(PoisonError::into_inner).clone();
        let mut planner = self.plan();
        planner.cancel_except(&origin);
        planner.pop()
    }
}

/// The prefetch service.
pub struct Prefetcher {
    shared: Arc<Shared>,
    worker: Option<JoinHandle<()>>,
}

impl Prefetcher {
    /// Start the worker over an existing cache and client.
    pub fn spawn(cache: Arc<Cache>, client: Arc<Client>, queue_cap: usize) -> Prefetcher {
        let shared = Arc::new(Shared {
            cache,
            client,
            planner: Mutex::new(Planner::new(queue_cap)),
            wake: Condvar::new(),
            signal: Mutex::new(false),
            completed: Mutex::new(Vec::new()),
            origin: Mutex::new(String::new()),
            cancel: Mutex::new(Cancel::new()),
            shutdown: AtomicBool::new(false),
        });
        let worker = {
            let shared = Arc::clone(&shared);
            std::thread::Builder::new()
                .name("folio-prefetch".into())
                .spawn(move || run(shared))
                .ok()
        };
        Prefetcher { shared, worker }
    }

    /// Tell the prefetcher which chapter the reader is on, and what it links to as "next".
    ///
    /// Returns the jobs queued and the reason for every candidate, so the caller can log a
    /// skipped prefetch without guessing.
    pub fn plan(
        &self,
        origin: &str,
        next: &[String],
    ) -> (Vec<Job>, Vec<(String, Option<Skipped>)>) {
        let moved = {
            let mut current = self.shared.origin.lock().unwrap_or_else(PoisonError::into_inner);
            let moved = *current != origin;
            *current = origin.to_string();
            moved
        };
        if moved {
            // A fetch for the previous chapter is now wasted bandwidth; stop it and let the new
            // chapter's jobs start on a clean flag.
            self.shared.restart_epoch();
        }
        let cache = Arc::clone(&self.shared.cache);
        let (queued, report) = {
            let mut planner = self.shared.plan();
            planner.plan(origin, next, |url| cache.contains(url))
        };
        if !queued.is_empty() {
            self.poke();
        }
        (queued, report)
    }

    /// Take everything the worker has finished since the last call.
    pub fn take_completed(&self) -> Vec<Landed> {
        std::mem::take(&mut *self.shared.completed())
    }

    /// How many jobs are waiting.
    pub fn queued(&self) -> usize {
        self.shared.plan().len()
    }

    /// Abandon the current speculation, e.g. when the reader closes the article.
    ///
    /// This cancels the fetch that is running but leaves the prefetcher usable: the next `plan`
    /// starts a fresh epoch. A prefetcher that stayed cancelled after a close would silently
    /// stop prefetching for the rest of the session.
    pub fn clear(&self) {
        {
            let mut origin = self.shared.origin.lock().unwrap_or_else(PoisonError::into_inner);
            origin.clear();
        }
        self.shared.plan().cancel_except("");
        self.shared.restart_epoch();
    }

    fn poke(&self) {
        let mut signal = self.shared.signal.lock().unwrap_or_else(PoisonError::into_inner);
        *signal = true;
        self.shared.wake.notify_one();
    }

    /// Stop the worker and wait for it. A fetch in flight is cancelled, not joined forever.
    pub fn shutdown(mut self) {
        self.shared.shutdown.store(true, Ordering::SeqCst);
        self.shared.cancel().cancel();
        self.poke();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Drop for Prefetcher {
    fn drop(&mut self) {
        self.shared.shutdown.store(true, Ordering::SeqCst);
        self.shared.cancel().cancel();
        {
            let mut signal = self.shared.signal.lock().unwrap_or_else(PoisonError::into_inner);
            *signal = true;
            self.shared.wake.notify_one();
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn run(shared: Arc<Shared>) {
    loop {
        if shared.shutdown.load(Ordering::SeqCst) {
            return;
        }
        let Some(job) = shared.next_job() else {
            // Nothing to do: park until `plan` pokes, or until the shutdown flag flips.
            let mut signal = shared.signal.lock().unwrap_or_else(PoisonError::into_inner);
            while !*signal && !shared.shutdown.load(Ordering::SeqCst) {
                let (guard, timeout) =
                    shared.wake.wait_timeout(signal, Duration::from_millis(500)).unwrap_or_else(|e| e.into_inner());
                signal = guard;
                let _ = timeout;
            }
            *signal = false;
            continue;
        };

        if shared.shutdown.load(Ordering::SeqCst) {
            shared.completed().push(Landed::Dropped { url: job.url });
            return;
        }

        // The reader may have opened this chapter while it sat in the queue. Re-checking here
        // rather than only at plan time is what keeps the queue honest when a click beats the
        // worker.
        if shared.cache.contains(&job.url) {
            shared.completed().push(Landed::AlreadyCached { url: job.url });
            continue;
        }

        // The cancel flag is snapshotted per job: a later `plan` replaces the shared flag, and
        // this in-flight request keeps watching the one it started with.
        let cancel = shared.current_cancel();
        match shared.client.get(&job.url, &cancel) {
            Ok(page) => {
                // Keyed by the URL that was requested, not the one after redirects: that is the
                // key the planner checks and the next request will use, so a redirect does not
                // make the same chapter prefetch itself again on every visit.
                let bytes = page.body.len();
                let final_url = page.url.clone();
                let entry = Entry::new(job.url.clone(), Kind::Article, page.body);
                let landed = match shared.cache.put(&entry) {
                    Ok(()) => Landed::Cached { url: job.url, final_url, bytes },
                    Err(e) => Landed::Failed { url: job.url, error: Failure::Network(e.to_string()) },
                };
                shared.completed().push(landed);
            }
            Err(Failure::Cancelled) => {
                shared.completed().push(Landed::Dropped { url: job.url });
            }
            Err(error) => {
                shared.completed().push(Landed::Failed { url: job.url, error });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::codec::Identity;
    use crate::cache::store::MemoryStore;

    fn prefetcher() -> Prefetcher {
        let cache = Cache::open(Box::new(MemoryStore::default()), Box::new(Identity), 1 << 20);
        let client = Arc::new(Client::with_defaults());
        Prefetcher::spawn(cache, client, 4)
    }

    #[test]
    fn a_chapter_already_in_the_cache_is_not_queued() {
        let cache = Cache::open(Box::new(MemoryStore::default()), Box::new(Identity), 1 << 20);
        cache.put(&Entry::new("https://ex.com/ch/2", Kind::Article, b"<html></html>".to_vec())).unwrap();
        let client = Arc::new(Client::with_defaults());
        let p = Prefetcher::spawn(Arc::clone(&cache), client, 4);

        let next = vec!["https://ex.com/ch/2".to_string(), "https://ex.com/ch/3".to_string()];
        let (queued, report) = p.plan("https://ex.com/ch/1", &next);
        assert_eq!(queued.len(), 1, "only N+2 is new");
        assert_eq!(report[0].1, Some(Skipped::Cached));
        p.shutdown();
    }

    #[test]
    fn shutdown_returns_promptly_with_an_empty_queue() {
        let p = prefetcher();
        let started = std::time::Instant::now();
        p.shutdown();
        assert!(started.elapsed() < Duration::from_secs(2), "took {:?}", started.elapsed());
    }

    #[test]
    fn a_new_chapter_drops_the_previous_chapters_jobs() {
        let p = prefetcher();
        // The origin moves before the worker picks anything up, so the old jobs are abandoned.
        p.plan("https://ex.com/ch/1", &["https://ex.com/ch/2".to_string()]);
        p.plan("https://ex.com/ch/9", &["https://ex.com/ch/10".to_string()]);
        let queued = p.queued();
        assert!(queued <= 1, "the previous chapter's job was abandoned, queue was {queued}");
        p.shutdown();
    }

    #[test]
    fn clear_leaves_the_prefetcher_usable() {
        let p = prefetcher();
        p.plan("https://ex.com/ch/1", &["https://ex.com/ch/2".to_string()]);
        p.clear();
        // A fresh plan after a close must not be cancelled by the flag the close set.
        let (queued, _) = p.plan("https://ex.com/ch/5", &["https://ex.com/ch/6".to_string()]);
        assert_eq!(queued.len(), 1, "prefetching still works after a clear");
        p.shutdown();
    }

    #[test]
    fn moving_chapters_replaces_the_cancel_epoch_instead_of_reusing_it() {
        let p = prefetcher();
        p.plan("https://ex.com/ch/1", &["https://ex.com/ch/2".to_string()]);
        p.plan("https://ex.com/ch/2", &["https://ex.com/ch/3".to_string()]);
        // If the epoch were reused, this flag would stay set and every later job would be
        // dropped as cancelled.
        assert!(!p.shared.current_cancel().is_cancelled(), "the new chapter gets a clean flag");
        p.shutdown();
    }
}
