use anyhow::{bail, Context, Result};
use flate2::{read::GzDecoder, write::GzEncoder, Compression};
use serde_json::{json, Value};
use std::io::{Read, Write};

const MAX_PAYLOAD: usize = 4 * 1024 * 1024;

pub(super) struct Response {
    pub text: Option<String>,
    pub last: bool,
}

fn packet(kind: u8, flags: u8, serialization: u8, payload: &[u8]) -> Result<Vec<u8>> {
    let mut gzip = GzEncoder::new(Vec::new(), Compression::fast());
    gzip.write_all(payload)?;
    let payload = gzip.finish()?;
    let mut message = vec![0x11, kind << 4 | flags, serialization << 4 | 1, 0];
    message.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    message.extend_from_slice(&payload);
    Ok(message)
}

pub(super) fn config() -> Result<Vec<u8>> {
    let config = json!({
        "user": {"uid": "voicekey-desktop"},
        "audio": {"format": "pcm", "codec": "raw", "rate": 16000, "bits": 16, "channel": 1},
        "request": {
            "model_name": "bigmodel", "result_type": "full", "enable_nonstream": true,
            "enable_itn": true, "enable_punc": true, "enable_ddc": false,
        },
    });
    packet(1, 0, 1, &serde_json::to_vec(&config)?)
}

pub(super) fn audio(samples: &[i16], last: bool) -> Result<Vec<u8>> {
    let bytes: Vec<u8> = samples
        .iter()
        .flat_map(|sample| sample.to_le_bytes())
        .collect();
    packet(2, if last { 2 } else { 0 }, 0, &bytes)
}

fn number(data: &[u8], offset: &mut usize) -> Result<u32> {
    let bytes = data
        .get(*offset..*offset + 4)
        .context("火山语音响应不完整")?;
    *offset += 4;
    Ok(u32::from_be_bytes(bytes.try_into().unwrap()))
}

pub(super) fn parse(data: &[u8]) -> Result<Response> {
    if data.len() < 4 || data[0] >> 4 != 1 || data[0] & 15 == 0 {
        bail!("火山语音响应协议无效");
    }
    let mut offset = (data[0] as usize & 15) * 4;
    if offset > data.len() {
        bail!("火山语音响应头不完整");
    }
    let kind = data[1] >> 4;
    let flags = data[1] & 15;
    let mut last = flags & 2 != 0;
    if kind == 15 {
        let code = number(data, &mut offset)?;
        // Error text can reflect credentials or transcript content; expose only the numeric code.
        bail!("火山语音服务返回错误（{code}），请检查服务开通、密钥权限和可用额度");
    }
    if kind != 9 || data[2] >> 4 != 1 {
        bail!("火山语音响应类型不受支持");
    }
    if flags & 1 != 0 {
        last |= (number(data, &mut offset)? as i32) < 0;
    }
    let size = number(data, &mut offset)? as usize;
    if size > MAX_PAYLOAD || size != data.len().saturating_sub(offset) {
        bail!("火山语音响应长度无效");
    }
    let compressed = &data[offset..];
    let payload = match data[2] & 15 {
        0 => compressed.to_vec(),
        1 => {
            let mut payload = Vec::new();
            GzDecoder::new(compressed)
                .take(MAX_PAYLOAD as u64 + 1)
                .read_to_end(&mut payload)
                .context("火山语音响应解压失败")?;
            if payload.len() > MAX_PAYLOAD {
                bail!("火山语音响应超过大小限制");
            }
            payload
        }
        _ => bail!("火山语音响应压缩格式不受支持"),
    };
    let value: Value = serde_json::from_slice(&payload).context("火山语音响应 JSON 无效")?;
    let text = value["result"]["text"]
        .as_str()
        .map(String::from)
        .or_else(|| {
            value["result"].as_array().map(|results| {
                results
                    .iter()
                    .filter_map(|result| result["text"].as_str())
                    .collect::<String>()
            })
        });
    Ok(Response { text, last })
}

#[cfg(test)]
mod tests;
