//! 定时器
//!
//!

use concurrent_queue::ConcurrentQueue;
use rustc_hash::FxHashMap;
use std::collections::BTreeMap;
use std::mem;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

const TIMER_QUEUE_SIZE: usize = 1024;

// 定时器回调
type TimerCb = Box<dyn Fn() -> ()>;
/// A single timer operation.
enum TimerOp {
    Insert(Instant, usize, Duration, TimerCb),
    Remove(usize),
}

pub struct TimerManager {
    /// An ordered map of registered timers.
    ///
    /// Timers are in the order in which they fire. The `usize` in this type is a timer ID used to
    /// distinguish timers that fire at the same time. The `Waker` represents the task awaiting the
    /// timer.
    timers: Mutex<BTreeMap<(Instant, usize, Duration), TimerCb>>,

    /// A queue of timer operations (insert and remove).
    ///
    /// When inserting or removing a timer, we don't process it immediately - we just push it into
    /// this queue. Timers actually get processed when the queue fills up or the reactor is polled.
    timer_ops: ConcurrentQueue<TimerOp>,

    timer_infos: Mutex<FxHashMap<usize, (Instant, Duration)>>,
}

impl TimerManager {
    pub fn new() -> Self {
        Self {
            timers: Mutex::new(BTreeMap::new()),
            timer_ops: ConcurrentQueue::bounded(TIMER_QUEUE_SIZE),
            timer_infos: Mutex::new(FxHashMap::default()),
        }
    }
    /// Registers a timer in the reactor.
    ///
    /// Returns the inserted timer's ID.
    pub fn insert_timer(&self, interval: Duration, waker: TimerCb, recurrent: bool) -> usize {
        // Generate a new timer ID.
        static ID_GENERATOR: AtomicUsize = AtomicUsize::new(1);
        let timer_id = ID_GENERATOR.fetch_add(1, Ordering::Relaxed);
        let when = Instant::now() + interval;
        let mut interval_val = Duration::ZERO;
        if recurrent {
            interval_val = interval;
        }
        // Push an insert operation.
        let value = TimerOp::Insert(when, timer_id, interval_val, waker);
        let value = self.timer_ops.push(value);
        while value.is_err() {
            // If the queue is full, drain it and try again.
            let mut timers = self.timers.lock().unwrap();
            let mut timer_infos = self.timer_infos.lock().unwrap();
            self.process_timer_ops(&mut timers, &mut timer_infos);
        }

        // Notify that a timer has been inserted.
        // self.notify();

        timer_id
    }

    /// Deregisters a timer from the reactor.
    pub fn remove_timer(&self, id: usize) {
        // Push a remove operation.
        while self.timer_ops.push(TimerOp::Remove(id)).is_err() {
            // If the queue is full, drain it and try again.
            let mut timers = self.timers.lock().unwrap();
            let mut timer_infos = self.timer_infos.lock().unwrap();
            self.process_timer_ops(&mut timers, &mut timer_infos);
        }
    }

    /// Processes ready timers and extends the list of wakers to wake.
    ///
    /// Returns the duration until the next timer before this method was called.
    pub fn process_timers(
        &self,
        wakers: &mut Vec<(Instant, usize, Duration, TimerCb)>,
    ) -> Option<Duration> {
        let span = tracing::trace_span!("process_timers");
        let _enter = span.enter();

        let mut timers = self.timers.lock().unwrap();
        let mut timer_infos = self.timer_infos.lock().unwrap();
        self.process_timer_ops(&mut timers, &mut timer_infos);

        let now = Instant::now();

        // Split timers into ready and pending timers.
        //
        // Careful to split just *after* `now`, so that a timer set for exactly `now` is considered
        // ready.
        let pending = timers.split_off(&(now + Duration::from_nanos(1), 0, Duration::ZERO));
        let ready = mem::replace(&mut *timers, pending);

        // Calculate the duration until the next event.
        let dur = if ready.is_empty() {
            // Duration until the next timer.
            timers
                .keys()
                .next()
                .map(|(when, _, _)| when.saturating_duration_since(now))
        } else {
            // Timers are about to fire right now.
            Some(Duration::from_secs(0))
        };

        for ((_, timer_id, _), _) in &ready {
            timer_infos.remove(timer_id);
        }

        // Drop the lock before waking.
        drop(timers);
        drop(timer_infos);

        // Add wakers to the list.
        tracing::trace!("{} ready wakers", ready.len());

        for ((when, timer_id, interval), waker) in ready {
            wakers.push((when, timer_id, interval, waker));
        }

        dur
    }

    /// Processes queued timer operations.
    fn process_timer_ops(
        &self,
        timers: &mut MutexGuard<'_, BTreeMap<(Instant, usize, Duration), TimerCb>>,
        timer_infos: &mut MutexGuard<'_, FxHashMap<usize, (Instant, Duration)>>,
    ) {
        // Process only as much as fits into the queue, or else this loop could in theory run
        // forever.
        self.timer_ops
            .try_iter()
            .take(self.timer_ops.capacity().unwrap())
            .for_each(|op| match op {
                TimerOp::Insert(when, id, interval, waker) => {
                    timers.insert((when, id, interval), waker);
                    timer_infos.insert(id, (when, interval));
                }
                TimerOp::Remove(id) => {
                    if let Some((when, interval)) = timer_infos.remove(&id) {
                        timers.remove(&(when, id, interval));
                    }
                }
            });
    }

    /// Registers a timer in the reactor.
    ///
    /// Returns the inserted timer's ID.
    pub fn repeat_insert_timer(
        &self,
        timer_id: usize,
        interval: Duration,
        waker: TimerCb,
        recurrent: bool,
    ) -> usize {
        let when = Instant::now() + interval;
        let mut interval_val = Duration::ZERO;
        if recurrent {
            interval_val = interval;
        }
        // Push an insert operation.
        let value = TimerOp::Insert(when, timer_id, interval_val, waker);
        let value = self.timer_ops.push(value);
        while value.is_err() {
            // If the queue is full, drain it and try again.
            let mut timers = self.timers.lock().unwrap();
            let mut timer_infos = self.timer_infos.lock().unwrap();
            self.process_timer_ops(&mut timers, &mut timer_infos);
        }

        // Notify that a timer has been inserted.
        // self.notify();

        timer_id
    }

    pub fn update(&self) {
        let mut cbs = Vec::new();
        self.process_timers(&mut cbs);
        for timer in cbs {
            timer.3();
            // recurrent
            if !timer.2.is_zero() {
                self.repeat_insert_timer(timer.1, timer.2, timer.3, true);
            }
        }
    }
}
