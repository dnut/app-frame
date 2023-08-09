use std::time::{Duration, SystemTime};

use async_trait::async_trait;

pub trait Clock: Send + Sync {
    fn current_timestamp(&self) -> u64;
}

#[async_trait]
pub trait Sleeper: Send + Sync {
    async fn sleep(&self, duration: Duration);
}

pub struct SystemClock(u64);

impl SystemClock {
    pub const fn default() -> Self {
        Self(1)
    }

    pub const fn accelerated(scale: u64) -> Self {
        Self(scale)
    }
}

impl Clock for SystemClock {
    fn current_timestamp(&self) -> u64 {
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            * self.0
    }
}

pub struct TokioSleeper(pub u32);

impl TokioSleeper {
    pub const fn default() -> Self {
        Self(1)
    }

    pub const fn accelerated(scale: u32) -> Self {
        Self(scale)
    }
}

#[async_trait]
impl Sleeper for TokioSleeper {
    async fn sleep(&self, duration: Duration) {
        tokio::time::sleep(duration / self.0).await
    }
}

pub struct Insomniac;

#[async_trait]
impl Sleeper for Insomniac {
    async fn sleep(&self, _duration: Duration) {}
}
