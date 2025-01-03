//! 定时器
//!
//!

use concurrent_queue::ConcurrentQueue;
use std::collections::BTreeMap;
use std::mem;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

const TIMER_QUEUE_SIZE: usize = 1024;

// 定时器回调
type TimerCb = Box<dyn Fn() -> ()>;
/// A single timer operation.
enum TimerOp {
    Insert(Instant, usize, TimerCb),
    Remove(Instant, usize),
}

pub struct TimerManager {
    /// An ordered map of registered timers.
    ///
    /// Timers are in the order in which they fire. The `usize` in this type is a timer ID used to
    /// distinguish timers that fire at the same time. The `Waker` represents the task awaiting the
    /// timer.
    timers: Mutex<BTreeMap<(Instant, usize), TimerCb>>,

    /// A queue of timer operations (insert and remove).
    ///
    /// When inserting or removing a timer, we don't process it immediately - we just push it into
    /// this queue. Timers actually get processed when the queue fills up or the reactor is polled.
    timer_ops: ConcurrentQueue<TimerOp>,
}

impl TimerManager {
    pub fn new() -> Self {
        Self {
            timers: Mutex::new(BTreeMap::new()),
            timer_ops: ConcurrentQueue::bounded(TIMER_QUEUE_SIZE),
        }
    }
    /// Registers a timer in the reactor.
    ///
    /// Returns the inserted timer's ID.
    pub fn insert_timer(&self, timer_id: usize, when: Instant, waker: TimerCb) -> usize {
        // Generate a new timer ID.
        // static ID_GENERATOR: AtomicUsize = AtomicUsize::new(1);
        // let id = ID_GENERATOR.fetch_add(1, Ordering::Relaxed);

        // Push an insert operation.
        let mut value = TimerOp::Insert(when, timer_id, waker);
        let mut value = self.timer_ops.push(value);
        while value.is_err() {
            // If the queue is full, drain it and try again.
            let mut timers = self.timers.lock().unwrap();
            self.process_timer_ops(&mut timers);
        }

        // Notify that a timer has been inserted.
        // self.notify();

        timer_id
    }

    /// Deregisters a timer from the reactor.
    pub fn remove_timer(&self, when: Instant, id: usize) {
        // Push a remove operation.
        while self.timer_ops.push(TimerOp::Remove(when, id)).is_err() {
            // If the queue is full, drain it and try again.
            let mut timers = self.timers.lock().unwrap();
            self.process_timer_ops(&mut timers);
        }
    }

    /// Processes ready timers and extends the list of wakers to wake.
    ///
    /// Returns the duration until the next timer before this method was called.
    pub fn process_timers(&self, wakers: &mut Vec<TimerCb>) -> Option<Duration> {
        let span = tracing::trace_span!("process_timers");
        let _enter = span.enter();

        let mut timers = self.timers.lock().unwrap();
        self.process_timer_ops(&mut timers);

        let now = Instant::now();

        // Split timers into ready and pending timers.
        //
        // Careful to split just *after* `now`, so that a timer set for exactly `now` is considered
        // ready.
        let pending = timers.split_off(&(now + Duration::from_nanos(1), 0));
        let ready = mem::replace(&mut *timers, pending);

        // Calculate the duration until the next event.
        let dur = if ready.is_empty() {
            // Duration until the next timer.
            timers
                .keys()
                .next()
                .map(|(when, _)| when.saturating_duration_since(now))
        } else {
            // Timers are about to fire right now.
            Some(Duration::from_secs(0))
        };

        // Drop the lock before waking.
        drop(timers);

        // Add wakers to the list.
        tracing::trace!("{} ready wakers", ready.len());

        for (_, waker) in ready {
            wakers.push(waker);
        }

        dur
    }

    /// Processes queued timer operations.
    fn process_timer_ops(&self, timers: &mut MutexGuard<'_, BTreeMap<(Instant, usize), TimerCb>>) {
        // Process only as much as fits into the queue, or else this loop could in theory run
        // forever.
        self.timer_ops
            .try_iter()
            .take(self.timer_ops.capacity().unwrap())
            .for_each(|op| match op {
                TimerOp::Insert(when, id, waker) => {
                    timers.insert((when, id), waker);
                }
                TimerOp::Remove(when, id) => {
                    timers.remove(&(when, id));
                }
            });
    }
}
