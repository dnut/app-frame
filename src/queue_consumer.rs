use std::{
    collections::HashMap,
    hash::Hash,
    marker::PhantomData,
    sync::Arc,
    time::{Duration, SystemTime},
};

use anyhow::bail;
use async_trait::async_trait;
use futures::future::join_all;
use ordered_float::OrderedFloat;
use parking_lot::{Mutex, RwLock};
use priority_queue::PriorityQueue;
use std::fmt::Debug;
use tokio::time::timeout;

use crate::{backoff::BackoffTracker, clone_to_async, error::LogError, short_name, time::Sleeper};

use super::service::MutJob;

// TODO: generalize this. a trait should be defined here, with no dependency on a specific type of queue.
pub type Queue<T> = Arc<RwLock<PriorityQueue<T, OrderedFloat<f64>>>>;

pub trait Receiver<T: Hash + Eq> {
    fn receiver(&self, config: QueueReceiveConfig, sleeper: Arc<dyn Sleeper>) -> QueueReceiver<T>;
}

impl<T: Hash + Eq + Clone> Receiver<T> for Queue<T> {
    fn receiver(&self, config: QueueReceiveConfig, sleeper: Arc<dyn Sleeper>) -> QueueReceiver<T> {
        QueueReceiver::new(self.clone(), config, sleeper)
    }
}

pub struct QueueReceiver<T: Hash + Eq> {
    queue: Queue<T>,
    /// History of last time each item was seen
    last_appearances: HashMap<T, SystemTime>,
    config: QueueReceiveConfig,
    sleeper: Arc<dyn Sleeper>,
}

// TODO: find a way to avoid Clone requirement
impl<T: Hash + Eq + Clone> QueueReceiver<T> {
    pub fn new(queue: Queue<T>, config: QueueReceiveConfig, sleeper: Arc<dyn Sleeper>) -> Self {
        Self {
            queue,
            config,
            last_appearances: HashMap::new(),
            sleeper,
        }
    }

    pub fn len(&self) -> usize {
        self.queue.read().len()
    }

    pub fn is_empty(&self) -> bool {
        self.queue.read().is_empty()
    }

    pub async fn recv(&mut self) -> (Vec<T>, usize) {
        self.recv_with(&[]).await
    }

    pub fn try_recv(&mut self) -> (Vec<T>, usize) {
        self.try_recv_with(&[])
    }

    /// Waits until some are available, then returns as many as possible, up to
    /// max_chunk_size.
    /// usize is the number remaining in the queue.
    pub async fn recv_with(&mut self, overrides: &[Override]) -> (Vec<T>, usize) {
        let config = self.config.with(overrides);
        let start = SystemTime::now();
        loop {
            if self.queue.read().len() < config.min_chunk_size {
                self.sleeper.sleep(config.poll_interval).await;
            } else {
                let x = self.try_recv_with(overrides);
                if !x.0.is_empty() {
                    return x;
                }
            }
            if let Some(max_wait) = config.max_wait {
                if SystemTime::now().duration_since(start).unwrap() > max_wait {
                    return (vec![], self.queue.read().len());
                }
            }
        }
    }

    /// If any are available, returns them immediately. Otherwise returns an empty vec.
    /// usize is the number remaining in the queue.
    pub fn try_recv_with(&mut self, overrides: &[Override]) -> (Vec<T>, usize) {
        let mut reader = self.queue.write();
        let max_return = self
            .config
            .with(overrides)
            .max_chunk_size
            .unwrap_or(usize::MAX);
        let actual_return = std::cmp::min(max_return, reader.len());
        let mut to_process = Vec::with_capacity(actual_return);

        while to_process.len() < max_return {
            match reader.pop() {
                Some((item, _)) => {
                    if let Some(cooldown) = self.config.with(overrides).cooldown {
                        let now = SystemTime::now();
                        let last = self
                            .last_appearances
                            .get(&item)
                            .unwrap_or(&SystemTime::UNIX_EPOCH);
                        if now.duration_since(*last).unwrap() < cooldown {
                            continue;
                        };
                        self.last_appearances.insert(item.clone(), now);
                    }
                    to_process.push(item);
                }
                None => break,
            }
        }
        (to_process, reader.len())
    }
}

#[derive(Clone, Copy, Debug)]
pub struct QueueReceiveConfig {
    /// How long to wait between polling attempts when the queue is empty
    pub poll_interval: Duration,
    /// The longest period to wait before returning an empty vec. None means wait forever
    pub max_wait: Option<Duration>,
    /// The largest number of items to return per method call. None means no limit.
    pub max_chunk_size: Option<usize>,
    ///
    pub min_chunk_size: usize,
    /// If an identical item is read from the queue multiple times within this
    /// many seconds, only the first one is used. None disables deduplication.
    pub cooldown: Option<Duration>,
    /// How to behave when the number of items in the queue is between 0 and
    /// max_chunk_size.
    pub batch_strategy: BatchStrategy,
}

impl Default for QueueReceiveConfig {
    fn default() -> Self {
        Self {
            poll_interval: Duration::from_secs(1),
            max_wait: None, // TODO: is this sane?
            max_chunk_size: None,
            min_chunk_size: 1,
            cooldown: None,
            batch_strategy: BatchStrategy::Responsive,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum BatchStrategy {
    /// React as soon as anything is received
    Responsive,
    /// React only when the number of items received reaches the full chunk size
    Efficient,
}

#[derive(Clone, Copy, Debug)]
pub enum Override {
    PollInterval(Duration),
    MaxWait(Option<Duration>),
    MaxChunkSize(Option<usize>),
    MinChunkSize(usize),
    Cooldown(Option<Duration>),
    BatchStrategy(BatchStrategy),
}

impl QueueReceiveConfig {
    fn with(mut self, settings: &[Override]) -> QueueReceiveConfig {
        for setting in settings {
            match setting {
                Override::PollInterval(x) => self.poll_interval = *x,
                Override::MaxWait(x) => self.max_wait = *x,
                Override::MaxChunkSize(x) => self.max_chunk_size = *x,
                Override::Cooldown(x) => self.cooldown = *x,
                Override::BatchStrategy(x) => self.batch_strategy = *x,
                Override::MinChunkSize(x) => self.min_chunk_size = *x,
            }
        }
        self
    }
}

impl From<QueueReceiveConfig> for Vec<Override> {
    fn from(value: QueueReceiveConfig) -> Self {
        vec![
            Override::PollInterval(value.poll_interval),
            Override::MaxWait(value.max_wait),
            Override::MaxChunkSize(value.max_chunk_size),
            Override::MinChunkSize(value.min_chunk_size),
            Override::Cooldown(value.cooldown),
            Override::BatchStrategy(value.batch_strategy),
        ]
    }
}

#[async_trait]
pub trait BatchProcessor<In, Out = ()> {
    type Intermediate;
    /// Action that does not have a special way of batching, and the caller can
    /// simply executed in parallel on many items at once.
    async fn prepare_item(&self, input: In) -> anyhow::Result<Self::Intermediate>;

    /// Action that does have a special way of batching, and this particular
    /// type can define the batching approach.
    async fn process_batch(&self, mid: Vec<Self::Intermediate>) -> anyhow::Result<Out>;
}

pub struct Dispatch<P, In>
where
    P: BatchProcessor<In>,
    In: Hash + Eq + Clone,
{
    provider: Mutex<QueueReceiver<In>>,
    processor: P,
    timeout: u64,
    backoff: Option<BackoffTracker<In>>,
    _phantom: PhantomData<In>,
}

impl<P: BatchProcessor<In>, In: Hash + Eq + Clone> Dispatch<P, In> {
    pub fn new(
        provider: QueueReceiver<In>,
        processor: P,
        timeout: u64,
        backoff: Option<BackoffTracker<In>>,
    ) -> Self {
        Self {
            provider: Mutex::new(provider),
            processor,
            timeout,
            backoff,
            _phantom: PhantomData,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.provider.lock().is_empty()
    }
}

#[async_trait]
impl<P, In> MutJob for Dispatch<P, In>
where
    P: BatchProcessor<In> + Send + Sync,
    In: Hash + Eq + Clone + Send + Sync + Debug,
    P::Intermediate: Send + Sync,
{
    async fn run_once_mut(&mut self) -> anyhow::Result<()> {
        process_batch(
            &self.provider,
            self.timeout,
            &self.processor,
            self.backoff.as_mut(),
        )
        .await
    }
}

/// Consume some items from the queue, processing and returning the result,
/// logging errors as they occur.
///
/// - name: the name to print in the logs.
/// - timeout_secs: the entire job will be cancelled if it exceeds this time
/// - processor -> defines two operations:
///   - the initial function to apply separately to each individual item.
///   - the final function to apply to the vec of outputs of the first stem.
pub async fn process_batch<T: Hash + Eq + Clone, Ret, Processor: BatchProcessor<T, Ret>>(
    receiver: &Mutex<QueueReceiver<T>>,
    timeout_secs: u64,
    processor: &Processor,
    mut backoff: Option<&mut BackoffTracker<T>>,
) -> anyhow::Result<Ret>
where
    T: std::fmt::Debug,
{
    // TODO: refactor this function into smaller pieces without too much interface complexity
    let input_name = &short_name::<T>();
    let proc_name = &short_name::<Processor>();
    match timeout(Duration::from_secs(timeout_secs), async {
        // collect inputs from the queue
        let (mut items, remaining) = receiver.lock().recv_with(&[]).await;
        if let Some(backoff) = backoff.as_ref() {
            items.retain(|x| backoff.is_ready(x));
        }
        if !items.is_empty() {
            tracing::info!(
                "Processing {} '{input_name}'s with '{proc_name}'. {remaining} remain.",
                items.len()
            );
        } else if remaining > 0 {
            tracing::debug!(
                "Processing {} '{input_name}'s with '{proc_name}'. {remaining} remain.",
                items.len()
            );
        }

        // Execute first step: process individually
        let (oks, errs) = join_all(items.into_iter().map(clone_to_async! { (processor) |item|
            (item.clone(), processor
                .prepare_item(item.clone())
                .await
                .log_with_context_passthrough(|| format!("{proc_name} - preparing {item:?}")))
        }))
        .await
        .into_iter()
        .partition::<Vec<_>, _>(|x| x.1.is_ok());

        // Organize outputs of first step
        let (ok_inputs, ok_intermediates) = oks
            .into_iter()
            .map(|(t, r)| (t, r.unwrap()))
            .unzip::<_, _, Vec<_>, Vec<_>>();
        let (err_inputs, _errors) = errs
            .into_iter()
            .map(|(t, r)| (t, r.err().unwrap()))
            .unzip::<_, _, Vec<_>, Vec<_>>();

        // Execute second step: back processor
        let batch_result = processor
            .process_batch(ok_intermediates)
            .await
            .log_with_context_passthrough(|| format!("'{proc_name}' batch processor"));

        // Handle results with backoff and return result
        if batch_result.is_err() {
            // TODO: process_batch should try to let us know if any inputs
            // succeeded, to make this more selective.
            let all_inputs = ok_inputs
                .into_iter()
                .chain(err_inputs.into_iter())
                .collect::<Vec<_>>();
            for input in all_inputs.clone() {
                if let Some(b) = backoff.as_mut() { b.event(input) }
            }
            bail!("'{proc_name}' failed to process batch inputs, see logs. failed inputs: {all_inputs:?}");
        } else {
            for successful_input in ok_inputs {
                if let Some(b) = backoff.as_mut() { b.clear(&successful_input) }
            }
            for failed_input in err_inputs.clone() {
                if let Some(b) = backoff.as_mut() { b.event(failed_input) }
            }
            if !err_inputs.is_empty() {
                bail!("'{proc_name}' failed to process some inputs, see logs. failed inputs: {err_inputs:?}");
            }
            batch_result
        }
    })
    .await
    {
        Ok(Ok(x)) => Ok(x),
        Ok(Err(e)) => Err(e),
        Err(e) => {
            Err(anyhow::anyhow!(e)).log_context_passthrough(&format!("'{proc_name}' timed out"))
        }
    }
}
