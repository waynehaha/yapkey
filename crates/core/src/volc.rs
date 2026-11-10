//! Official Doubao Speech 2.0 streaming API; credentials are supplied by the desktop keychain.
use crate::{ws, Audio, Engine, Partial};
use anyhow::{bail, Context, Result};
use futures_util::{SinkExt, StreamExt};
use std::sync::RwLock;
use std::time::Duration;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::Message;

mod protocol;

const ENDPOINT: &str = "wss://openspeech.bytedance.com/api/v3/sauc/bigmodel_async";
pub const RESOURCE_ID: &str = "volc.seedasr.sauc.duration";
const CHUNK_SAMPLES: usize = crate::audio::FRAME * 10;
const MIN_PACKET_INTERVAL: Duration = Duration::from_millis(100);
const SEND_TIMEOUT: Duration = Duration::from_secs(10);
const FINAL_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Default)]
pub struct VolcEngine {
    api_key: RwLock<String>,
}

pub fn validate_key(input: &str) -> Result<String> {
    let key = input.trim();
    if key.is_empty() || key.len() > 4096 || !key.bytes().all(|byte| (33..=126).contains(&byte)) {
        bail!("请输入有效的火山语音 API Key，不要包含空格或换行");
    }
    Ok(key.into())
}

impl VolcEngine {
    pub fn set_key(&self, key: String) -> Result<()> {
        let key = validate_key(&key)?;
        *self
            .api_key
            .write()
            .map_err(|_| anyhow::anyhow!("火山语音配置暂不可用"))? = key;
        Ok(())
    }

    pub fn clear_key(&self) {
        if let Ok(mut key) = self.api_key.write() {
            key.clear();
        }
    }

    pub fn configured(&self) -> bool {
        self.api_key.read().is_ok_and(|key| !key.is_empty())
    }
}

#[async_trait::async_trait]
impl Engine for VolcEngine {
    async fn run(&self, audio: Audio, partial: Partial) -> Result<String> {
        let key = self
            .api_key
            .read()
            .map_err(|_| anyhow::anyhow!("火山语音配置暂不可用"))?
            .clone();
        if key.is_empty() {
            bail!("请先在识别设置中配置火山语音 API Key");
        }
        let id = uuid::Uuid::new_v4().to_string();
        let socket = ws::connect(
            ENDPOINT,
            &[
                ("X-Api-Key", &key),
                ("X-Api-Resource-Id", RESOURCE_ID),
                ("X-Api-Request-Id", &id),
                ("X-Api-Connect-Id", &id),
            ],
        )
        .await
        .context("火山语音连接失败，请检查网络、密钥和服务开通状态")?;
        recognize(socket, audio, partial).await
    }

    // Do not open billable API sessions merely because a modifier key was pressed.
}

async fn recognize(socket: ws::Ws, mut audio: Audio, partial: Partial) -> Result<String> {
    let (mut sender, mut receiver) = socket.split();
    tokio::time::timeout(
        SEND_TIMEOUT,
        sender.send(Message::Binary(protocol::config()?)),
    )
    .await
    .context("火山语音请求发送超时")??;
    let mut text = String::new();
    let mut samples = Vec::with_capacity(CHUNK_SAMPLES);
    let mut input_closed = false;
    let mut sent_end = false;
    let mut next_packet = Instant::now();
    let mut final_deadline = Instant::now();

    loop {
        let send_at = if input_closed {
            Instant::now()
        } else {
            next_packet
        };
        tokio::select! {
            message = receiver.next() => {
                let Some(data) = ws::data(message)? else { continue };
                let response = protocol::parse(&data)?;
                if let Some(next) = response.text.filter(|next| !next.is_empty()) {
                    if next != text {
                        text = next;
                        partial(&text);
                    }
                }
                if response.last {
                    if !sent_end {
                        bail!("火山语音会话提前结束，请重试");
                    }
                    let _ = tokio::time::timeout(Duration::from_secs(1), sender.close()).await;
                    if text.is_empty() {
                        bail!("没有识别到语音内容");
                    }
                    return Ok(text);
                }
            }
            frame = audio.recv(), if !input_closed && samples.len() < CHUNK_SAMPLES => {
                match frame {
                    Some(frame) => {
                        if frame.len() != crate::audio::FRAME {
                            bail!("火山语音音频帧大小无效");
                        }
                        samples.extend(frame);
                    }
                    None => input_closed = true,
                }
            }
            _ = tokio::time::sleep_until(send_at), if !sent_end && (input_closed || samples.len() >= CHUNK_SAMPLES) => {
                let packet = protocol::audio(&samples, input_closed)?;
                tokio::time::timeout(SEND_TIMEOUT, sender.send(Message::Binary(packet)))
                    .await.context("火山语音音频发送超时")??;
                samples.clear();
                next_packet = Instant::now() + MIN_PACKET_INTERVAL;
                if input_closed {
                    sent_end = true;
                    final_deadline = Instant::now() + FINAL_TIMEOUT;
                }
            }
            _ = tokio::time::sleep_until(final_deadline), if sent_end => {
                bail!("火山语音定稿超时，请重试");
            }
        }
    }
}

#[cfg(test)]
mod tests;
