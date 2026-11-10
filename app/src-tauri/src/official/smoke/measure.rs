use serde_json::{json, Value};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use voicekey_core::{audio, ws::AbortOnDrop, Engine};

pub(super) async fn run(
    engine: Arc<dyn Engine>,
    file: &str,
    trigger_ms: u64,
    prewarm: bool,
) -> anyhow::Result<Value> {
    // Decode before starting the common capture/recognition clock for either channel.
    let mut source = audio::file_frames(file, false)?;
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    let started = Instant::now();
    let clock = tokio::time::Instant::now();
    let audio_end = Arc::new(AtomicU64::new(0));
    let ended = audio_end.clone();
    let capture = AbortOnDrop(tokio::spawn(async move {
        let mut frames = 0;
        while let Some(frame) = source.recv().await {
            tokio::time::sleep_until(clock + Duration::from_millis(frames * 20)).await;
            if sender.send(frame).is_err() {
                return;
            }
            frames += 1;
        }
        tokio::time::sleep_until(clock + Duration::from_millis(frames * 20)).await;
        ended.store(started.elapsed().as_millis() as u64, Ordering::Release);
    }));
    let warm = if prewarm {
        let engine = engine.clone();
        Some(AbortOnDrop(tokio::spawn(
            async move { engine.prewarm().await },
        )))
    } else {
        None
    };
    if trigger_ms > 0 {
        tokio::time::sleep_until(clock + Duration::from_millis(trigger_ms)).await;
    }
    let first = Arc::new(AtomicU64::new(0));
    let partial_time = first.clone();
    let callbacks = Arc::new(AtomicU64::new(0));
    let updates = callbacks.clone();
    let result = engine
        .run(
            receiver,
            Box::new(move |text| {
                if !text.is_empty() {
                    updates.fetch_add(1, Ordering::Relaxed);
                    let _ = partial_time.compare_exchange(
                        0,
                        started.elapsed().as_millis() as u64,
                        Ordering::AcqRel,
                        Ordering::Acquire,
                    );
                }
            }),
        )
        .await;
    let final_ms = started.elapsed().as_millis() as u64;
    let end = audio_end.load(Ordering::Acquire);
    drop(capture);
    drop(warm);
    Ok(json!({
        "success":result.is_ok(),"text":result.as_ref().ok(),
        "error":result.as_ref().err().map(ToString::to_string),
        "first_partial_ms":first.load(Ordering::Acquire),"final_ms":final_ms,
        "audio_end_ms":end,"after_audio_ms":(end!=0).then(||final_ms.saturating_sub(end)),
        "trigger_ms":trigger_ms,"prewarm_requested":prewarm,"partial_callbacks":callbacks.load(Ordering::Relaxed),
    }))
}
