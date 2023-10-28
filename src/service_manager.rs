use std::{
    ops::Deref,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};

use anyhow::{bail, ensure};
use futures::lock::Mutex;
use futures::FutureExt;
use tokio::task::JoinHandle;

use super::service::Job;
use crate::health_endpoint;
use crate::service::Service;
use crate::{
    error::display_error,
    health_endpoint::HealthEndpointConfig,
    time::{Clock, Sleeper, SystemClock, TokioSleeper},
    Never, ToError,
};

#[async_trait::async_trait]
pub trait Application: Initialize + Serves {
    /// Starts the app and runs forever with default monitoring
    async fn run(&self) -> anyhow::Result<Never> {
        self.run_custom(Default::default()).await
    }

    /// Starts the app and runs forever with configured monitoring
    async fn run_custom(&self, config: RunConfig) -> anyhow::Result<Never> {
        config.validate()?;
        let mgr = Arc::new(self.start().await?);
        if let Some(config) = config.http_health_endpoint {
            let mgr2 = mgr.clone();
            tokio::spawn(async move { health_endpoint::run(mgr2, config).await });
        }
        Ok(mgr
            .monitor_with_recovery(10, config.log_interval, config.attempt_recovery_after)
            .await)
    }

    /// Starts the app and returns, so you may implement custom monitoring.
    async fn start(&self) -> anyhow::Result<ServiceManager> {
        tracing::info!("Initializing application.");
        self.init().run_once().await?;
        let mut mgr = ServiceManager::new(
            Arc::new(SystemClock::default()),
            Arc::new(TokioSleeper::default()),
        );
        for service in self.services() {
            mgr.register_service(service);
        }
        tracing::info!("Starting Services.");
        mgr.spawn_services().await?;
        Ok(mgr)
    }
}
impl<T: Initialize + Serves> Application for T {}

pub struct RunConfig {
    /// Interval in seconds between application status log messages when there are no problems.
    pub log_interval: i32,
    /// Services will be restarted after they have been in a failing state for this many seconds.
    pub attempt_recovery_after: i32,
    /// Set to None to disable the http health endpoint.
    pub http_health_endpoint: Option<HealthEndpointConfig>,
}

impl RunConfig {
    fn validate(&self) -> anyhow::Result<()> {
        ensure!(self.log_interval >= 0);
        ensure!(self.attempt_recovery_after >= 0);
        Ok(())
    }
}

impl Default for RunConfig {
    fn default() -> Self {
        Self {
            log_interval: 21600,
            attempt_recovery_after: 120,
            http_health_endpoint: Some(Default::default()),
        }
    }
}

pub trait Initialize {
    /// Returns the jobs that should run once before initializing the long
    /// running services.
    ///
    /// Calling this function does not run the jobs. You must run them some
    /// other way.
    #[must_use]
    fn init(&self) -> Vec<Arc<dyn Job>> {
        vec![]
    }
}

pub trait Serves {
    /// Returns the services that should run for the entire life of the app.
    ///
    /// Calling this function does not run the services. You must run them some
    /// other way.
    #[must_use]
    fn services(&self) -> Vec<Box<dyn Service>>;
}

/// Manages services: start/stop/monitor
pub struct ServiceManager {
    clock: Arc<dyn Clock>,
    sleeper: Arc<dyn Sleeper>,
    managed_services: Vec<ManagedService>,
}

impl ServiceManager {
    pub fn new(clock: Arc<dyn Clock>, sleeper: Arc<dyn Sleeper>) -> Self {
        Self {
            clock,
            sleeper,
            managed_services: vec![],
        }
    }
}

impl ServiceManager {
    pub fn register_service(&mut self, mut service: Box<dyn Service>) {
        let heart = Arc::new(AtomicUsize::new(0));
        service.set_heartbeat(Arc::new((heart.clone(), self.clock.clone())));
        self.managed_services.push(ManagedService {
            clock: self.clock.clone(),
            service: service.into(),
            heart,
            failing_health_checks_since: 0.into(),
            handle: Mutex::new(None),
        });
    }

    pub async fn spawn_services(&mut self) -> anyhow::Result<()> {
        for managed_service in self.managed_services.iter_mut() {
            managed_service.spawn().await?;
        }
        Ok(())
    }

    pub async fn monitor(&self, check_interval: u64, log_interval: i32) -> Never {
        let mut publisher = ServiceReportPublisher::new(log_interval);
        loop {
            publisher.handle_report(self.check());
            self.sleeper
                .sleep(Duration::from_secs(check_interval))
                .await;
        }
    }

    /// If a service is dead for over two minutes, try to restart it.
    pub async fn monitor_with_recovery(
        &self,
        check_interval: u64,
        log_interval: i32,
        attempt_recovery_after: i32,
    ) -> Never {
        let mut publisher = ServiceReportPublisher::new(log_interval);
        loop {
            let report = self.check();
            publisher.handle_report(report.clone());
            for ServiceReport {
                id: index,
                status: Unhealthy { since, .. },
                ..
            } in report.dead
            {
                let svc = &self.managed_services[index];
                if since + attempt_recovery_after < self.clock.current_timestamp() as i32 {
                    if let Err(e) = svc.restart(Duration::from_secs(10)).await {
                        tracing::error!(
                            "Failed to restart {}: {}",
                            svc.service.name(),
                            display_error(&e)
                        );
                    }
                }
            }
            self.sleeper
                .sleep(Duration::from_secs(check_interval))
                .await;
        }
    }

    pub fn check(&self) -> ServiceReportSummary {
        let (mut alive, mut dead) = (vec![], vec![]);
        let timestamp = self.clock.current_timestamp() as i32;
        for (index, ms) in self.managed_services.iter().enumerate() {
            let status = ms.check(timestamp);
            if let ServiceStatus::Unhealthy(unhealthy) = status {
                dead.push(ServiceReport {
                    id: index,
                    name: ms.service.name(),
                    status: unhealthy,
                });
            } else {
                alive.push(ServiceReport {
                    id: index,
                    name: ms.service.name(),
                    status: (),
                });
            }
        }
        ServiceReportSummary {
            alive,
            dead,
            timestamp,
        }
    }
}

struct ManagedService {
    clock: Arc<dyn Clock>,
    service: Arc<dyn Service>,
    heart: Arc<AtomicUsize>,
    failing_health_checks_since: AtomicUsize,
    handle: Mutex<Option<JoinHandle<Never>>>, // TODO: shouldn't need a mutex here but it's hard to do another way
}

impl ManagedService {
    /// If healthy, returns an empty vec.
    /// If unhealthy, returns a list of reasons why.
    pub fn check(&self, current_timestamp: i32) -> ServiceStatus {
        let mut unhealthy_since = 0;
        let last_beat = self.heart.load(Ordering::Relaxed) as i32;
        let time_since_last_beat = current_timestamp - last_beat;
        let ttl = self.service.heartbeat_ttl();
        let mut dead_reasons = vec![];
        if time_since_last_beat >= ttl {
            unhealthy_since = last_beat + ttl;
            dead_reasons.push(format!(
                "No heartbeat for {time_since_last_beat} seconds (ttl: {ttl})"
            ))
        }
        if self.service.is_healthy() {
            self.failing_health_checks_since.store(0, Ordering::Relaxed);
        } else {
            let failing_since = match self.failing_health_checks_since.compare_exchange(
                0,
                current_timestamp as usize,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => current_timestamp,
                Err(existing) => existing as i32,
            };
            unhealthy_since = std::cmp::min(failing_since, unhealthy_since);
            dead_reasons.push(format!(
                "Failed health checks for {} seconds",
                current_timestamp - failing_since
            ))
        }

        if dead_reasons.is_empty() {
            ServiceStatus::Healthy
        } else {
            ServiceStatus::Unhealthy(Unhealthy {
                since: unhealthy_since,
                reasons: dead_reasons,
            })
        }
    }

    pub async fn restart(&self, timeout: Duration) -> anyhow::Result<()> {
        self.abort(timeout).await?;
        self.spawn().await
    }

    pub async fn spawn(&self) -> anyhow::Result<()> {
        let mut handle = self.handle.lock().await;
        if handle.as_ref().is_some() {
            bail!(
                "Cannot start service, already running: {}",
                self.service.name()
            );
        }
        tracing::info!("Starting service: {}", self.service.name());
        (&*self.heart, self.clock.clone()).beat();
        let svc = self.service.clone();
        *handle = Some(tokio::spawn(async move { svc.run_forever().await }));
        self.failing_health_checks_since.store(0, Ordering::Relaxed);

        Ok(())
    }

    pub async fn abort(&self, timeout: Duration) -> anyhow::Result<()> {
        tracing::info!("Aborting service: {}", self.service.name());
        let mut handle_opt = self.handle.lock().await;
        if let Some(handle) = handle_opt.as_mut() {
            if handle.now_or_never().is_none() {
                handle.abort();
                tokio::time::timeout(timeout, async move {
                    let _desired_join_error = handle.await.to_error();
                })
                .await?;
            }
        }
        *handle_opt = None;
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub enum ServiceStatus {
    Healthy,
    Unhealthy(Unhealthy),
}

#[derive(Clone, Debug)]
pub struct Unhealthy {
    pub since: i32,
    pub reasons: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct ServiceReport<Status> {
    id: usize,
    name: String,
    status: Status,
}

#[derive(Clone, Debug)]
pub struct ServiceReportSummary {
    pub alive: Vec<ServiceReport<()>>,
    pub dead: Vec<ServiceReport<Unhealthy>>,
    pub timestamp: i32,
}

pub trait Heartbeat: Send + Sync {
    fn beat(&self);
}

impl Heartbeat for () {
    fn beat(&self) {}
}

impl<U, C> Heartbeat for (U, C)
where
    U: Deref<Target = AtomicUsize> + Send + Sync,
    C: Deref<Target = dyn Clock> + Send + Sync,
{
    fn beat(&self) {
        self.0
            .store(self.1.current_timestamp() as usize, Ordering::Relaxed);
    }
}

struct ServiceReportPublisher {
    log_interval: i32,
    last_healthy_log: i32,
    last_healthy_count: usize,
    last_dead_count: usize,
}

impl ServiceReportPublisher {
    fn new(log_interval: i32) -> Self {
        Self {
            log_interval,
            last_healthy_log: 0,
            last_healthy_count: 0,
            last_dead_count: 0,
        }
    }

    /// Decide if anything needs to be published, and publish it.
    fn handle_report(
        &mut self,
        ServiceReportSummary {
            alive,
            dead,
            timestamp,
            ..
        }: ServiceReportSummary,
    ) {
        let some_are_dead = !dead.is_empty();
        let n_healthy = alive.len();

        if some_are_dead {
            tracing::error!(
                "{n_healthy} services are healthy. {} are unhealthy: {dead:#?}",
                dead.len()
            );
        } else if timestamp - self.last_healthy_log >= self.log_interval
            || n_healthy != self.last_healthy_count
            || dead.len() != self.last_dead_count
        {
            let prefix = if some_are_dead { "" } else { "all " };
            let alive_list = pretty(&alive.into_iter().map(|x| x.name).collect());
            if n_healthy + dead.len() != self.last_healthy_count + self.last_dead_count {
                tracing::info!("{prefix}{n_healthy} services are healthy: {alive_list}");
            } else {
                tracing::info!("{prefix}{n_healthy} services are healthy");
                tracing::debug!("{prefix}{n_healthy} services are healthy: {alive_list}");
            }
            self.last_healthy_log = timestamp;
        }
        self.last_healthy_count = n_healthy;
        self.last_dead_count = dead.len();
    }
}

fn pretty(v: &Vec<String>) -> String {
    format!("{v:#?}")
        .replace('\"', "")
        .replace("[\n", "\n")
        .replace(",\n", "\n")
        .replace("\n]", "")
}
