use super::*;

fn response(text: &str, sequence: i32, compressed: bool) -> Vec<u8> {
    let payload = serde_json::to_vec(&json!({"result": {"text": text}})).unwrap();
    let mut message = packet(9, if sequence < 0 { 3 } else { 1 }, 1, &payload).unwrap();
    if !compressed {
        message[2] = 0x10;
        message.truncate(4);
        message.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        message.extend_from_slice(&payload);
    }
    message.splice(4..4, sequence.to_be_bytes());
    message
}

fn payload(message: &[u8]) -> Vec<u8> {
    assert_eq!(
        u32::from_be_bytes(message[4..8].try_into().unwrap()) as usize,
        message.len() - 8
    );
    let mut output = Vec::new();
    GzDecoder::new(&message[8..])
        .read_to_end(&mut output)
        .unwrap();
    output
}

#[test]
fn request_uses_pcm_and_full_realtime_results_with_final_correction() {
    let message = config().unwrap();
    assert_eq!(&message[..4], &[0x11, 0x10, 0x11, 0]);
    let config: Value = serde_json::from_slice(&payload(&message)).unwrap();
    assert_eq!(config["audio"]["format"], "pcm");
    assert_eq!(config["audio"]["rate"], 16000);
    assert_eq!(config["request"]["result_type"], "full");
    assert_eq!(config["request"]["enable_nonstream"], true);
}

#[test]
fn audio_keeps_signed_samples_in_little_endian_and_marks_the_last_packet() {
    let samples = [-32768i16, -12, 0, 23, 32767];
    let message = audio(&samples, true).unwrap();
    assert_eq!(&message[..4], &[0x11, 0x22, 0x01, 0]);
    assert_eq!(
        payload(&message),
        samples
            .iter()
            .flat_map(|sample| sample.to_le_bytes())
            .collect::<Vec<_>>()
    );
    assert!(payload(&audio(&[], true).unwrap()).is_empty());
}

#[test]
fn parses_compressed_and_plain_responses_and_final_sequences() {
    for compressed in [false, true] {
        for sequence in [1, -1] {
            let result = parse(&response("今天开会。", sequence, compressed)).unwrap();
            assert_eq!(result.text.as_deref(), Some("今天开会。"));
            assert_eq!(result.last, sequence < 0);
        }
    }
}

#[test]
fn truncated_messages_never_panic_or_look_like_a_success() {
    let message = response("完整的一句。", -1, true);
    for length in 0..message.len() {
        assert!(parse(&message[..length]).is_err(), "length={length}");
    }
    let mut oversized = message.clone();
    oversized[8..12].copy_from_slice(&u32::MAX.to_be_bytes());
    assert!(parse(&oversized).is_err());
}

#[test]
fn errors_never_echo_remote_text_or_credentials() {
    let mut message = vec![0x11, 0xf0, 0, 0];
    message.extend_from_slice(&45000000u32.to_be_bytes());
    let secret = b"fake-private-key-for-redaction-test";
    message.extend_from_slice(&(secret.len() as u32).to_be_bytes());
    message.extend_from_slice(secret);
    let error = parse(&message).err().unwrap().to_string();
    assert!(error.contains("45000000"));
    assert!(!error.contains("private-key"));
}

#[test]
fn rejects_unknown_compression_and_invalid_json() {
    let mut message = response("你好", 1, false);
    message[2] = 0x17;
    assert!(parse(&message).is_err());
    let mut message = response("你好", 1, false);
    let end = message.len() - 1;
    message[end] = b'!';
    assert!(parse(&message).is_err());
}

#[test]
fn decompression_has_a_bounded_output_size() {
    let oversized = vec![b' '; MAX_PAYLOAD + 1];
    let mut message = packet(9, 1, 1, &oversized).unwrap();
    message.splice(4..4, 1i32.to_be_bytes());
    assert!(parse(&message)
        .err()
        .unwrap()
        .to_string()
        .contains("大小限制"));
}
