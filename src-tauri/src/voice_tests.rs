use super::*;

#[test]
fn only_known_models_resolve() {
    assert!(spec("large-v3-turbo-q5_0").is_ok());
    assert!(spec("../../etc/passwd").is_err());
    assert!(spec("").is_err());
}

#[test]
fn every_model_publishes_a_sha1_and_a_ggml_file() {
    for model in MODELS {
        assert_eq!(model.sha1.len(), 40, "{} has a malformed sha1", model.id);
        assert!(model.sha1.chars().all(|c| c.is_ascii_hexdigit()));
        assert!(model.file.starts_with("ggml-") && model.file.ends_with(".bin"));
        assert!(model.bytes > 0);
    }
}

#[test]
fn model_ids_are_unique() {
    let mut ids: Vec<&str> = MODELS.iter().map(|model| model.id).collect();
    ids.sort_unstable();
    let count = ids.len();
    ids.dedup();
    assert_eq!(ids.len(), count);
}

#[test]
fn the_wav_header_describes_16khz_mono_pcm() {
    let path = std::env::temp_dir().join(format!("drift-voice-header-{}.wav", std::process::id()));
    let samples = vec![0u8; 8];
    write_wav(&path, &samples).expect("wav should be written");
    let written = std::fs::read(&path).expect("wav should be readable");
    std::fs::remove_file(&path).ok();

    assert_eq!(&written[0..4], b"RIFF");
    assert_eq!(&written[8..12], b"WAVE");
    assert_eq!(u32::from_le_bytes(written[4..8].try_into().unwrap()), 36 + 8);
    assert_eq!(u16::from_le_bytes(written[20..22].try_into().unwrap()), 1);
    assert_eq!(u16::from_le_bytes(written[22..24].try_into().unwrap()), 1);
    assert_eq!(u32::from_le_bytes(written[24..28].try_into().unwrap()), SAMPLE_RATE);
    assert_eq!(u16::from_le_bytes(written[34..36].try_into().unwrap()), 16);
    assert_eq!(&written[36..40], b"data");
    assert_eq!(u32::from_le_bytes(written[40..44].try_into().unwrap()), 8);
    assert_eq!(written.len(), 44 + 8);
}
