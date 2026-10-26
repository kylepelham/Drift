use std::sync::{Arc, Mutex};

use axum::body::Bytes;
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use base64::Engine as _;
use serde_json::{Value, json};

use super::{
    PROVIDERS, Phrase, Provider, Speech, SpeechVoice, SpokenAudio, Transcription, VoiceError, VoiceGroup, speech,
    transcribe, voices, wav,
};
use crate::Engine;

/// One request the stand-in providers saw.
#[derive(Clone, Debug)]
struct Seen {
    method: Method,
    uri: String,
    auth: String,
    content_type: String,
    body: Vec<u8>,
}

impl Seen {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap()
    }

    /// The value of a multipart field, as the form carries it.
    fn field(&self, name: &str) -> Vec<String> {
        let marker = format!("name=\"{name}\"\r\n\r\n");
        self.text()
            .split(&marker)
            .skip(1)
            .map(|rest| rest.split("\r\n").next().unwrap_or_default().to_string())
            .collect()
    }
}

type Log = Arc<Mutex<Vec<Seen>>>;

fn header(headers: &HeaderMap, name: &str) -> String {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string()
}

/// Recorded answers, one per provider path; the key `bad` is refused and `busy` rate limited.
fn recorded(method: &Method, path: &str, polls: usize) -> Response {
    match (method.as_str(), path) {
        ("POST", "/v1/audio/transcriptions") => {
            json_answer(json!({ "text": " Open the Drift repo. ", "usage": { "type": "tokens" } }))
        }
        ("POST", "/openai/v1/audio/transcriptions") => {
            json_answer(json!({ "text": "Run the tests.", "x_groq": { "id": "req_1" } }))
        }
        ("POST", "/v1/listen") => json_answer(json!({
            "metadata": { "channels": 1 },
            "results": { "channels": [{ "alternatives": [{ "transcript": "Check the build.", "confidence": 0.98 }] }] }
        })),
        ("POST", "/v1/speech-to-text") => json_answer(
            json!({ "language_code": "en", "language_probability": 0.98, "text": "Fix the lint.", "words": [] }),
        ),
        ("POST", "/v2/upload") => json_answer(json!({ "upload_url": "https://cdn.assemblyai.com/upload/abc" })),
        ("POST", "/v2/transcript") => json_answer(json!({ "id": "t1", "status": "queued" })),
        ("GET", "/v2/transcript/t1") if polls < 2 => json_answer(json!({ "id": "t1", "status": "processing" })),
        ("GET", "/v2/transcript/t1") => json_answer(json!({ "id": "t1", "status": "completed", "text": "Ship it." })),
        ("POST", "/v1/audio/speech") => (StatusCode::OK, "ID3-openai").into_response(),
        ("POST", path) if path.starts_with("/v1/text-to-speech/") => (StatusCode::OK, "ID3-elevenlabs").into_response(),
        ("GET", "/v2/voices") => json_answer(voice_page(polls)),
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

/// ElevenLabs' voices in two pages: premade, a clone, a saved library voice, a workspace one.
fn voice_page(page: usize) -> Value {
    if page == 0 {
        return json!({
            "voices": [
                { "voice_id": "JBFqnCBsd6RMkjVDRZzb", "name": "George", "category": "premade",
                  "labels": { "gender": "male", "age": "middle_aged", "accent": "british" } },
                { "voice_id": "mine1", "name": "Kyle", "category": "cloned", "is_owner": true, "labels": {} },
            ],
            "has_more": true,
            "next_page_token": "p2",
        });
    }

    json!({
        "voices": [
            { "voice_id": "lib1", "name": "Narrator", "category": "professional", "is_owner": false,
              "sharing": { "status": "copied" }, "labels": { "accent": "american" } },
            { "voice_id": "team1", "name": "Team", "category": "generated", "is_owner": false },
            { "name": "no id" },
        ],
        "has_more": false,
        "next_page_token": null,
    })
}

fn json_answer(value: Value) -> Response {
    axum::Json(value).into_response()
}

async fn serve() -> (String, Log) {
    let log: Log = Arc::default();
    let seen = log.clone();
    let app = axum::Router::new().fallback(move |method: Method, uri: Uri, headers: HeaderMap, body: Bytes| {
        let seen = seen.clone();
        async move {
            let auth = [header(&headers, "authorization"), header(&headers, "xi-api-key")].concat();
            let polls = seen
                .lock()
                .unwrap()
                .iter()
                .filter(|old| old.method == Method::GET)
                .count();
            seen.lock().unwrap().push(Seen {
                method: method.clone(),
                uri: uri.to_string(),
                auth: auth.clone(),
                content_type: header(&headers, "content-type"),
                body: body.to_vec(),
            });
            if auth.ends_with("bad") {
                return (
                    StatusCode::UNAUTHORIZED,
                    axum::Json(json!({ "error": { "message": "Incorrect API key" } })),
                )
                    .into_response();
            }
            if auth.ends_with("busy") {
                return (StatusCode::TOO_MANY_REQUESTS, "slow down").into_response();
            }
            recorded(&method, uri.path(), polls)
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    (url, log)
}

fn provider(id: &str) -> &'static Provider {
    PROVIDERS.iter().find(|provider| provider.id == id).unwrap()
}

fn phrase(language: Option<&str>) -> Phrase {
    Phrase {
        wav: wav(&[1, 0, 2, 0]),
        language: language.map(str::to_string),
        keyterms: vec!["Drift".into()],
    }
}

async fn heard(id: &str, language: Option<&str>) -> (String, Vec<Seen>) {
    let (url, log) = serve().await;
    let text = transcribe::run(provider(id), "k", &phrase(language), Some(&url))
        .await
        .unwrap();
    let seen = log.lock().unwrap().clone();

    (text, seen)
}

#[tokio::test]
async fn openai_transcribes_with_gpt_transcribe_keywords_and_languages() {
    let (text, seen) = heard("openai", Some("en")).await;

    assert_eq!(text, "Open the Drift repo.");
    assert_eq!(seen[0].uri, "/v1/audio/transcriptions");
    assert_eq!(seen[0].auth, "Bearer k");
    assert!(seen[0].content_type.starts_with("multipart/form-data; boundary="));
    assert_eq!(seen[0].field("model"), ["gpt-transcribe"]);
    assert_eq!(seen[0].field("keywords[]"), ["Drift"]);
    assert_eq!(seen[0].field("languages[]"), ["en"]);
    assert!(
        seen[0].field("language").is_empty(),
        "gpt-transcribe refuses both language fields"
    );
    assert!(
        seen[0]
            .text()
            .contains("filename=\"phrase.wav\"\r\nContent-Type: audio/wav\r\n\r\nRIFF")
    );
}

#[tokio::test]
async fn groq_transcribes_with_whisper_and_a_vocabulary_prompt() {
    let (text, seen) = heard("groq", Some("en")).await;

    assert_eq!(text, "Run the tests.");
    assert_eq!(seen[0].uri, "/openai/v1/audio/transcriptions");
    assert_eq!(seen[0].auth, "Bearer k");
    assert_eq!(seen[0].field("model"), ["whisper-large-v3-turbo"]);
    assert_eq!(seen[0].field("language"), ["en"]);
    assert_eq!(seen[0].field("prompt"), ["Vocabulary: Drift."]);
}

#[tokio::test]
async fn deepgram_takes_the_wav_as_the_body_and_options_in_the_query() {
    let (text, seen) = heard("deepgram", Some("en")).await;

    assert_eq!(text, "Check the build.");
    assert_eq!(
        seen[0].uri,
        "/v1/listen?model=nova-3&smart_format=true&language=en&keyterm=Drift"
    );
    assert_eq!(seen[0].auth, "Token k");
    assert_eq!(seen[0].content_type, "audio/wav");
    assert!(seen[0].body.starts_with(b"RIFF"));

    let (_, detected) = heard("deepgram", None).await;
    assert!(detected[0].uri.contains("detect_language=true") && !detected[0].uri.contains("language=en"));
}

#[tokio::test]
async fn elevenlabs_transcribes_with_scribe_v2_and_no_surcharged_keyterms() {
    let (text, seen) = heard("elevenlabs", Some("en")).await;

    assert_eq!(text, "Fix the lint.");
    assert_eq!(seen[0].uri, "/v1/speech-to-text");
    assert_eq!(seen[0].auth, "k");
    assert_eq!(seen[0].field("model_id"), ["scribe_v2"]);
    assert_eq!(seen[0].field("language_code"), ["en"]);
    assert!(seen[0].field("keyterms").is_empty());
}

#[tokio::test]
async fn assemblyai_uploads_submits_and_polls_until_completed() {
    let (text, seen) = heard("assemblyai", None).await;

    assert_eq!(text, "Ship it.");
    let calls: Vec<String> = seen
        .iter()
        .map(|seen| format!("{} {}", seen.method, seen.uri))
        .collect();
    assert_eq!(
        calls,
        [
            "POST /v2/upload",
            "POST /v2/transcript",
            "GET /v2/transcript/t1",
            "GET /v2/transcript/t1",
            "GET /v2/transcript/t1"
        ]
    );
    assert!(seen.iter().all(|seen| seen.auth == "k"));
    assert!(seen[0].body.starts_with(b"RIFF"));
    let job = seen[1].json();
    assert_eq!(job["audio_url"], "https://cdn.assemblyai.com/upload/abc");
    assert_eq!(job["language_detection"], true);
    assert_eq!(job["keyterms_prompt"], json!(["Drift"]));
}

#[tokio::test]
async fn a_refused_key_or_rate_limit_is_reported_and_nothing_else_is_tried() {
    let (url, log) = serve().await;

    let refused = transcribe::run(provider("openai"), "bad", &phrase(None), Some(&url)).await;
    assert_eq!(refused, Err(VoiceError::Refused("OpenAI", "Incorrect API key".into())));
    let busy = transcribe::run(provider("deepgram"), "busy", &phrase(None), Some(&url)).await;
    assert_eq!(busy, Err(VoiceError::RateLimited("Deepgram")));
    assert_eq!(log.lock().unwrap().len(), 2, "one request each, no fallback");
    assert_eq!(
        refused.unwrap_err().to_string(),
        "OpenAI refused the key: Incorrect API key"
    );
}

async fn spoken(id: &str, voice: Option<&str>) -> (SpokenAudio, Seen) {
    let (url, log) = serve().await;
    let audio = speech::run(provider(id), "k", "Tests pass.", voice, Some(&url))
        .await
        .unwrap();
    let seen = log.lock().unwrap()[0].clone();

    (audio, seen)
}

#[tokio::test]
async fn openai_speech_sends_gpt_4o_mini_tts_with_the_default_voice() {
    let (audio, seen) = spoken("openai", None).await;

    assert_eq!(seen.uri, "/v1/audio/speech");
    assert_eq!(seen.auth, "Bearer k");
    assert_eq!(
        seen.json(),
        json!({ "model": "gpt-4o-mini-tts", "input": "Tests pass.", "voice": speech::OPENAI_VOICE, "response_format": "mp3" })
    );
    assert_eq!(audio.mime, "audio/mpeg");
    assert_eq!(
        base64::engine::general_purpose::STANDARD.decode(audio.audio).unwrap(),
        b"ID3-openai"
    );
}

#[tokio::test]
async fn elevenlabs_lists_every_page_of_the_accounts_voices_by_where_they_come_from() {
    let (url, log) = serve().await;

    let listed = voices::list(provider("elevenlabs"), "k", Some(&url)).await.unwrap();

    let voice = |id: &str, name: &str, group, detail: Option<&str>| SpeechVoice {
        id: id.into(),
        name: name.into(),
        group,
        detail: detail.map(str::to_string),
    };
    assert_eq!(
        listed,
        [
            voice(
                "JBFqnCBsd6RMkjVDRZzb",
                "George",
                VoiceGroup::Default,
                Some("male, middle aged, british")
            ),
            voice("mine1", "Kyle", VoiceGroup::Custom, None),
            voice("lib1", "Narrator", VoiceGroup::Community, Some("american")),
            voice("team1", "Team", VoiceGroup::Workspace, None),
        ]
    );
    let seen = log.lock().unwrap().clone();
    assert_eq!(seen.len(), 2);
    assert_eq!(seen[0].auth, "k");
    assert!(seen[0].uri.contains("page_size=100") && !seen[0].uri.contains("next_page_token"));
    assert!(seen[1].uri.contains("next_page_token=p2"));

    let refused = voices::list(provider("elevenlabs"), "bad", Some(&url)).await;
    assert!(
        matches!(refused, Err(VoiceError::Refused("ElevenLabs", _))),
        "{refused:?}"
    );
    assert!(matches!(
        voices::list(provider("openai"), "k", Some(&url)).await,
        Err(VoiceError::Unknown(_))
    ));
}

#[tokio::test]
async fn elevenlabs_speech_names_the_voice_in_the_path() {
    let (audio, seen) = spoken("elevenlabs", None).await;

    assert_eq!(
        seen.uri,
        format!(
            "/v1/text-to-speech/{}?output_format=mp3_44100_128",
            speech::ELEVENLABS_VOICE
        )
    );
    assert_eq!(seen.auth, "k");
    assert_eq!(
        seen.json(),
        json!({ "text": "Tests pass.", "model_id": "eleven_flash_v2_5" })
    );
    assert_eq!(
        base64::engine::general_purpose::STANDARD.decode(audio.audio).unwrap(),
        b"ID3-elevenlabs"
    );

    let (_, chosen) = spoken("elevenlabs", Some("abc123")).await;
    assert!(chosen.uri.starts_with("/v1/text-to-speech/abc123?"));
}

fn engine() -> Arc<Engine> {
    let directory = crate::platform::temp::test_run().join(format!("drift-voice-{}", crate::random_hex(4)));
    let options = crate::Options {
        file_credentials: true,
        ..Default::default()
    };

    Engine::open_with(&directory, options).unwrap()
}

#[tokio::test]
async fn keys_are_saved_shown_by_their_last_four_and_required() {
    let engine = engine();
    let request = Transcription {
        provider: "groq".into(),
        audio: base64::engine::general_purpose::STANDARD.encode([1, 0]),
        language: Some("auto".into()),
        keyterms: Vec::new(),
    };
    assert!(engine.voice_providers().iter().all(|provider| provider.key.is_none()));
    assert_eq!(engine.voice_transcribe(&request).await, Err(VoiceError::NoKey("Groq")));

    engine.set_voice_key("groq", "  gsk_secret1234 ").unwrap();
    let listed = engine.voice_providers();
    let groq = listed.iter().find(|provider| provider.id == "groq").unwrap();
    assert_eq!(groq.key.as_deref(), Some("...1234"));
    assert!(groq.transcription && !groq.speech);

    engine.remove_voice_key("groq").unwrap();
    assert_eq!(engine.voice_transcribe(&request).await, Err(VoiceError::NoKey("Groq")));
    assert_eq!(
        engine.set_voice_key("nope", "k"),
        Err(VoiceError::Unknown("nope".into()))
    );
    assert_eq!(
        engine.set_voice_key("groq", " "),
        Err(VoiceError::Invalid("the key is empty".into()))
    );
}

#[tokio::test]
async fn requests_the_provider_cannot_take_are_refused_before_sending() {
    let engine = engine();
    engine.set_voice_key("groq", "k").unwrap();
    engine.set_voice_key("elevenlabs", "k").unwrap();
    let speak = |provider: &str, text: &str, voice: Option<&str>| Speech {
        provider: provider.into(),
        text: text.into(),
        voice: voice.map(str::to_string),
    };

    let unspoken = engine.voice_speak(&speak("groq", "hi", None)).await;
    assert_eq!(unspoken, Err(VoiceError::Unknown("groq".into())));
    let long = "a".repeat(super::MAX_SPEECH_CHARS + 1);
    assert!(matches!(
        engine.voice_speak(&speak("elevenlabs", &long, None)).await,
        Err(VoiceError::Invalid(_))
    ));
    let escaping = engine.voice_speak(&speak("elevenlabs", "hi", Some("../voices"))).await;
    assert!(matches!(escaping, Err(VoiceError::Invalid(_))));

    let silent = Transcription {
        provider: "groq".into(),
        audio: String::new(),
        language: None,
        keyterms: Vec::new(),
    };
    assert!(matches!(
        engine.voice_transcribe(&silent).await,
        Err(VoiceError::Invalid(_))
    ));
}

#[test]
fn a_phrase_becomes_a_16_khz_wav_and_auto_means_detect() {
    let request = Transcription {
        provider: "openai".into(),
        audio: base64::engine::general_purpose::STANDARD.encode([1, 0, 2, 0]),
        language: Some("auto".into()),
        keyterms: vec![" Drift ".into(), " ".into()],
    };
    let phrase = Phrase::from_request(&request).unwrap();

    assert_eq!(phrase.language, None);
    assert_eq!(phrase.keyterms, ["Drift"]);
    assert_eq!(phrase.wav.len(), 48);
    assert_eq!(&phrase.wav[..4], b"RIFF");
    assert_eq!(u32::from_le_bytes(phrase.wav[24..28].try_into().unwrap()), 16_000);
    assert_eq!(&phrase.wav[44..], [1, 0, 2, 0]);
}
