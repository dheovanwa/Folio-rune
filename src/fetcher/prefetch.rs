//! Which chapters to fetch ahead of the reader, and in what order.
//!
//! The planner is deliberately not a thread and not a scheduler. It is a pure function over a
//! queue: the reader says "I am on chapter N", the planner says "these two URLs are worth
//! fetching, in this order, and here is why the rest were skipped". Something else does the I/O.
//! That split is what makes prefetching testable at all — the interesting behaviour is the
//! ordering and the skipping, not the sockets.
//!
//! Two rules shape it:
//!
//! - **Depth is two, not twenty.** N+1 and N+2 cover the reader who finishes a chapter and keeps
//!   going, without spending a site's goodwill on chapters that will never be opened.
//! - **Cached is not queued.** A URL already in the cache, or already in the queue, is skipped.
//!   Without that, every redraw of a chapter re-queues the same two URLs and the queue becomes
//!   an infinite loop with extra steps.

use std::collections::VecDeque;

/// How far ahead the planner looks. Two chapters is the whole point of the module.
pub const LOOKAHEAD: usize = 2;

/// Why a URL was not queued. Returned so a caller can log it, and so the tests can assert on the
/// reason instead of on a bare `false`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Skipped {
    /// Already waiting in the queue.
    Queued,
    /// Already in the cache, fresh or not.
    Cached,
    /// The queue is at its cap.
    Full,
    /// Not a URL the fetcher can rank, e.g. an empty string.
    NotAUrl,
}

/// One unit of speculative work.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Job {
    pub url: String,
    /// 1 for N+1, 2 for N+2. Lower is more urgent.
    pub depth: u8,
    /// The chapter the reader was on when this was queued, for cancellation.
    pub origin: String,
}

/// The prefetch queue. Single-threaded; the fetcher owns one behind a mutex.
#[derive(Debug, Default)]
pub struct Planner {
    queue: VecDeque<Job>,
    cap: usize,
}

impl Planner {
    /// A planner that will hold at most `cap` jobs.
    pub fn new(cap: usize) -> Planner {
        Planner { queue: VecDeque::with_capacity(cap.min(64)), cap }
    }

    /// Queue the chapters after `origin`.
    ///
    /// `next` is the URLs the current page links to as its successors, in reading order — the
    /// caller gets them from the extracted document, so the planner never parses HTML and never
    /// guesses a chapter number. Only the first [`LOOKAHEAD`] *usable* links are considered: a
    /// share button or a `#` anchor before the next chapter must not eat the reader's lookahead.
    ///
    /// Returns the jobs queued and, in the same order, what happened to each candidate.
    pub fn plan(
        &mut self,
        origin: &str,
        next: &[String],
        is_cached: impl Fn(&str) -> bool,
    ) -> (Vec<Job>, Vec<(String, Option<Skipped>)>) {
        // Moving to a new chapter abandons the old chapter's speculation: those URLs are one
        // step behind the reader now and would only compete with the fresh ones.
        self.queue.retain(|job| job.origin == origin);

        let mut queued = Vec::new();
        let mut report = Vec::new();
        let mut usable = 0;

        for url in next {
            let url = url.trim();
            if url.is_empty() || !looks_like_url(url) {
                report.push((url.to_string(), Some(Skipped::NotAUrl)));
                continue;
            }
            usable += 1;
            if usable > LOOKAHEAD {
                // Everything past the second usable link is out of scope, so it is not reported
                // as skipped either: it was never a candidate.
                break;
            }
            let depth = usable as u8;
            if self.queue.iter().any(|job| job.url == url) {
                report.push((url.to_string(), Some(Skipped::Queued)));
                continue;
            }
            if is_cached(url) {
                report.push((url.to_string(), Some(Skipped::Cached)));
                continue;
            }
            if self.queue.len() >= self.cap {
                report.push((url.to_string(), Some(Skipped::Full)));
                continue;
            }
            let job = Job { url: url.to_string(), depth, origin: origin.to_string() };
            self.queue.push_back(job.clone());
            queued.push(job);
            report.push((url.to_string(), None));
        }
        (queued, report)
    }

    /// Take the most urgent job. N+1 before N+2, and within a depth, first queued first.
    pub fn pop(&mut self) -> Option<Job> {
        // The queue is at most a handful of entries, so a scan beats keeping it sorted.
        let best = self
            .queue
            .iter()
            .enumerate()
            .min_by_key(|(i, job)| (job.depth, *i))
            .map(|(i, _)| i)?;
        self.queue.remove(best)
    }

    /// Drop everything queued from a chapter other than `origin`.
    pub fn cancel_except(&mut self, origin: &str) {
        self.queue.retain(|job| job.origin == origin);
    }

    pub fn len(&self) -> usize {
        self.queue.len()
    }

    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    /// The queued jobs, most urgent first, without removing them.
    pub fn peek_all(&self) -> Vec<Job> {
        let mut all: Vec<Job> = self.queue.iter().cloned().collect();
        all.sort_by_key(|job| job.depth);
        all
    }
}

/// A cheap sanity check. The planner is not a validator; it only refuses strings that could not
/// be a fetchable address, so a `javascript:` link or a stray `#` never reaches a socket.
fn looks_like_url(url: &str) -> bool {
    if url.starts_with('#') || url.contains(char::is_whitespace) {
        return false;
    }
    let lower = url.to_ascii_lowercase();
    if lower.starts_with("javascript:") || lower.starts_with("mailto:") || lower.starts_with("data:") {
        return false;
    }
    url.starts_with("http://") || url.starts_with("https://") || url.contains('.')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn urls() -> Vec<String> {
        vec![
            "https://ex.com/ch/2".to_string(),
            "https://ex.com/ch/3".to_string(),
            "https://ex.com/ch/4".to_string(),
        ]
    }

    #[test]
    fn only_the_next_two_chapters_are_queued() {
        let mut p = Planner::new(8);
        let (queued, _) = p.plan("https://ex.com/ch/1", &urls(), |_| false);
        assert_eq!(queued.len(), LOOKAHEAD);
        assert_eq!(queued[0].depth, 1);
        assert_eq!(queued[1].depth, 2);
    }

    #[test]
    fn n_plus_one_is_served_before_n_plus_two() {
        let mut p = Planner::new(8);
        p.plan("https://ex.com/ch/1", &urls(), |_| false);
        assert_eq!(p.pop().unwrap().url, "https://ex.com/ch/2");
        assert_eq!(p.pop().unwrap().url, "https://ex.com/ch/3");
        assert!(p.pop().is_none());
    }

    #[test]
    fn a_cached_chapter_is_not_queued_again() {
        let mut p = Planner::new(8);
        let (queued, report) =
            p.plan("https://ex.com/ch/1", &urls(), |u| u.ends_with("/ch/2"));
        assert_eq!(queued.len(), 1, "only N+2 survives");
        assert_eq!(report[0].1, Some(Skipped::Cached));
        assert_eq!(report[1].1, None);
    }

    #[test]
    fn the_same_url_is_not_queued_twice() {
        let mut p = Planner::new(8);
        p.plan("https://ex.com/ch/1", &urls(), |_| false);
        let (queued, report) = p.plan("https://ex.com/ch/1", &urls(), |_| false);
        assert!(queued.is_empty());
        assert!(report.iter().all(|(_, why)| *why == Some(Skipped::Queued)));
        assert_eq!(p.len(), 2, "the queue did not grow");
    }

    #[test]
    fn moving_to_a_new_chapter_abandons_the_old_speculation() {
        let mut p = Planner::new(8);
        p.plan("https://ex.com/ch/1", &urls(), |_| false);
        let (queued, _) = p.plan("https://ex.com/ch/2", &urls(), |_| false);
        assert!(queued.len() <= LOOKAHEAD);
        assert!(p.peek_all().iter().all(|job| job.origin == "https://ex.com/ch/2"));
    }

    #[test]
    fn a_full_queue_says_so_instead_of_growing() {
        let mut p = Planner::new(1);
        let (queued, report) = p.plan("https://ex.com/ch/1", &urls(), |_| false);
        assert_eq!(queued.len(), 1);
        assert_eq!(report[1].1, Some(Skipped::Full));
    }

    #[test]
    fn a_link_that_cannot_be_fetched_never_reaches_the_queue() {
        let mut p = Planner::new(8);
        let links = vec![
            "javascript:void(0)".to_string(),
            "#part-2".to_string(),
            "mailto:a@b.c".to_string(),
            "".to_string(),
            "https://ex.com/ch/2".to_string(),
        ];
        let (queued, report) = p.plan("https://ex.com/ch/1", &links, |_| false);
        assert_eq!(queued.len(), 1);
        assert_eq!(queued[0].url, "https://ex.com/ch/2");
        assert_eq!(report[0].1, Some(Skipped::NotAUrl));
    }
}
