//! Fixed-file latency checks; never opens a microphone or types into another app.
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use voicekey_core::{audio, util, DoubaoEngine, Engine};

fn elapsed(start: Instant) -> u64 {
    start.elapsed().as_millis() as u64
}

fn event(value: Value) {
    println!("{value}");
    let _ = std::io::stdout().flush();
}

async fn measure(
    engine: Arc<DoubaoEngine>,
    file: &str,
    run: usize,
    prewarm_ms: u64,
) -> Result<Value> {
    let started = Instant::now();
    let first_partial = Arc::new(AtomicU64::new(u64::MAX));
    let audio_end = Arc::new(AtomicU64::new(u64::MAX));
    let mut source = audio::file_frames(file, false).context("read test audio")?;
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    let ended = audio_end.clone();
    let capture = tokio::spawn(async move {
        let clock = tokio::time::Instant::now();
        let mut frames = 0u64;
        while let Some(frame) = source.recv().await {
            tokio::time::sleep_until(clock + Duration::from_millis(frames * 20)).await;
            if sender.send(frame).is_err() {
                return;
            }
            frames += 1;
        }
        tokio::time::sleep_until(clock + Duration::from_millis(frames * 20)).await;
        let at = elapsed(started);
        ended.store(at, Ordering::Release);
        event(json!({"event": "audio_end", "run": run, "elapsed_ms": at}));
    });
    let warm_engine = engine.clone();
    let warm = tokio::spawn(async move { warm_engine.prewarm().await });
    tokio::time::sleep(Duration::from_millis(prewarm_ms)).await;
    let first = first_partial.clone();
    let result = engine
        .run(
            receiver,
            Box::new(move |text| {
                if !text.is_empty() {
                    let at = elapsed(started);
                    if first
                        .compare_exchange(u64::MAX, at, Ordering::AcqRel, Ordering::Acquire)
                        .is_ok()
                    {
                        event(json!({"event": "first_partial", "run": run, "elapsed_ms": at}));
                    }
                }
            }),
        )
        .await;
    let final_ms = elapsed(started);
    capture.abort();
    let _ = capture.await;
    warm.abort();
    let _ = warm.await;
    let end = audio_end.load(Ordering::Acquire);
    let first = first_partial.load(Ordering::Acquire);
    Ok(json!({
        "event": "result", "run": run, "prewarm_ms": prewarm_ms,
        "first_partial_ms": (first != u64::MAX).then_some(first),
        "audio_end_ms": (end != u64::MAX).then_some(end),
        "final_ms": final_ms,
        "after_audio_ms": (end != u64::MAX).then(|| final_ms.saturating_sub(end)),
        "success": result.is_ok(), "text": result.as_ref().ok(),
        "error": result.as_ref().err().map(|error| format!("{error:#}")),
    }))
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        bail!("usage: vk-latency file.wav isolated-data-dir [runs=3] [prewarm-ms=300]");
    }
    util::set_data_dir(args[2].clone().into());
    let runs = args
        .get(3)
        .map(|value| value.parse())
        .transpose()?
        .unwrap_or(3);
    let prewarm = args
        .get(4)
        .map(|value| value.parse())
        .transpose()?
        .unwrap_or(300);
    let engine = Arc::new(DoubaoEngine::default());
    for run in 1..=runs {
        let result = measure(engine.clone(), &args[1], run, prewarm).await?;
        let success = result["success"].as_bool() == Some(true);
        event(result);
        if !success {
            bail!("recognition failed; stopping the latency check");
        }
        if run < runs {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }
    Ok(())
}
