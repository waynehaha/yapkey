use super::FRAME;
use crate::ws::AbortOnDrop;
use crate::Audio;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::Instant;

const LEADING_FRAMES: usize = 10;
const FRAME_INTERVAL: Duration = Duration::from_millis(20);
const CATCHUP_INTERVAL: Duration = Duration::from_millis(5);
const BACKLOG_FRAMES: usize = 3;

pub struct Lead {
    rx: mpsc::Receiver<Vec<i16>>,
    _worker: AbortOnDrop<()>,
}

impl Lead {
    pub async fn recv(&mut self) -> Option<Vec<i16>> {
        self.rx.recv().await
    }
}

pub fn lead(audio: Audio) -> Lead {
    paced(audio, false)
}

/// Catch up only buffered speech, using the existing 4x replay cadence.
pub(crate) fn lead_catchup(audio: Audio) -> Lead {
    paced(audio, true)
}

fn paced(mut audio: Audio, catchup: bool) -> Lead {
    let (tx, rx) = mpsc::channel(1);
    let worker = tokio::spawn(async move {
        let mut next = Instant::now();
        for _ in 0..LEADING_FRAMES {
            if !emit(&tx, vec![0; FRAME], &mut next, FRAME_INTERVAL).await {
                return;
            }
        }
        while let Some(frame) = audio.recv().await {
            let interval = if catchup && audio.len() >= BACKLOG_FRAMES {
                CATCHUP_INTERVAL
            } else {
                FRAME_INTERVAL
            };
            if !emit(&tx, frame, &mut next, interval).await {
                return;
            }
        }
    });
    Lead {
        rx,
        _worker: AbortOnDrop(worker),
    }
}

async fn emit(
    tx: &mpsc::Sender<Vec<i16>>,
    frame: Vec<i16>,
    next: &mut Instant,
    interval: Duration,
) -> bool {
    let now = Instant::now();
    if *next > now {
        tokio::time::sleep_until(*next).await;
    } else {
        *next = now;
    }
    if tx.send(frame).await.is_err() {
        return false;
    }
    *next += interval;
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn delayed_capture(catchup: bool) -> (Duration, Vec<i16>) {
        let (tx, rx) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            for value in 1..=50 {
                if tx.send(vec![value; FRAME]).is_err() {
                    return;
                }
                tokio::time::sleep(FRAME_INTERVAL).await;
            }
        });
        let mut audio = paced(rx, catchup);
        let started = Instant::now();
        tokio::time::sleep(Duration::from_millis(400)).await;
        let mut frames = Vec::new();
        while let Some(frame) = audio.recv().await {
            frames.push(frame[0]);
        }
        (started.elapsed(), frames)
    }

    #[tokio::test(start_paused = true)]
    async fn normal_pacing_retains_the_leading_silence_and_all_frames() {
        let (elapsed, frames) = delayed_capture(false).await;
        let expected: Vec<_> = std::iter::repeat(0)
            .take(LEADING_FRAMES)
            .chain(1..=50)
            .collect();
        assert_eq!(frames, expected);
        assert!(elapsed >= Duration::from_millis(1500));
    }

    #[tokio::test(start_paused = true)]
    async fn connection_backlog_is_recovered_without_dropping_first_or_last_frames() {
        let (normal, expected) = delayed_capture(false).await;
        let (caught_up, actual) = delayed_capture(true).await;
        assert_eq!(actual, expected);
        assert!(caught_up + Duration::from_millis(250) < normal);
        assert!(caught_up >= Duration::from_millis(1000));
    }

    #[tokio::test(start_paused = true)]
    async fn catchup_does_not_burst_an_entire_buffer() {
        let (tx, rx) = mpsc::unbounded_channel();
        for value in 1..=100 {
            tx.send(vec![value; FRAME]).unwrap();
        }
        drop(tx);
        let mut audio = lead_catchup(rx);
        let started = Instant::now();
        let mut frames = Vec::new();
        while let Some(frame) = audio.recv().await {
            frames.push(frame[0]);
        }
        assert_eq!(&frames[..LEADING_FRAMES], vec![0; LEADING_FRAMES]);
        assert_eq!(&frames[LEADING_FRAMES..], (1..=100).collect::<Vec<_>>());
        assert!(started.elapsed() >= Duration::from_millis(650));
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[tokio::test(start_paused = true)]
    async fn dropping_the_receiver_releases_an_idle_source() {
        let (tx, rx) = mpsc::unbounded_channel();
        let mut audio = lead_catchup(rx);
        for _ in 0..LEADING_FRAMES {
            assert!(audio.recv().await.is_some());
        }
        drop(audio);
        tokio::time::timeout(Duration::from_secs(1), tx.closed())
            .await
            .unwrap();
    }
}
