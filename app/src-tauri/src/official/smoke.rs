use super::credentials;
use serde_json::json;
use std::sync::Arc;
use voicekey_core::{util, DoubaoEngine, Engine, VolcEngine};

mod measure;

pub fn main() -> Option<i32> {
    let mut args = std::env::args().skip(1);
    let mode = args.next()?;
    if mode != "--test-volc-file" && mode != "--test-asr-file" {
        return None;
    }
    let channel = if mode == "--test-asr-file" {
        args.next().unwrap_or_default()
    } else {
        "volc".into()
    };
    let Some(file) = args.next() else {
        eprintln!("usage: voicekey --test-volc-file sample.wav | --test-asr-file doubao|volc sample.wav isolated-data-dir");
        return Some(2);
    };
    if channel != "doubao" && channel != "volc" {
        eprintln!("Unsupported test channel");
        return Some(2);
    }
    if channel == "doubao" {
        let Some(dir) = args.next() else {
            eprintln!("Doubao comparisons require an isolated device cache directory");
            return Some(2);
        };
        util::set_data_dir(dir.into());
    }
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(runtime) => runtime,
        Err(_) => return Some(2),
    };
    let outcome = runtime.block_on(async {
        let engine: Arc<dyn Engine> = if channel == "volc" {
            let key = credentials::load()
                .map_err(anyhow::Error::msg)?
                .ok_or_else(|| anyhow::anyhow!("请先在应用中保存火山语音 API Key"))?;
            let engine = VolcEngine::default();
            engine.set_key(key)?;
            Arc::new(engine)
        } else {
            Arc::new(DoubaoEngine::default())
        };
        let compare = mode == "--test-asr-file";
        let mut result =
            measure::run(engine, &file, if compare { 300 } else { 0 }, compare).await?;
        result["channel"] = json!(channel);
        Ok::<_, anyhow::Error>(result)
    });
    match outcome {
        Ok(value) => {
            println!("{value}");
            Some(if value["success"] == true { 0 } else { 1 })
        }
        Err(error) => {
            println!("{}", json!({"success":false, "error":error.to_string()}));
            Some(1)
        }
    }
}
