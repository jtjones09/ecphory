// SUBSCRIPTIONS — Persistent attention with fault isolation (Spec 8 §6)
//
// Per spec §6.A: an observer is an agent that holds a generative model
// of what patterns it cares about. Observation is the relation between
// fabric state and observer pattern at a moment of attention.
//
// Per spec §6.B (algorithmic):
// - Pattern matching on write commit. When a write commits, evaluate
//   each registered subscription's predicate against the new node.
// - Callback dispatch on a dedicated thread pool (default 8 threads)
//   per §2.6.1. Callbacks NEVER run on the request path.
// - Push, not pull. The fabric pushes matched events to subscribers.
// - Backpressure: a subscriber whose callback queue grows beyond a
//   configurable threshold (default 1,000) is marked `Lagged`.
//   Subsequent matches enqueue a single `Lagged` summary and skip
//   individual matches until the queue drains.
//
// Per spec §2.6.1:
// - A panic in a subscription callback is caught at the callback
//   boundary, logged structured, and converted to a `SubscriptionPanic`
//   event. Nabu's HTTP request handlers never observe a callback panic.
//
// v1 implementation: standard library only. Single mpsc channel feeding
// a worker pool. Per-subscription `AtomicUsize` queue depth + `AtomicBool`
// lagged flag. `catch_unwind` around the callback invocation.

use std::collections::HashMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, RwLock};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::node::IntentNode;
use metrics::{counter, histogram};

use super::observability::{
    METRIC_FABRIC_PANIC_TOTAL, METRIC_FABRIC_SUBSCRIPTION_CALLBACK_LATENCY_SECONDS,
};

/// Channel through which the dispatch worker reports panics back to
/// the bridge so they can be materialized as fabric nodes (Spec 8 §13
/// self-witnessing). The sender is held by the `BridgeFabric` and
/// cloned into each `SubscriptionEntry`. A backlog beyond a small
/// bounded queue (default 4096) drops events with a structured
/// warning — the panic counter still increments either way.
pub type PanicReporter = Arc<dyn Fn(SubscriptionPanicEvent) + Send + Sync>;

/// Structured event reported when a callback panics.
#[derive(Debug, Clone)]
pub struct SubscriptionPanicEvent {
    pub subscription_id: SubscriptionId,
    pub panic_message: String,
}

// ── Public types ──────────────────────────────────────────────────

/// Opaque handle returned by `subscribe`. Passed back to `unsubscribe`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SubscriptionId(pub u64);

impl std::fmt::Display for SubscriptionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "sub:{}", self.0)
    }
}

/// Predicate evaluated against the candidate node at write time.
/// Returns `true` if the subscription is interested in the node.
///
/// Note: per Spec 8 §6.C.1 the implementation may swap a trie-keyed
/// index in the future; the spec does not constrain the data
/// structure. v1 walks subscriptions linearly.
pub type Predicate = Arc<dyn Fn(&IntentNode) -> bool + Send + Sync>;

/// User-supplied subscription callback. Invoked on the dispatch pool —
/// never on the write-request path. Per spec §6.B.2 the return value
/// signals dispatch follow-up.
pub type Callback = Arc<dyn Fn(&IntentNode, ObserverContext) -> CallbackResult + Send + Sync>;

/// Outcome of a callback invocation (Spec 8 §7).
#[derive(Debug, Clone, PartialEq)]
pub enum CallbackResult {
    Success,
    /// Triggers a single retry after a 100ms backoff.
    RetryRequested,
    /// Surfaces a `SubscriptionError` event (Spec 8 §6.B.2).
    UnrecoverableError(String),
}

/// Context handed to a callback. Includes the subscription's identity
/// plus per-subscriber observation count for accountability /
/// backpressure visibility.
#[derive(Debug, Clone)]
pub struct ObserverContext {
    pub subscription_id: SubscriptionId,
    pub observation_count: u64,
}

/// Errors emitted by `subscribe` / `unsubscribe`.
#[derive(Debug, Clone, PartialEq)]
pub enum SubscribeError {
    PatternInvalid { reason: String },
    SubscriptionLimitExceeded,
    FabricDegraded,
    UnknownSubscription(SubscriptionId),
}

impl std::fmt::Display for SubscribeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SubscribeError::PatternInvalid { reason } => {
                write!(f, "pattern invalid: {}", reason)
            }
            SubscribeError::SubscriptionLimitExceeded => {
                write!(f, "subscription limit exceeded")
            }
            SubscribeError::FabricDegraded => write!(f, "fabric is in degraded mode"),
            SubscribeError::UnknownSubscription(id) => {
                write!(f, "no subscription found for {}", id)
            }
        }
    }
}

impl std::error::Error for SubscribeError {}

// ── Per-subscription state (Spec 8 §6.C.3) ────────────────────────

/// State held for each active subscription. Backpressure-relevant
/// counters are atomics so the dispatch path can update them without
/// holding the registry lock.
pub struct SubscriptionEntry {
    pub id: SubscriptionId,
    pub pattern: Predicate,
    pub callback: Callback,
    pub queue_depth: AtomicUsize,
    pub observation_count: AtomicU64,
    pub panic_count: AtomicU64,
    pub lagged: AtomicBool,
    /// Optional reporter the worker calls when a callback panics.
    /// When wired by `BridgeFabric`, panics also become fabric nodes
    /// (Spec 8 §13 self-witnessing). When `None`, only stderr + the
    /// metric counter record the panic.
    pub panic_reporter: Mutex<Option<PanicReporter>>,
}

impl SubscriptionEntry {
    fn new(id: SubscriptionId, pattern: Predicate, callback: Callback) -> Self {
        Self {
            id,
            pattern,
            callback,
            queue_depth: AtomicUsize::new(0),
            observation_count: AtomicU64::new(0),
            panic_count: AtomicU64::new(0),
            lagged: AtomicBool::new(false),
            panic_reporter: Mutex::new(None),
        }
    }
}

/// Snapshot of a subscription's runtime state — exposed to callers via
/// `BridgeFabric::subscription_state` so the immune system / debug
/// endpoint (Spec 8 §8.5.3) can read it.
#[derive(Debug, Clone)]
pub struct SubscriptionState {
    pub id: SubscriptionId,
    pub queue_depth: usize,
    pub observation_count: u64,
    pub panic_count: u64,
    pub lagged: bool,
}

// ── Registry ──────────────────────────────────────────────────────

/// Holds active subscriptions. Read-mostly; write only on
/// subscribe/unsubscribe.
pub struct SubscriptionRegistry {
    next_id: AtomicU64,
    entries: RwLock<HashMap<SubscriptionId, Arc<SubscriptionEntry>>>,
    /// Per Spec 8 SubscribeError::SubscriptionLimitExceeded — a soft
    /// cap to avoid runaway leaks. v1 default is permissive.
    limit: usize,
}

impl SubscriptionRegistry {
    pub fn new(limit: usize) -> Self {
        Self {
            next_id: AtomicU64::new(1),
            entries: RwLock::new(HashMap::new()),
            limit,
        }
    }

    pub fn add(
        &self,
        pattern: Predicate,
        callback: Callback,
    ) -> Result<SubscriptionId, SubscribeError> {
        self.add_with_panic_reporter(pattern, callback, None)
    }

    pub fn add_with_panic_reporter(
        &self,
        pattern: Predicate,
        callback: Callback,
        panic_reporter: Option<PanicReporter>,
    ) -> Result<SubscriptionId, SubscribeError> {
        let mut entries = self.entries.write().expect("subscription registry poisoned");
        if entries.len() >= self.limit {
            return Err(SubscribeError::SubscriptionLimitExceeded);
        }
        let id = SubscriptionId(self.next_id.fetch_add(1, Ordering::Relaxed));
        let entry = Arc::new(SubscriptionEntry::new(id, pattern, callback));
        if let Some(reporter) = panic_reporter {
            *entry.panic_reporter.lock().expect("panic reporter mutex poisoned") =
                Some(reporter);
        }
        entries.insert(id, entry);
        Ok(id)
    }

    pub fn remove(&self, id: SubscriptionId) -> Result<(), SubscribeError> {
        let mut entries = self.entries.write().expect("subscription registry poisoned");
        entries
            .remove(&id)
            .ok_or(SubscribeError::UnknownSubscription(id))?;
        Ok(())
    }

    /// Snapshot of all active subscription entries (Arc-cloned so the
    /// dispatch path can hold them without keeping the registry lock).
    pub fn snapshot(&self) -> Vec<Arc<SubscriptionEntry>> {
        let entries = self.entries.read().expect("subscription registry poisoned");
        entries.values().cloned().collect()
    }

    pub fn state(&self, id: SubscriptionId) -> Option<SubscriptionState> {
        let entries = self.entries.read().expect("subscription registry poisoned");
        entries.get(&id).map(|e| SubscriptionState {
            id: e.id,
            queue_depth: e.queue_depth.load(Ordering::Relaxed),
            observation_count: e.observation_count.load(Ordering::Relaxed),
            panic_count: e.panic_count.load(Ordering::Relaxed),
            lagged: e.lagged.load(Ordering::Relaxed),
        })
    }

    pub fn count(&self) -> usize {
        self.entries.read().expect("subscription registry poisoned").len()
    }
}

// ── Dispatch pool (Spec 8 §6.C.2) ─────────────────────────────────

/// One unit of work for a worker thread. Carries the subscription
/// entry (Arc-cloned) and the node to deliver. Cloning the node here
/// decouples the dispatch lifetime from the fabric's lock scope.
struct DispatchTask {
    entry: Arc<SubscriptionEntry>,
    node: IntentNode,
    /// Whether this is a retry attempt (for `RetryRequested`).
    retry: bool,
}

/// Thread pool that runs subscription callbacks in isolation.
pub struct DispatchPool {
    /// Sender held in `Option` so `Drop` can close the channel,
    /// signalling workers to exit.
    sender: Mutex<Option<Sender<DispatchTask>>>,
    workers: Mutex<Vec<JoinHandle<()>>>,
}

/// Tunable knobs for the dispatch pool and backpressure.
#[derive(Debug, Clone, Copy)]
pub struct DispatchConfig {
    /// Number of worker threads. Default 8 per Spec 8 §2.6.1.
    pub worker_count: usize,
    /// Threshold beyond which a subscriber is marked `Lagged`.
    /// Default 1,000 per Spec 8 §6.B.4.
    pub lagged_threshold: usize,
    /// Backoff before a `RetryRequested` task is re-dispatched.
    /// Default 100ms per Spec 8 §6.B.2.
    pub retry_backoff: Duration,
}

impl Default for DispatchConfig {
    fn default() -> Self {
        Self {
            worker_count: 8,
            lagged_threshold: 1_000,
            retry_backoff: Duration::from_millis(100),
        }
    }
}

impl DispatchPool {
    pub fn new(config: DispatchConfig) -> Self {
        let (tx, rx) = mpsc::channel::<DispatchTask>();
        let rx = Arc::new(Mutex::new(rx));
        let mut workers = Vec::with_capacity(config.worker_count);

        for worker_idx in 0..config.worker_count {
            let rx = Arc::clone(&rx);
            let retry_backoff = config.retry_backoff;
            let handle = thread::Builder::new()
                .name(format!("ecphory-dispatch-{}", worker_idx))
                .spawn(move || worker_loop(rx, retry_backoff))
                .expect("failed to spawn dispatch worker");
            workers.push(handle);
        }

        Self {
            sender: Mutex::new(Some(tx)),
            workers: Mutex::new(workers),
        }
    }

    /// Try to enqueue a callback invocation. Returns `false` if the pool
    /// is shut down or the channel is otherwise unavailable.
    pub fn enqueue(&self, entry: Arc<SubscriptionEntry>, node: IntentNode) -> bool {
        let guard = self.sender.lock().expect("dispatch sender poisoned");
        let sender = match guard.as_ref() {
            Some(s) => s,
            None => return false,
        };
        entry.queue_depth.fetch_add(1, Ordering::Relaxed);
        match sender.send(DispatchTask { entry, node, retry: false }) {
            Ok(()) => true,
            Err(mpsc::SendError(failed)) => {
                // Channel closed mid-send. Roll back the queue-depth
                // increment so observers see consistent state.
                failed.entry.queue_depth.fetch_sub(1, Ordering::Relaxed);
                false
            }
        }
    }

    /// Cleanly stop the worker pool. Closes the sender so receiver
    /// `recv()` returns `Err`; workers exit; we join them.
    pub fn shutdown(&self) {
        // Drop the sender so workers' recv() returns Err.
        {
            let mut guard = self.sender.lock().expect("dispatch sender poisoned");
            guard.take();
        }
        let mut workers = self.workers.lock().expect("dispatch workers poisoned");
        for handle in workers.drain(..) {
            // Best-effort join; ignore any panic on the worker.
            let _ = handle.join();
        }
    }
}

impl Drop for DispatchPool {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn worker_loop(rx: Arc<Mutex<Receiver<DispatchTask>>>, retry_backoff: Duration) {
    loop {
        // Acquire the receiver briefly; releasing it lets siblings
        // pull while we run the callback.
        let next = {
            let guard = rx.lock().expect("dispatch receiver poisoned");
            guard.recv()
        };
        let task = match next {
            Ok(t) => t,
            Err(_) => return, // channel closed, exit cleanly
        };
        run_task(task, &rx, retry_backoff);
    }
}

fn run_task(task: DispatchTask, rx: &Arc<Mutex<Receiver<DispatchTask>>>, retry_backoff: Duration) {
    let DispatchTask { entry, node, retry } = task;

    // Build the observer context for the callback.
    let observation_count = entry.observation_count.fetch_add(1, Ordering::Relaxed) + 1;
    let context = ObserverContext {
        subscription_id: entry.id,
        observation_count,
    };

    // Per Spec 8 §2.6.1: catch panics at the callback boundary so they
    // can't propagate into Nabu's request handlers.
    let started = std::time::Instant::now();
    let invoke_result = {
        let cb = Arc::clone(&entry.callback);
        let node_ref = &node;
        let ctx = context.clone();
        catch_unwind(AssertUnwindSafe(move || cb(node_ref, ctx)))
    };
    histogram!(METRIC_FABRIC_SUBSCRIPTION_CALLBACK_LATENCY_SECONDS)
        .record(started.elapsed().as_secs_f64());

    let result = match invoke_result {
        Ok(r) => r,
        Err(panic_payload) => {
            let panic_message = panic_message(&panic_payload);
            entry.panic_count.fetch_add(1, Ordering::Relaxed);
            counter!(
                METRIC_FABRIC_PANIC_TOTAL,
                "location" => "subscription_callback",
            )
            .increment(1);
            eprintln!(
                "[ecphory::subscription] subscription_panic id={} panic_message={:?}",
                entry.id, panic_message
            );
            // Spec 8 §13 self-witnessing: when a panic reporter is
            // wired in (via `BridgeFabric`), surface this panic as a
            // fabric node observable by the immune system. Best-effort:
            // a panic in the reporter itself is caught.
            if let Some(reporter) = entry
                .panic_reporter
                .lock()
                .ok()
                .and_then(|guard| guard.clone())
            {
                let event = SubscriptionPanicEvent {
                    subscription_id: entry.id,
                    panic_message: panic_message.clone(),
                };
                let _ = catch_unwind(AssertUnwindSafe(move || reporter(event)));
            }
            CallbackResult::UnrecoverableError(panic_message)
        }
    };

    entry.queue_depth.fetch_sub(1, Ordering::Relaxed);
    if entry.queue_depth.load(Ordering::Relaxed) == 0 {
        entry.lagged.store(false, Ordering::Relaxed);
    }

    match result {
        CallbackResult::Success => {}
        CallbackResult::RetryRequested if !retry => {
            // Single retry per Spec 8 §6.B.2. Sleep on the worker —
            // sibling workers continue processing other tasks.
            thread::sleep(retry_backoff);
            // Re-acquire the receiver to re-enqueue. Direct to the
            // local worker is fine; we just re-execute the task.
            let _ = rx; // keep arc alive
            entry.queue_depth.fetch_add(1, Ordering::Relaxed);
            run_task(
                DispatchTask {
                    entry: Arc::clone(&entry),
                    node,
                    retry: true,
                },
                rx,
                retry_backoff,
            );
        }
        CallbackResult::RetryRequested => {
            eprintln!(
                "[ecphory::subscription] subscription_retry_exhausted id={}",
                entry.id
            );
        }
        CallbackResult::UnrecoverableError(msg) => {
            eprintln!(
                "[ecphory::subscription] subscription_error id={} reason={:?}",
                entry.id, msg
            );
        }
    }
}

fn panic_message(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "<unknown panic payload>".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    fn make_entry(matcher: Predicate, cb: Callback) -> Arc<SubscriptionEntry> {
        Arc::new(SubscriptionEntry::new(SubscriptionId(1), matcher, cb))
    }

    fn fresh_node() -> IntentNode {
        IntentNode::new("test")
    }

    #[test]
    fn registry_assigns_unique_ids() {
        let reg = SubscriptionRegistry::new(1024);
        let pat: Predicate = Arc::new(|_| true);
        let cb: Callback = Arc::new(|_, _| CallbackResult::Success);
        let a = reg.add(Arc::clone(&pat), Arc::clone(&cb)).unwrap();
        let b = reg.add(pat, cb).unwrap();
        assert_ne!(a, b);
        assert_eq!(reg.count(), 2);
    }

    #[test]
    fn registry_remove_works() {
        let reg = SubscriptionRegistry::new(1024);
        let pat: Predicate = Arc::new(|_| true);
        let cb: Callback = Arc::new(|_, _| CallbackResult::Success);
        let id = reg.add(pat, cb).unwrap();
        assert_eq!(reg.count(), 1);
        reg.remove(id).unwrap();
        assert_eq!(reg.count(), 0);
        // Removing a non-existent subscription is an error.
        assert!(matches!(
            reg.remove(id),
            Err(SubscribeError::UnknownSubscription(_))
        ));
    }

    #[test]
    fn registry_limit_is_enforced() {
        let reg = SubscriptionRegistry::new(2);
        let pat: Predicate = Arc::new(|_| true);
        let cb: Callback = Arc::new(|_, _| CallbackResult::Success);
        let _ = reg.add(Arc::clone(&pat), Arc::clone(&cb)).unwrap();
        let _ = reg.add(Arc::clone(&pat), Arc::clone(&cb)).unwrap();
        let result = reg.add(pat, cb);
        assert_eq!(result.unwrap_err(), SubscribeError::SubscriptionLimitExceeded);
    }

    #[test]
    fn pool_runs_callback_off_request_path() {
        let counter = Arc::new(AtomicUsize::new(0));
        let counter_for_cb = Arc::clone(&counter);
        let cb: Callback = Arc::new(move |_node, _ctx| {
            counter_for_cb.fetch_add(1, Ordering::SeqCst);
            CallbackResult::Success
        });
        let pat: Predicate = Arc::new(|_| true);
        let entry = make_entry(pat, cb);

        let pool = DispatchPool::new(DispatchConfig {
            worker_count: 2,
            ..DispatchConfig::default()
        });

        for _ in 0..5 {
            assert!(pool.enqueue(Arc::clone(&entry), fresh_node()));
        }

        // Spin-wait for callbacks to complete (≤ 2s).
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while counter.load(Ordering::SeqCst) < 5 && std::time::Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(counter.load(Ordering::SeqCst), 5,
            "All 5 callbacks must run on the dispatch pool.");
        assert_eq!(entry.observation_count.load(Ordering::SeqCst), 5);
        pool.shutdown();
    }

    #[test]
    fn callback_panic_is_isolated() {
        // A panicking callback must not bring down the worker thread or
        // bubble into the request path. The next callback must still run.
        let panicked_then_normal = Arc::new(AtomicUsize::new(0));
        let counter_for_cb = Arc::clone(&panicked_then_normal);
        let cb: Callback = Arc::new(move |node, _ctx| {
            counter_for_cb.fetch_add(1, Ordering::SeqCst);
            if node.want.description.contains("BOOM") {
                panic!("intentional panic from test");
            }
            CallbackResult::Success
        });
        let pat: Predicate = Arc::new(|_| true);
        let entry = make_entry(pat, cb);

        let pool = DispatchPool::new(DispatchConfig {
            worker_count: 1, // single worker — panic must not kill it
            ..DispatchConfig::default()
        });

        pool.enqueue(Arc::clone(&entry), {
            let mut n = fresh_node();
            n.want.description = "BOOM".into();
            n
        });
        pool.enqueue(Arc::clone(&entry), fresh_node());

        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while panicked_then_normal.load(Ordering::SeqCst) < 2
            && std::time::Instant::now() < deadline
        {
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(panicked_then_normal.load(Ordering::SeqCst), 2,
            "Worker must survive the panic and process the next task.");
        assert_eq!(entry.panic_count.load(Ordering::SeqCst), 1);
        pool.shutdown();
    }

    #[test]
    fn slow_callback_does_not_block_other_workers() {
        // Worker A holds a slow callback; Worker B must still be able
        // to drain a fast callback off the same channel.
        let fast_done = Arc::new(AtomicUsize::new(0));
        let slow_started = Arc::new(AtomicUsize::new(0));

        let fast_done_for_cb = Arc::clone(&fast_done);
        let slow_started_for_cb = Arc::clone(&slow_started);

        let slow_cb: Callback = Arc::new(move |_n, _c| {
            slow_started_for_cb.fetch_add(1, Ordering::SeqCst);
            thread::sleep(Duration::from_millis(200));
            CallbackResult::Success
        });
        let fast_cb: Callback = Arc::new(move |_n, _c| {
            fast_done_for_cb.fetch_add(1, Ordering::SeqCst);
            CallbackResult::Success
        });

        let pat: Predicate = Arc::new(|_| true);
        let slow_entry = make_entry(Arc::clone(&pat), slow_cb);
        let fast_entry = make_entry(pat, fast_cb);

        let pool = DispatchPool::new(DispatchConfig {
            worker_count: 2,
            ..DispatchConfig::default()
        });

        pool.enqueue(slow_entry, fresh_node());
        // Wait for slow to start so we know one worker is occupied.
        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        while slow_started.load(Ordering::SeqCst) == 0
            && std::time::Instant::now() < deadline
        {
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(slow_started.load(Ordering::SeqCst), 1);

        // Now enqueue the fast callback — it must run on the other worker.
        pool.enqueue(fast_entry, fresh_node());
        let deadline = std::time::Instant::now() + Duration::from_millis(150);
        while fast_done.load(Ordering::SeqCst) == 0
            && std::time::Instant::now() < deadline
        {
            thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(fast_done.load(Ordering::SeqCst), 1,
            "Fast callback must run on the second worker before slow finishes (200ms).");
        pool.shutdown();
    }

    #[test]
    fn retry_requested_runs_callback_twice() {
        let count = Arc::new(AtomicUsize::new(0));
        let count_for_cb = Arc::clone(&count);
        let cb: Callback = Arc::new(move |_, _| {
            let n = count_for_cb.fetch_add(1, Ordering::SeqCst);
            // First invocation requests retry; second succeeds.
            if n == 0 {
                CallbackResult::RetryRequested
            } else {
                CallbackResult::Success
            }
        });
        let pat: Predicate = Arc::new(|_| true);
        let entry = make_entry(pat, cb);
        let pool = DispatchPool::new(DispatchConfig {
            worker_count: 1,
            retry_backoff: Duration::from_millis(20),
            ..DispatchConfig::default()
        });

        pool.enqueue(Arc::clone(&entry), fresh_node());
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while count.load(Ordering::SeqCst) < 2 && std::time::Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(count.load(Ordering::SeqCst), 2,
            "Spec 8 §6.B.2: RetryRequested triggers a single retry.");
        pool.shutdown();
    }
}
