use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::hash::Hash;
use std::sync::Arc;

use crate::time::Clock;
use crate::time::SystemClock;

/// Tracks events that you don't want to repeat very often, and lets you know
/// when it's safe to take the action that may cause those events.
pub struct BackoffTracker<T> {
    field: HashMap<T, BackoffEntry>,
    clock: Arc<dyn Clock>,
    config: BackoffConfig,
}

impl<T> Default for BackoffTracker<T> {
    fn default() -> Self {
        Self::new(Arc::new(SystemClock::default()), BackoffConfig::default())
    }
}

impl<T> BackoffTracker<T> {
    pub fn new(clock: Arc<dyn Clock>, config: BackoffConfig) -> Self {
        Self {
            field: HashMap::new(),
            clock,
            config,
        }
    }
}

/// High level management of code execution using backoff tracking primitives.
impl<'t, T: Eq + Hash + 't> BackoffTracker<T> {
    /// Use this if you'd like to rate limit some code regardless of its outcome.
    ///
    /// Triggers an event on any execution. see `backoff_generic`.
    pub fn backoff<F: Fn() -> X, X>(&mut self, id: T, f: F) -> Option<X> {
        self.backoff_generic(id, f, false, |_| true)
    }

    /// Use this if you'd like to rate limit some code regardless of its
    /// outcome, but not if other events occur in between.
    ///
    /// Triggers an event on any execution. see `backoff_generic` and
    /// `singular_event`
    pub fn backoff_singular<F: Fn() -> X, X>(&mut self, id: T, f: F) -> Option<X> {
        self.backoff_generic(id, f, true, |_| true)
    }

    /// Use this if you'd like to rate limit some code when it fails.
    /// Triggers an event on Err. see `backoff_generic`.
    pub fn backoff_errors<F, O, E>(&mut self, id: T, f: F) -> Option<Result<O, E>>
    where
        F: Fn() -> Result<O, E>,
    {
        self.backoff_generic(id, f, false, Result::is_err)
    }

    /// Use this if you'd like to rate limit some code when it succeeds.
    /// Triggers an event on Ok. see `backoff_generic`.
    pub fn backoff_oks<F, O, E>(&mut self, id: T, f: F) -> Option<Result<O, E>>
    where
        F: Fn() -> Result<O, E>,
    {
        self.backoff_generic(id, f, false, Result::is_ok)
    }

    /// Generic function used by other `backoff_` functions.
    ///
    /// Run the code if ready. Trigger an event when the predicate is true
    /// Returns None if the execution was skipped due to not being ready.
    pub fn backoff_generic<F, X, P>(
        &mut self,
        id: T,
        f: F,
        singular: bool,
        predicate: P,
    ) -> Option<X>
    where
        F: Fn() -> X,
        P: Fn(&X) -> bool,
    {
        if self.is_ready(&id) {
            let result = f();
            if predicate(&result) {
                if singular && !self.field.contains_key(&id) {
                    self.field = HashMap::new();
                }
                self.event(id);
            }
            Some(result)
        } else {
            None
        }
    }
}

/// Primitives to track the backoff
impl<'t, T: Eq + Hash + 't> BackoffTracker<T> {
    pub fn event(&mut self, item: T) {
        let now = self.clock.current_timestamp();
        match self.field.entry(item) {
            Entry::Vacant(entry) => {
                entry.insert(BackoffEntry {
                    count: 1,
                    last: now,
                    delay: self.config.initial_delay(),
                });
            }
            Entry::Occupied(mut entry) => {
                let BackoffEntry { count, last, delay } = entry.get();
                if last + delay < now {
                    let delay = match self.config.strategy {
                        BackoffStrategy::Exponential(b) => {
                            std::cmp::min(delay * b, self.config.max_delay)
                        }
                        BackoffStrategy::Cliff(c) => {
                            if count > &c {
                                self.config.max_delay
                            } else {
                                self.config.min_delay
                            }
                        }
                    };
                    entry.insert(BackoffEntry {
                        count: count + 1,
                        last: now,
                        delay,
                    });
                }
            }
        }
    }

    /// Like `event`, but it clears all history of other events when a new event
    /// is received.
    ///
    /// Use this when you only need to backoff events that are consecutive
    /// duplicates, but don't need to backoff events if different events occur
    /// in between.
    pub fn singular_event(&mut self, item: T) {
        if !self.field.contains_key(&item) {
            self.field = HashMap::new();
        }
        self.event(item);
    }

    pub fn clear(&mut self, item: &T) {
        if self.field.contains_key(item) {
            self.field.remove(item);
        }
    }

    pub fn clear_many<'a>(&mut self, items: impl IntoIterator<Item = &'a T>)
    where
        't: 'a,
    {
        for item in items {
            self.field.remove(item);
        }
    }

    pub fn is_ready(&self, item: &T) -> bool {
        match self.field.get(item) {
            Some(x) => self.clock.current_timestamp() > x.last + x.delay,
            None => true,
        }
    }
}

/// Events must not be recorded here if they occurred...
/// - within a delay window
/// - before the last `clear`
struct BackoffEntry {
    /// Total quantity of registered events that occurred outside a delay window.
    count: u64,
    /// The time that the last event occurred outside a delay window.
    last: u64,
    /// The time window to wait since last_event before being "ready"
    delay: u64,
}

pub struct BackoffConfig {
    /// The minimum amount of time to wait before trying again.
    pub min_delay: u64,
    /// The maximum amount of time to wait before trying again.
    pub max_delay: u64,
    /// How to progress from min_delay to max_delay.
    pub strategy: BackoffStrategy,
}

impl BackoffConfig {
    /// Simple rate limiter with a constant rate, doesn't backoff progressively.
    pub fn constant(period: u64) -> Self {
        Self {
            min_delay: period,
            max_delay: period,
            strategy: BackoffStrategy::Cliff(0),
        }
    }

    /// Once an event occurs, it will never be ready again unless cleared.
    pub fn once() -> Self {
        Self {
            min_delay: u64::MAX,
            max_delay: u64::MAX,
            strategy: BackoffStrategy::Cliff(0),
        }
    }

    pub fn initial_delay(&self) -> u64 {
        match self.strategy {
            BackoffStrategy::Cliff(0) => self.max_delay,
            _ => self.min_delay,
        }
    }
}

impl Default for BackoffConfig {
    fn default() -> Self {
        Self {
            min_delay: 30,
            max_delay: 3600,
            strategy: Default::default(),
        }
    }
}

pub enum BackoffStrategy {
    /// The wrapped integer is the base b of the exponential b^n where n is the
    /// number of events. Each event will trigger a delay b times as large as
    /// the prior delay.
    Exponential(u64),

    /// Cliff means you wait exactly the minimum delay for some number of
    /// events, then you suddenly switch to waiting the maximum interval for
    /// each event. The wrapped integer is the number of times to delay with the
    /// minimum interval before switching to the max interval.
    Cliff(u64),
}

impl Default for BackoffStrategy {
    fn default() -> Self {
        // 3 is the closest integer to Euler's constant, which makes it the
        // obvious choice as a standard base for an exponential. It also
        // subjectively feels to me like a generally satisfying rate of cooldown
        // in most cases, where 2 feels overly aggressive.
        BackoffStrategy::Exponential(3)
    }
}
