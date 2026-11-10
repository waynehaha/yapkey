use super::*;
use crate::pb::{self, PBuf};
use crate::ws::AbortOnDrop;
use futures_util::StreamExt;
use std::sync::atomic::AtomicUsize;
use tokio::net::TcpListener;
use tokio::sync::Notify;
use tokio_tungstenite::tungstenite::Message;

struct Server {
    url: String,
    connections: Arc<AtomicUsize>,
    _task: AbortOnDrop<()>,
}

impl Server {
    async fn open(close_first_task: bool) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}", listener.local_addr().unwrap());
        let connections = Arc::new(AtomicUsize::new(0));
        let count = connections.clone();
        let task = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let index = count.fetch_add(1, Ordering::AcqRel);
                tokio::spawn(async move {
                    let Ok(mut ws) = tokio_tungstenite::accept_async(stream).await else {
                        return;
                    };
                    while let Some(Ok(Message::Binary(data))) = ws.next().await {
                        let fields = pb::parse(&data).unwrap();
                        let event = match fields.string(5).as_deref() {
                            Some("StartTask") => "TaskStarted",
                            Some("StartSession") => "SessionStarted",
                            _ => continue,
                        };
                        let response = PBuf::new().s(4, event).v(5, super::super::OK as u64).0;
                        if ws.send(Message::Binary(response)).await.is_err() {
                            return;
                        }
                        if close_first_task && index == 0 && event == "TaskStarted" {
                            let _ = ws.close(None).await;
                            return;
                        }
                    }
                });
            }
        });
        Self {
            url,
            connections,
            _task: AbortOnDrop(task),
        }
    }

    async fn prepared(&self) -> Result<Prepared> {
        Prepared::connect(
            &self.url,
            "latency-regression",
            "isolated-test-device".into(),
        )
        .await
    }

    fn count(&self) -> usize {
        self.connections.load(Ordering::Acquire)
    }
}

#[tokio::test]
async fn a_prewarmed_task_is_used_without_a_second_connection() {
    let server = Server::open(false).await;
    let pool = Pool::default();
    pool.prewarm_with(|| server.prepared()).await;
    pool.prewarm_with(|| server.prepared()).await;
    let _session = pool.start_with(|| server.prepared()).await.unwrap();
    assert_eq!(server.count(), 1);
    assert!(pool.slot.lock().await.is_none());
}

#[tokio::test]
async fn starting_during_prewarm_waits_for_the_same_connection() {
    let server = Server::open(false).await;
    let pool = Pool::default();
    let entered = Notify::new();
    let release = Notify::new();
    let warm = pool.prewarm_with(|| async {
        entered.notify_one();
        release.notified().await;
        server.prepared().await
    });
    let start = async {
        entered.notified().await;
        release.notify_one();
        pool.start_with(|| server.prepared()).await
    };
    let (_, session) = tokio::join!(warm, start);
    assert!(session.is_ok());
    assert_eq!(server.count(), 1);
}

#[tokio::test]
async fn a_closed_prepared_task_reconnects_before_audio_is_consumed() {
    let server = Server::open(true).await;
    let pool = Pool::default();
    pool.prewarm_with(|| server.prepared()).await;
    let _session = pool.start_with(|| server.prepared()).await.unwrap();
    assert_eq!(server.count(), 2);
}

#[tokio::test]
async fn concurrent_sessions_never_share_one_prepared_socket() {
    let server = Server::open(false).await;
    let pool = Pool::default();
    pool.prewarm_with(|| server.prepared()).await;
    let (first, second) = tokio::join!(
        pool.start_with(|| server.prepared()),
        pool.start_with(|| server.prepared())
    );
    assert_ne!(first.unwrap().rid, second.unwrap().rid);
    assert_eq!(server.count(), 2);
}

#[tokio::test(start_paused = true)]
async fn a_failed_prewarm_does_not_repeat_the_full_connection_timeout() {
    let pool = Pool::default();
    let attempts = AtomicUsize::new(0);
    pool.prewarm_with(|| async {
        attempts.fetch_add(1, Ordering::AcqRel);
        Err(anyhow::anyhow!("controlled connection failure"))
    })
    .await;
    let error = pool
        .start_with(|| async {
            attempts.fetch_add(1, Ordering::AcqRel);
            Err(anyhow::anyhow!("unexpected retry"))
        })
        .await
        .err()
        .unwrap();
    assert!(error.to_string().contains("controlled connection failure"));
    assert_eq!(attempts.load(Ordering::Acquire), 1);
}

#[tokio::test(start_paused = true)]
async fn pending_preparation_has_a_bounded_start_timeout() {
    let pool = Pool::default();
    let started = Instant::now();
    let result = pool
        .start_with(|| std::future::pending::<Result<Prepared>>())
        .await;
    assert!(result.is_err());
    assert!(started.elapsed() <= PREPARE_TIMEOUT + Duration::from_secs(1));
}

#[tokio::test]
async fn idle_preparation_expires_and_releases_its_socket() {
    let server = Server::open(false).await;
    let pool = Pool::default();
    pool.prewarm_with(|| server.prepared()).await;
    tokio::task::yield_now().await;
    tokio::time::pause();
    tokio::time::advance(IDLE_LIFETIME + Duration::from_millis(1)).await;
    tokio::task::yield_now().await;
    assert!(pool.slot.lock().await.is_none());
}

#[tokio::test]
async fn clearing_an_inflight_prewarm_cannot_restore_an_old_device() {
    let server = Server::open(false).await;
    let pool = Pool::default();
    let entered = Notify::new();
    let release = Notify::new();
    let warm = pool.prewarm_with(|| async {
        entered.notify_one();
        release.notified().await;
        server.prepared().await
    });
    let clear = async {
        entered.notified().await;
        pool.clear();
        release.notify_one();
    };
    tokio::join!(warm, clear);
    assert!(pool.slot.lock().await.is_none());
}
