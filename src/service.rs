use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};

use async_trait::async_trait;
use futures::lock::Mutex;
use tokio::time::timeout;

use crate::{
    error::LogError,
    short_name,
    time::{Sleeper, TokioSleeper},
    Never,
};

use super::service_manager::Heartbeat;

#[async_trait]
pub trait Service: Send + Sync {
    /// The service is never expected to stop, it should do its job until
    /// externally shutdown.
    async fn run_forever(&self) -> Never;

    /// The name used to represent the service in the logs
    fn name(&self) -> String {
        short_name::<Self>()
    }

    /// The maximum amount of seconds allowed between heartbeats before the
    /// service is deemed "dead"
    fn heartbeat_ttl(&self) -> i32;

    /// The service should frequently call "beat" to indicate that it is still
    /// alive.
    fn set_heartbeat(&mut self, heartbeat: Arc<dyn Heartbeat + 'static>);

    /// The service should be able to self-diagnose any internal issues and
    /// report if there is a problem.
    fn is_healthy(&self) -> bool;
}

#[async_trait]
impl<J: Job> Job for Vec<J> {
    async fn run_once(&self) -> anyhow::Result<()> {
        for job in self.iter() {
            job.run_once().await?;
        }
        Ok(())
    }
}

const LOOP_SLEEPER: TokioSleeper = TokioSleeper::default();

pub struct LoopingJobService {
    job: Arc<dyn LoopableJob>,
    heartbeat: Arc<dyn Heartbeat>,
    sleeper: Arc<dyn Sleeper>,
    /// Number of consecutive failures that have happened until now without any
    /// successes.  
    /// If the last attempt was a success, this will be 0.
    consecutive_failures: AtomicUsize,
}

impl From<Arc<dyn LoopableJob>> for LoopingJobService {
    fn from(job: Arc<dyn LoopableJob>) -> Self {
        Self {
            job,
            heartbeat: Arc::new(()),
            sleeper: Arc::new(LOOP_SLEEPER),
            consecutive_failures: 0.into(),
        }
    }
}

impl<J: LoopableJob + 'static> From<J> for LoopingJobService {
    fn from(job: J) -> Self {
        Self {
            job: Arc::new(job),
            heartbeat: Arc::new(()),
            sleeper: Arc::new(LOOP_SLEEPER),
            consecutive_failures: 0.into(),
        }
    }
}

impl LoopingJobService {
    pub fn new<J: Job + 'static>(job: J, config: LoopConfig) -> Self {
        Self {
            job: Arc::new((job, config)),
            heartbeat: Arc::new(()),
            sleeper: Arc::new(LOOP_SLEEPER),
            consecutive_failures: 0.into(),
        }
    }
}

/// Use this as the service type instead of LoopingJobService for mutable jobs
/// that need to be implemented as MutJob instead of Job.
///
/// Ideally, LoopingJobService could be used for both Job and MutJob, but it is
/// not possible to write multiple sufficiently generic implementations of
/// From<> for LoopingJobService due to the lack of specialization and negative
/// trait bounds.
pub struct LoopingMutJobService(LoopingJobService);

impl<Mj: MutJob + SelfConfiguredLoop + Send + Sync + 'static> From<Mj> for LoopingMutJobService {
    fn from(value: Mj) -> Self {
        let config = value.loop_config();
        Self(LoopingJobService::from((Mutex::new(value), config)))
    }
}

#[async_trait]
impl Service for LoopingMutJobService {
    async fn run_forever(&self) -> Never {
        self.0.run_forever().await
    }

    fn set_heartbeat(&mut self, heartbeat: Arc<dyn Heartbeat>) {
        self.0.set_heartbeat(heartbeat)
    }

    fn name(&self) -> String {
        Service::name(&self.0)
    }

    fn heartbeat_ttl(&self) -> i32 {
        self.0.heartbeat_ttl()
    }

    fn is_healthy(&self) -> bool {
        self.0.is_healthy()
    }
}

#[derive(Clone)]
pub struct LoopConfig {
    /// Time to pause between iterations, doing nothing.
    pub delay_secs: i32,
    /// The longest you would ever expect it to take to execute a single
    /// iteration, not including the delay.
    pub max_iteration_secs: i32,
}

pub trait LoopableJob: Job + SelfConfiguredLoop {}
impl<T: Job + SelfConfiguredLoop> LoopableJob for T {}

pub trait SelfConfiguredLoop {
    fn loop_config(&self) -> LoopConfig;
}

#[async_trait]
impl<J: Job, T: Send + Sync> Job for (J, T) {
    async fn run_once(&self) -> anyhow::Result<()> {
        self.0.run_once().await
    }

    fn name(&self) -> String {
        short_name::<J>()
    }
}
impl<T> SelfConfiguredLoop for (T, LoopConfig) {
    fn loop_config(&self) -> LoopConfig {
        self.1.clone()
    }
}

pub trait AsJob {
    fn as_job<'a>(self: Arc<Self>) -> Arc<dyn Job + 'a>
    where
        Self: 'a;
}

impl<T: Job + Sized> AsJob for T {
    fn as_job<'a>(self: Arc<Self>) -> Arc<dyn Job + 'a>
    where
        Self: 'a,
    {
        self
    }
}

#[async_trait]
impl Job for LoopingJobService {
    async fn run_once(&self) -> anyhow::Result<()> {
        self.job.run_once().await
    }
}

#[async_trait]
impl Service for LoopingJobService {
    async fn run_forever(&self) -> Never {
        loop {
            self.heartbeat.beat();
            let success = self
                .job
                .run_once()
                .await
                .log_with_context(|| format!("Unhandled error in {}", self.job.name()));
            match success {
                Some(()) => self.consecutive_failures.store(0, Ordering::Relaxed),
                None => {
                    self.consecutive_failures.fetch_add(1, Ordering::Relaxed);
                }
            }
            self.sleeper
                .sleep(Duration::from_secs(
                    self.job.loop_config().delay_secs as u64,
                ))
                .await;
        }
    }

    fn set_heartbeat(&mut self, heartbeat: Arc<dyn Heartbeat>) {
        self.heartbeat = heartbeat;
    }

    fn name(&self) -> String {
        self.job.name()
    }

    fn heartbeat_ttl(&self) -> i32 {
        self.job.loop_config().delay_secs + self.job.loop_config().max_iteration_secs
    }

    fn is_healthy(&self) -> bool {
        0 == self.consecutive_failures.load(Ordering::Relaxed)
    }
}

#[async_trait]
pub trait Job: AsJob + Send + Sync {
    async fn run_once(&self) -> anyhow::Result<()>;

    fn name(&self) -> String {
        short_name::<Self>()
    }
}

#[async_trait]
impl Job for Vec<Arc<dyn Job>> {
    async fn run_once(&self) -> anyhow::Result<()> {
        for item in self.iter() {
            item.run_once().await?;
        }
        Ok(())
    }
}

#[async_trait]
impl<F: Fn() + Send + Sync> Job for F {
    async fn run_once(&self) -> anyhow::Result<()> {
        self();
        Ok(())
    }
}

/// Use this if you never plan on trying to use the same instance of this job
/// multiple times concurrently.
#[async_trait]
pub trait MutJob {
    async fn run_once_mut(&mut self) -> anyhow::Result<()>;
}

#[async_trait]
impl<Mj: MutJob + Send + Sync> Job for Mutex<Mj> {
    async fn run_once(&self) -> anyhow::Result<()> {
        timeout(Duration::from_secs(60), self.lock())
            .await?
            .run_once_mut()
            .await
    }

    fn name(&self) -> String {
        short_name::<Mj>()
    }
}
