//! Each transcription provider's wire shape for one finished phrase.

use std::time::Duration;

use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use serde_json::{Value, json};

use super::{Phrase, Provider, VoiceError, answer_json};

/// AssemblyAI transcribes after upload; a short phrase is ready within seconds.
const POLL: Duration = Duration::from_millis(500);
const MAX_POLLS: usize = 240;
/// Query values keep the characters URLs leave alone.
const QUERY: &AsciiSet = &NON_ALPHANUMERIC.remove(b'-').remove(b'_').remove(b'.').remove(b'~');

/// A multipart form, built by hand so the HTTP client needs no extra feature for five small bodies.
struct Form {
    boundary: String,
    body: Vec<u8>,
}

impl Form {
    fn new() -> Self {
        Self {
            boundary: format!("drift-{}", crate::random_hex(12)),
            body: Vec::new(),
        }
    }

    fn head(&mut self, disposition: &str, mime: Option<&str>) {
        let kind = mime.map(|mime| format!("Content-Type: {mime}\r\n")).unwrap_or_default();
        let head = format!(
            "--{}\r\nContent-Disposition: form-data; {disposition}\r\n{kind}\r\n",
            self.boundary
        );
        self.body.extend_from_slice(head.as_bytes());
    }

    fn text(mut self, name: &str, value: &str) -> Self {
        self.head(&format!("name=\"{name}\""), None);
        self.body.extend_from_slice(value.as_bytes());
        self.body.extend_from_slice(b"\r\n");
        self
    }

    fn texts<'a>(self, name: &str, values: impl IntoIterator<Item = &'a String>) -> Self {
        values.into_iter().fold(self, |form, value| form.text(name, value))
    }

    fn wav(mut self, wav: &[u8]) -> Self {
        self.head("name=\"file\"; filename=\"phrase.wav\"", Some("audio/wav"));
        self.body.extend_from_slice(wav);
        self.body.extend_from_slice(b"\r\n");
        self
    }

    fn attach(mut self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        self.body
            .extend_from_slice(format!("--{}--\r\n", self.boundary).as_bytes());
        request
            .header(
                "content-type",
                format!("multipart/form-data; boundary={}", self.boundary),
            )
            .body(self.body)
    }
}

fn url(base: Option<&str>, host: &str, path: &str) -> String {
    format!("{}{path}", base.unwrap_or(host))
}

fn text_of(name: &'static str, answer: &Value, pointer: &str) -> Result<String, VoiceError> {
    answer
        .pointer(pointer)
        .and_then(Value::as_str)
        .map(|text| text.trim().to_string())
        .ok_or_else(|| VoiceError::Failed(name, "the answer held no transcript".into()))
}

pub(super) async fn run(
    provider: &Provider,
    key: &str,
    phrase: &Phrase,
    base: Option<&str>,
) -> Result<String, VoiceError> {
    match provider.id {
        "openai" => openai(key, phrase, base).await,
        "groq" => groq(key, phrase, base).await,
        "deepgram" => deepgram(key, phrase, base).await,
        "elevenlabs" => elevenlabs(key, phrase, base).await,
        "assemblyai" => assemblyai(key, phrase, base).await,
        other => Err(VoiceError::Unknown(other.into())),
    }
}

/// `gpt-transcribe` takes expected words as `keywords[]` and languages as `languages[]`, never the singular field.
async fn openai(key: &str, phrase: &Phrase, base: Option<&str>) -> Result<String, VoiceError> {
    let form = Form::new()
        .text("model", "gpt-transcribe")
        .texts("keywords[]", &phrase.keyterms)
        .texts("languages[]", &phrase.language)
        .wav(&phrase.wav);
    let request = crate::llm::http::client()
        .post(url(base, "https://api.openai.com", "/v1/audio/transcriptions"))
        .bearer_auth(key);
    let answer = answer_json("OpenAI", form.attach(request)).await?;

    text_of("OpenAI", &answer, "/text")
}

/// Groq serves whisper, which biases toward a prompt rather than a word list.
async fn groq(key: &str, phrase: &Phrase, base: Option<&str>) -> Result<String, VoiceError> {
    let prompt = (!phrase.keyterms.is_empty()).then(|| format!("Vocabulary: {}.", phrase.keyterms.join(", ")));
    let form = Form::new()
        .text("model", "whisper-large-v3-turbo")
        .text("response_format", "json")
        .texts("language", &phrase.language)
        .texts("prompt", &prompt)
        .wav(&phrase.wav);
    let request = crate::llm::http::client()
        .post(url(base, "https://api.groq.com", "/openai/v1/audio/transcriptions"))
        .bearer_auth(key);
    let answer = answer_json("Groq", form.attach(request)).await?;

    text_of("Groq", &answer, "/text")
}

/// Deepgram reads the WAV as the body, with every option in the query.
async fn deepgram(key: &str, phrase: &Phrase, base: Option<&str>) -> Result<String, VoiceError> {
    let mut query = vec![("model", "nova-3".to_string()), ("smart_format", "true".into())];
    match &phrase.language {
        Some(code) => query.push(("language", code.clone())),
        None => query.push(("detect_language", "true".into())),
    }
    query.extend(phrase.keyterms.iter().map(|term| ("keyterm", term.clone())));
    let query: Vec<String> = query
        .iter()
        .map(|(name, value)| format!("{name}={}", utf8_percent_encode(value, QUERY)))
        .collect();
    let path = format!("/v1/listen?{}", query.join("&"));
    let request = crate::llm::http::client()
        .post(url(base, "https://api.deepgram.com", &path))
        .header("authorization", format!("Token {key}"))
        .header("content-type", "audio/wav")
        .body(phrase.wav.clone());
    let answer = answer_json("Deepgram", request).await?;

    text_of("Deepgram", &answer, "/results/channels/0/alternatives/0/transcript")
}

/// Scribe's keyterms carry a surcharge and refuse brackets, so the vocabulary is not sent to it.
async fn elevenlabs(key: &str, phrase: &Phrase, base: Option<&str>) -> Result<String, VoiceError> {
    let form = Form::new()
        .text("model_id", "scribe_v2")
        .text("tag_audio_events", "false")
        .texts("language_code", &phrase.language)
        .wav(&phrase.wav);
    let request = crate::llm::http::client()
        .post(url(base, "https://api.elevenlabs.io", "/v1/speech-to-text"))
        .header("xi-api-key", key);
    let answer = answer_json("ElevenLabs", form.attach(request)).await?;

    text_of("ElevenLabs", &answer, "/text")
}

/// AssemblyAI takes an upload, then a transcript job that is polled until it completes or fails.
async fn assemblyai(key: &str, phrase: &Phrase, base: Option<&str>) -> Result<String, VoiceError> {
    const NAME: &str = "AssemblyAI";
    let host = "https://api.assemblyai.com";
    let client = crate::llm::http::client();
    let uploaded = answer_json(
        NAME,
        client
            .post(url(base, host, "/v2/upload"))
            .header("authorization", key)
            .header("content-type", "application/octet-stream")
            .body(phrase.wav.clone()),
    )
    .await?;
    let audio_url = text_of(NAME, &uploaded, "/upload_url")?;

    let mut job = json!({ "audio_url": audio_url });
    match &phrase.language {
        Some(code) => job["language_code"] = json!(code),
        None => job["language_detection"] = json!(true),
    }
    if !phrase.keyterms.is_empty() {
        job["keyterms_prompt"] = json!(phrase.keyterms);
    }
    let created = answer_json(
        NAME,
        client
            .post(url(base, host, "/v2/transcript"))
            .header("authorization", key)
            .json(&job),
    )
    .await?;
    let id = text_of(NAME, &created, "/id")?;

    poll_assemblyai(key, &url(base, host, &format!("/v2/transcript/{id}"))).await
}

async fn poll_assemblyai(key: &str, url: &str) -> Result<String, VoiceError> {
    const NAME: &str = "AssemblyAI";
    for _ in 0..MAX_POLLS {
        let job = answer_json(NAME, crate::llm::http::client().get(url).header("authorization", key)).await?;
        match job["status"].as_str() {
            Some("completed") => return Ok(job["text"].as_str().unwrap_or_default().trim().to_string()),
            Some("error") => {
                let why = job["error"].as_str().unwrap_or("the transcript failed");
                return Err(VoiceError::Failed(NAME, why.to_string()));
            }
            _ => tokio::time::sleep(POLL).await,
        }
    }

    Err(VoiceError::Failed(NAME, "the transcript took too long".into()))
}
