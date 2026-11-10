use super::*;
use flate2::{read::GzDecoder, write::GzEncoder, Compression};
use serde_json::json;
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;

fn response(text: &str, last: bool) -> Message {
    let mut gzip = GzEncoder::new(Vec::new(), Compression::fast());
    gzip.write_all(
        serde_json::to_string(&json!({"result": {"text": text}}))
            .unwrap()
            .as_bytes(),
    )
    .unwrap();
    let payload = gzip.finish().unwrap();
    let mut message = vec![0x11, if last { 0x93 } else { 0x91 }, 0x11, 0];
    message.extend_from_slice(&if last { -1i32 } else { 1i32 }.to_be_bytes());
    message.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    message.extend_from_slice(&payload);
    Message::Binary(message)
}

async fn socket_pair() -> (
    ws::Ws,
    tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    let client = ws::connect(&url, &[]);
    let server = async {
        let (stream, _) = listener.accept().await.unwrap();
        tokio_tungstenite::accept_async(stream).await.unwrap()
    };
    let (client, server) = tokio::join!(client, server);
    (client.unwrap(), server)
}

#[tokio::test]
async fn streams_partial_then_corrected_final_and_keeps_every_audio_sample() {
    let (client, mut server) = socket_pair().await;
    let (sender, audio) = tokio::sync::mpsc::unbounded_channel();
    for value in 1..=25i16 {
        sender.send(vec![value; crate::audio::FRAME]).unwrap();
    }
    drop(sender);
    let shown = Arc::new(Mutex::new(Vec::new()));
    let partials = shown.clone();
    let recognize = recognize(
        client,
        audio,
        Box::new(move |text| partials.lock().unwrap().push(text.to_owned())),
    );
    let serve = async {
        let Message::Binary(config) = server.next().await.unwrap().unwrap() else {
            panic!("missing config")
        };
        assert_eq!(config[1] >> 4, 1);
        let mut received = Vec::new();
        loop {
            let Message::Binary(message) = server.next().await.unwrap().unwrap() else {
                continue;
            };
            let mut pcm = Vec::new();
            GzDecoder::new(&message[8..]).read_to_end(&mut pcm).unwrap();
            received.extend(
                pcm.chunks_exact(2)
                    .map(|bytes| i16::from_le_bytes(bytes.try_into().unwrap())),
            );
            let last = message[1] & 2 != 0;
            server
                .send(response(if last { "今天开会。" } else { "今天开" }, last))
                .await
                .unwrap();
            if last {
                break;
            }
        }
        received
    };
    let (result, received) = tokio::join!(recognize, serve);
    assert_eq!(result.unwrap(), "今天开会。");
    let expected: Vec<_> = (1..=25i16)
        .flat_map(|value| std::iter::repeat_n(value, crate::audio::FRAME))
        .collect();
    assert_eq!(received, expected);
    assert_eq!(*shown.lock().unwrap(), vec!["今天开", "今天开会。"]);
}

#[tokio::test]
async fn a_broken_connection_does_not_claim_an_incomplete_result_is_final() {
    let (client, mut server) = socket_pair().await;
    let (sender, audio) = tokio::sync::mpsc::unbounded_channel();
    for _ in 0..10 {
        sender.send(vec![1; crate::audio::FRAME]).unwrap();
    }
    let recognize = recognize(client, audio, Box::new(|_| {}));
    let serve = async {
        server.next().await;
        server.next().await;
        server.send(response("还有后半句", false)).await.unwrap();
        server.close(None).await.unwrap();
    };
    let (result, _) = tokio::join!(recognize, serve);
    assert!(result.is_err());
}

#[tokio::test]
async fn missing_key_fails_before_any_audio_is_consumed() {
    let engine = VolcEngine::default();
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    let error = engine.run(receiver, Box::new(|_| {})).await.unwrap_err();
    assert!(error.to_string().contains("配置"));
    assert!(sender.is_closed());
}

#[test]
fn invalid_key_does_not_overwrite_a_working_configuration() {
    let engine = VolcEngine::default();
    engine.set_key("test-only-credential".into()).unwrap();
    assert!(engine.set_key("two\nlines".into()).is_err());
    assert!(engine.configured());
    assert_eq!(
        engine.api_key.read().unwrap().as_str(),
        "test-only-credential"
    );
    engine.clear_key();
    assert!(!engine.configured());
}

#[tokio::test]
async fn finalization_has_a_bounded_wait_after_audio_ends() {
    let (client, mut server) = socket_pair().await;
    let (sender, audio) = tokio::sync::mpsc::unbounded_channel();
    drop(sender);
    let task = tokio::spawn(recognize(client, audio, Box::new(|_| {})));
    server.next().await.unwrap().unwrap();
    server.next().await.unwrap().unwrap();
    tokio::time::pause();
    tokio::time::advance(FINAL_TIMEOUT + Duration::from_secs(1)).await;
    let error = task.await.unwrap().unwrap_err();
    assert!(error.to_string().contains("定稿超时"));
}

#[tokio::test]
async fn cancelling_recognition_releases_its_audio_source() {
    let (client, mut server) = socket_pair().await;
    let (sender, audio) = tokio::sync::mpsc::unbounded_channel();
    let task = tokio::spawn(recognize(client, audio, Box::new(|_| {})));
    server.next().await.unwrap().unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    tokio::time::timeout(Duration::from_secs(1), sender.closed())
        .await
        .unwrap();
    let disconnected = tokio::time::timeout(Duration::from_secs(1), server.next())
        .await
        .unwrap();
    assert!(!matches!(disconnected, Some(Ok(Message::Binary(_)))));
}
