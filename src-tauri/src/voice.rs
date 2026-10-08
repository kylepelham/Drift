//! Local speech to text. Models are fetched only when asked for, and transcription runs in the
//! bundled whisper.cpp sidecar, so recorded audio never leaves the machine.

use base64::Engine as _;
use serde::Serialize;
use sha1::{Digest, Sha1};
use std::fmt::Write as _;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{Emitter, Manager, State};

const HOST: &str = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main";
const PROGRESS_EVENT: &str = "voice-model-progress";
/// Whisper only accepts 16 kHz mono, so the webview resamples before sending.
const SAMPLE_RATE: u32 = 16_000;
/// A dictated phrase is seconds long; more than this means the caller sent the wrong buffer.
const MAX_AUDIO_BYTES: usize = SAMPLE_RATE as usize * 2 * 180;
const PROGRESS_STEP: u64 = 2 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
enum VoiceError {
    #[error("unknown voice model: {0}")]
    UnknownModel(String),
    #[error("model download failed with status {0}")]
    DownloadStatus(reqwest::StatusCode),
    #[error("cancelled")]
    Cancelled,
    #[error("downloaded model failed its checksum")]
    Checksum,
    #[error("the speech model is not downloaded")]
    ModelMissing,
    #[error("the speech recognizer is missing from this build")]
    RecognizerMissing,
    #[error("unusable audio length")]
    InvalidAudio,
    #[error("{0}")]
    Transcription(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Http(#[from] reqwest::Error),
    #[error(transparent)]
    Tauri(#[from] tauri::Error),
    #[error(transparent)]
    Audio(#[from] base64::DecodeError),
}

struct ModelSpec {
    id: &'static str,
    file: &'static str,
    sha1: &'static str,
    bytes: u64,
}

/// Sizes and hashes are published by the whisper.cpp model repository and verified after download.
const MODELS: &[ModelSpec] = &[
    ModelSpec {
        id: "large-v3-turbo-q5_0",
        file: "ggml-large-v3-turbo-q5_0.bin",
        sha1: "e050f7970618a659205450ad97eb95a18d69c9ee",
        bytes: 573_571_072,
    },
    ModelSpec {
        id: "small-q5_1",
        file: "ggml-small-q5_1.bin",
        sha1: "6fe57ddcfdd1c6b07cdcc73aaf620810ce5fc771",
        bytes: 189_792_256,
    },
    ModelSpec {
        id: "base-q5_1",
        file: "ggml-base-q5_1.bin",
        sha1: "a3733eda680ef76256db5fc5dd9de8629e62c5e7",
        bytes: 59_768_832,
    },
];

#[derive(Default)]
pub(crate) struct VoiceDownload(AtomicBool);

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct VoiceModel {
    id: String,
    bytes: u64,
    installed: bool,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct VoiceProgress {
    id: String,
    received: u64,
    total: u64,
}

fn spec(id: &str) -> Result<&'static ModelSpec, VoiceError> {
    MODELS
        .iter()
        .find(|model| model.id == id)
        .ok_or_else(|| VoiceError::UnknownModel(id.to_string()))
}

fn model_dir(app: &tauri::AppHandle) -> Result<PathBuf, VoiceError> {
    let dir = app.path().app_data_dir()?.join("voice-models");
    std::fs::create_dir_all(&dir)?;

    Ok(dir)
}

fn model_path(app: &tauri::AppHandle, model: &ModelSpec) -> Result<PathBuf, VoiceError> {
    Ok(model_dir(app)?.join(model.file))
}

/// The sidecar sits beside the executable once bundled, and in `binaries` during development.
fn sidecar(name: &str) -> Option<PathBuf> {
    let file = format!("{name}.exe");
    if let Some(bundled) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(&file)))
        .filter(|path| path.exists())
    {
        return Some(bundled);
    }
    let dev = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("binaries")
        .join(&file);
    dev.exists().then_some(dev)
}

/// The GPU sidecar imports `vulkan-1.dll` at load time, so it cannot start without a driver.
fn vulkan_ready() -> bool {
    std::env::var_os("SystemRoot")
        .map(|root| PathBuf::from(root).join("System32").join("vulkan-1.dll"))
        .is_some_and(|path| path.exists())
}

pub(crate) fn whisper_binary() -> Option<PathBuf> {
    if vulkan_ready()
        && let Some(accelerated) = sidecar("whisper-cli-vulkan")
    {
        return Some(accelerated);
    }
    sidecar("whisper-cli")
}

#[tauri::command]
pub(crate) fn voice_supported() -> bool {
    whisper_binary().is_some()
}

#[tauri::command]
pub(crate) fn voice_acceleration() -> bool {
    vulkan_ready() && sidecar("whisper-cli-vulkan").is_some()
}

#[tauri::command]
pub(crate) fn voice_models(app: tauri::AppHandle) -> Result<Vec<VoiceModel>, String> {
    let dir = model_dir(&app).map_err(|error| error.to_string())?;
    Ok(MODELS
        .iter()
        .map(|model| VoiceModel {
            id: model.id.to_string(),
            bytes: model.bytes,
            installed: dir.join(model.file).is_file(),
        })
        .collect())
}

#[tauri::command]
pub(crate) fn voice_model_remove(app: tauri::AppHandle, id: String) -> Result<(), String> {
    let model = spec(&id).map_err(|error| error.to_string())?;
    let path = model_path(&app, model).map_err(|error| error.to_string())?;
    if !path.exists() {
        return Ok(());
    }
    std::fs::remove_file(path).map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) fn voice_model_cancel(download: State<VoiceDownload>) {
    download.0.store(true, Ordering::Relaxed);
}

#[tauri::command]
pub(crate) async fn voice_model_download(
    app: tauri::AppHandle,
    download: State<'_, VoiceDownload>,
    id: String,
) -> Result<(), String> {
    download_model(&app, &download, &id)
        .await
        .map_err(|error| error.to_string())
}

async fn download_model(app: &tauri::AppHandle, download: &VoiceDownload, id: &str) -> Result<(), VoiceError> {
    let model = spec(id)?;
    let destination = model_path(app, model)?;
    if destination.is_file() {
        return Ok(());
    }
    download.0.store(false, Ordering::Relaxed);
    let partial = destination.with_extension("part");
    let result = fetch_model(app, download, model, &partial).await;
    if result.is_err() {
        let _ = std::fs::remove_file(&partial);
        return result;
    }
    Ok(std::fs::rename(&partial, &destination)?)
}

async fn fetch_model(
    app: &tauri::AppHandle,
    download: &VoiceDownload,
    model: &ModelSpec,
    partial: &PathBuf,
) -> Result<(), VoiceError> {
    let response = reqwest::get(format!("{HOST}/{}", model.file)).await?;
    if !response.status().is_success() {
        return Err(VoiceError::DownloadStatus(response.status()));
    }
    let total = response.content_length().unwrap_or(model.bytes);
    let mut file = std::fs::File::create(partial)?;
    let mut hasher = Sha1::new();
    let mut received = 0u64;
    let mut announced = 0u64;
    let mut response = response;
    while let Some(chunk) = response.chunk().await? {
        if download.0.load(Ordering::Relaxed) {
            return Err(VoiceError::Cancelled);
        }
        hasher.update(&chunk);
        file.write_all(&chunk)?;
        received += chunk.len() as u64;
        if received - announced < PROGRESS_STEP && received < total {
            continue;
        }
        announced = received;
        let _ = app.emit(
            PROGRESS_EVENT,
            VoiceProgress {
                id: model.id.to_string(),
                received,
                total,
            },
        );
    }
    file.flush()?;
    let digest = hasher.finalize();
    let mut actual = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(actual, "{byte:02x}").unwrap();
    }
    if actual != model.sha1 {
        return Err(VoiceError::Checksum);
    }
    Ok(())
}

#[tauri::command]
pub(crate) async fn voice_transcribe(
    app: tauri::AppHandle,
    id: String,
    audio: String,
    language: String,
    prompt: String,
) -> Result<String, String> {
    transcribe(&app, &id, audio, language, prompt)
        .await
        .map_err(|error| error.to_string())
}

async fn transcribe(
    app: &tauri::AppHandle,
    id: &str,
    audio: String,
    language: String,
    prompt: String,
) -> Result<String, VoiceError> {
    let model = spec(id)?;
    let model_file = model_path(app, model)?;
    if !model_file.is_file() {
        return Err(VoiceError::ModelMissing);
    }
    let binary = whisper_binary().ok_or(VoiceError::RecognizerMissing)?;
    let fallback = sidecar("whisper-cli").filter(|path| *path != binary);
    let samples = base64::engine::general_purpose::STANDARD.decode(audio)?;
    if samples.is_empty() || samples.len() > MAX_AUDIO_BYTES {
        return Err(VoiceError::InvalidAudio);
    }
    tauri::async_runtime::spawn_blocking(move || {
        let attempt = run_whisper(&binary, &model_file, &samples, &language, &prompt);
        // A driver that reports Vulkan but cannot run it still leaves the CPU sidecar usable.
        match (attempt, fallback) {
            (Err(error), Some(cpu)) => {
                eprintln!("voice: GPU transcription failed ({error}); retrying on the CPU sidecar");
                run_whisper(&cpu, &model_file, &samples, &language, &prompt)
            }
            (attempt, _) => attempt,
        }
    })
    .await?
}

fn run_whisper(
    binary: &PathBuf,
    model: &PathBuf,
    samples: &[u8],
    language: &str,
    prompt: &str,
) -> Result<String, VoiceError> {
    let wav = std::env::temp_dir().join(format!(
        "drift-voice-{}-{:?}.wav",
        std::process::id(),
        std::thread::current().id()
    ));
    write_wav(&wav, samples)?;
    let mut command = std::process::Command::new(binary);
    command
        .arg("-m")
        .arg(model)
        .arg("-f")
        .arg(&wav)
        .args(["-l", language])
        .args(["-nt", "-np", "-sns"]);
    if !prompt.is_empty() {
        command.args(["--prompt", prompt]);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(crate::CREATE_NO_WINDOW);
    }
    let output = command.output();
    let _ = std::fs::remove_file(&wav);
    let output = output?;
    if !output.status.success() {
        return Err(VoiceError::Transcription(
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Minimal 16-bit mono PCM header; the sidecar reads files rather than a stream.
fn write_wav(path: &PathBuf, samples: &[u8]) -> std::io::Result<()> {
    let mut file = std::fs::File::create(path)?;
    let data = samples.len() as u32;
    let byte_rate = SAMPLE_RATE * 2;
    let mut header = Vec::with_capacity(44);
    header.extend_from_slice(b"RIFF");
    header.extend_from_slice(&(36 + data).to_le_bytes());
    header.extend_from_slice(b"WAVEfmt ");
    header.extend_from_slice(&16u32.to_le_bytes());
    header.extend_from_slice(&1u16.to_le_bytes());
    header.extend_from_slice(&1u16.to_le_bytes());
    header.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    header.extend_from_slice(&byte_rate.to_le_bytes());
    header.extend_from_slice(&2u16.to_le_bytes());
    header.extend_from_slice(&16u16.to_le_bytes());
    header.extend_from_slice(b"data");
    header.extend_from_slice(&data.to_le_bytes());
    file.write_all(&header)?;
    file.write_all(samples)?;
    file.flush()
}

#[cfg(test)]
#[path = "voice_tests.rs"]
mod tests;
