# Voice

Dictation lets you speak a prompt into the composer instead of typing it. Everything runs on
this machine: there is no API key, no account, and no audio leaves the computer.

It is off by default. Speaking to an agent is a minority habit and the feature needs a model
download before it does anything, so the microphone button does not exist until it is turned
on in Settings > Voice. Nothing is fetched, and no shell command is issued, while it is off.

Settings live under Settings > Voice. That section is named for the capability rather than
for dictation specifically, because speech output belongs beside speech input when it lands.

## Why a sidecar rather than a Rust binding

The obvious way to embed whisper.cpp is `whisper-rs`, but on Windows it requires LLVM and
`LIBCLANG_PATH` because it generates bindings with `bindgen`. That would have landed in the
local build, the Windows CI job, and the release build job, and bindgen with libclang on
`windows-latest` has a documented history of breaking builds.

Calling whisper.cpp as a subprocess removes bindgen entirely. `scripts/build-whisper.ts`
compiles it with CMake and MSVC, which the release runners already have and which the Rust
build already needs, and drops the result into `src-tauri/binaries` next to the engine. It is
declared in `externalBin` and resolved the same way `engine_binary` resolves the sidecar:
beside the executable once bundled, from `binaries` during development.

Each binary is statically linked, so it is a single file with no DLLs beside it, and it is
bundled in the authenticated installer like the other runtime components.

## Why two sidecars

Transcription is built twice, and the difference is not marginal. Measured on a six second
phrase with the default `large-v3-turbo-q5_0` model:

| Backend | Time |
| --- | --- |
| Vulkan | 0.98s |
| CPU | 10.7s |

The CPU is slower than real time, which makes the best model unusable without a GPU. Vulkan
is used rather than CUDA because it is vendor neutral, covering NVIDIA, AMD and Intel from
one binary, and because a CUDA build would need the CUDA toolkit at build time and its runtime
DLLs beside the executable.

The GPU binary is 72MB against 1.8MB for the CPU one, because the SPIR-V compute shaders are
embedded, and it imports `vulkan-1.dll` at load time so it cannot start on a machine with no
graphics driver. Both are therefore shipped. `whisper_binary` picks the GPU build when
`vulkan-1.dll` is present, and `voice_transcribe` retries once on the CPU build if the GPU
attempt fails, which covers a driver that advertises Vulkan but cannot actually run it.

On a machine with no GPU the smaller models remain perfectly usable: `base-q5_1` transcribes
the same phrase in 0.87s on the CPU. Settings > Voice reports which backend is active so the
choice of model is an informed one.

Building the GPU sidecar needs the Vulkan SDK for `glslc`, which compiles the shaders.
`scripts/build-whisper.ts` detects it and skips the GPU build with a warning when it is
absent, so a contributor without the SDK still gets a working CPU build.

## Why the model is not bundled or auto-fetched

Drift's installer is 56MB. The recommended model is 547MB, so bundling it is not an option.
It is also never downloaded on your behalf: the first time you click the microphone, Voice
settings opens and shows the download and its size. Nothing is fetched until you press the
button.

Three tiers are offered, all from the whisper.cpp model repository, which publishes a SHA1
for every file. The hash is computed while the file streams to disk and checked before the
download is accepted, and the file is written to a `.part` name and only renamed on success,
so an interrupted download can never look installed.

## How a phrase becomes text

Whisper is not a streaming model. It reads 30 second windows, which is why continuous
streaming implementations resort to chunking and stitching. Dictation does not need that,
because speech already arrives in phrases separated by pauses.

- `src/voice/capture.ts` opens the microphone in an `AudioContext` fixed at 16 kHz, which is
  what whisper wants, so nothing has to be resampled afterwards. An `AudioWorklet` loaded from
  a blob URL posts frames back, and they are gathered into uniform 32ms blocks.
- `src/voice/audio.ts` is the segmenter. It measures the energy of each block, requires a
  run of voiced blocks before it believes speech started, keeps a short pre-roll so the first
  word is not clipped, and closes the phrase after a run of silence. It is pure and unit
  tested, including the cases that matter most: silence alone never produces a phrase, and a
  brief knock is not mistaken for speech.
- `src/voice/dictation.ts` sends each finished phrase to the sidecar. Phrases are transcribed
  one at a time through a promise chain so they reach the draft in the order they were spoken.
- `src-tauri/src/voice.rs` writes the samples to a temporary WAV, runs the sidecar, and
  returns the text.

Gating on the segmenter is what keeps whisper from hallucinating. It invents filler during
silence, so silence is never sent to it. `cleanTranscript` is the second line of defence and
drops a result that is nothing but a bracketed marker such as `[BLANK_AUDIO]`.

## Decisions

- **Only finalized speech is written to the draft.** The draft only ever grows, so typing and
  speaking cannot fight over the textarea.
- **Silence never sends.** A pause ends a phrase; it does not submit the message.
- **Custom vocabulary is a whisper prompt.** Whisper biases decoding toward an initial prompt,
  so repo names, library names, and CLI flags are passed as `--prompt` rather than being
  hoped for.
- **The download is cancellable.** A 547MB fetch that cannot be stopped is hostile, so the
  command polls an atomic flag and the partial file is removed on cancel.

## Limits

The very first transcription after installing takes around nine seconds while the graphics
driver compiles and caches the Vulkan pipelines. That cost is paid once per machine, not once
per session, and every run afterwards is about a second.

The model is loaded for each phrase because the sidecar is invoked per utterance. That is
included in the timings above and is comfortably fast enough, but running whisper.cpp in its
server mode would keep the model resident and cut the remaining latency further.

Dictation itself can run as long as you like, because finalized text is committed to the
draft as it arrives rather than held in memory. The finished message is still bounded by the
context window of the model you send it to.
