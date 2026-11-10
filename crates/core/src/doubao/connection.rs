use super::{expect, request, DoubaoDevice, APP_KEY, WS_URL};
use crate::ws::{self, Ws};
use anyhow::{Context, Result};
use futures_util::SinkExt;
use serde_json::json;
use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tokio::time::Instant;

const PREPARE_TIMEOUT: Duration = Duration::from_secs(10);
const IDLE_LIFETIME: Duration = Duration::from_secs(15);

pub(super) struct Session {
    pub ws: Ws,
    pub rid: String,
}

struct Prepared {
    ws: Ws,
    rid: String,
    did: String,
}

impl Prepared {
    async fn open() -> Result<Self> {
        let device = DoubaoDevice::load().await?;
        let url = format!("{WS_URL}?aid=401734&device_id={}", device.did);
        Self::connect(&url, &device.phone.ua(), device.did).await
    }

    async fn connect(url: &str, ua: &str, did: String) -> Result<Self> {
        let mut ws = ws::connect(
            url,
            &[
                ("User-Agent", ua),
                ("proto-version", "v2"),
                ("x-custom-keepalive", "true"),
            ],
        )
        .await?;
        let rid = uuid::Uuid::new_v4().to_string();
        ws.send(request(APP_KEY, "StartTask", "", &[], &rid, 0))
            .await?;
        expect(&mut ws, "TaskStarted").await?;
        Ok(Self { ws, rid, did })
    }

    async fn start(mut self) -> Result<Session> {
        let config = json!({
            "audio_info": {"channel": 1, "format": "speech_opus", "sample_rate": 16000},
            "enable_punctuation": true,
            "enable_speech_rejection": true,
            "extra": {"app_name": "oime", "cell_compress_rate": 8, "did": self.did,
                      "enable_asr_threepass": false, "enable_asr_twopass": true, "input_mode": "stream"},
        }).to_string();
        self.ws
            .send(request(APP_KEY, "StartSession", &config, &[], &self.rid, 0))
            .await?;
        expect(&mut self.ws, "SessionStarted").await?;
        Ok(Session {
            ws: self.ws,
            rid: self.rid,
        })
    }
}

struct Cached {
    value: Result<Prepared>,
    expires: Instant,
    epoch: u64,
}

#[derive(Default)]
pub(super) struct Pool {
    slot: Arc<Mutex<Option<Cached>>>,
    epoch: AtomicU64,
}

impl Pool {
    pub async fn prewarm(&self) {
        self.prewarm_with(Prepared::open).await;
    }

    async fn prewarm_with<F, Fut>(&self, open: F)
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<Prepared>>,
    {
        let mut slot = self.slot.lock().await;
        let epoch = self.epoch.load(Ordering::Acquire);
        if slot.as_ref().is_some_and(|cached| {
            cached.epoch == epoch && cached.expires > Instant::now() && cached.value.is_ok()
        }) {
            return;
        }
        *slot = None;
        let value = match tokio::time::timeout(PREPARE_TIMEOUT, open()).await {
            Ok(value) => value,
            Err(error) => Err(error).context("豆包连接准备超时"),
        };
        if self.epoch.load(Ordering::Acquire) != epoch {
            return;
        }
        *slot = Some(Cached {
            value,
            expires: Instant::now() + IDLE_LIFETIME,
            epoch,
        });
        let weak = Arc::downgrade(&self.slot);
        tokio::spawn(async move {
            tokio::time::sleep(IDLE_LIFETIME).await;
            if let Some(slot) = weak.upgrade() {
                let mut slot = slot.lock().await;
                if slot
                    .as_ref()
                    .is_some_and(|cached| cached.expires <= Instant::now())
                {
                    *slot = None;
                }
            }
        });
    }

    pub async fn start(&self) -> Result<Session> {
        self.start_with(Prepared::open).await
    }

    async fn start_with<F, Fut>(&self, open: F) -> Result<Session>
    where
        F: Fn() -> Fut,
        Fut: Future<Output = Result<Prepared>>,
    {
        tokio::time::timeout(PREPARE_TIMEOUT, self.start_inner(open))
            .await
            .context("豆包连接准备超时")?
    }

    async fn start_inner<F, Fut>(&self, open: F) -> Result<Session>
    where
        F: Fn() -> Fut,
        Fut: Future<Output = Result<Prepared>>,
    {
        let cached = self.slot.lock().await.take().filter(|cached| {
            cached.epoch == self.epoch.load(Ordering::Acquire) && cached.expires > Instant::now()
        });
        if let Some(cached) = cached {
            // A prepared task is leased once. Confirm it is still usable before consuming any audio.
            if let Ok(session) = cached.value?.start().await {
                return Ok(session);
            }
        }
        open().await?.start().await
    }

    pub fn clear(&self) {
        self.epoch.fetch_add(1, Ordering::AcqRel);
        if let Ok(mut slot) = self.slot.try_lock() {
            *slot = None;
        }
    }
}

#[cfg(test)]
mod tests;
