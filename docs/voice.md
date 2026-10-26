# Voice

Settings > Voice holds speech in both directions: dictation into the composer, and replies read
aloud. Both work entirely on this machine by default. A cloud provider is used only after you pick
it in Settings and save its key, and the table below says exactly what each choice sends away.

Dictation is off by default. Speaking to an agent is a minority habit and local dictation needs a
model download before it does anything, so the microphone button does not exist until it is turned
on in Settings > Voice. Nothing is fetched, and no shell command is issued, while it is off. Read
aloud is off by default too: no play button appears and nothing is spoken until it is turned on.

## What leaves the machine

| Choice | What is sent | To whom |
| --- | --- | --- |
| Dictation, On this computer (whisper) | Nothing | |
| Dictation, OpenAI, Groq, Deepgram, ElevenLabs or AssemblyAI | Each recorded phrase as a WAV, its language when one is set, and your custom vocabulary (except to ElevenLabs) | That provider, with your key |
| Read aloud, This computer's voice | Nothing; only voices the system marks as local are offered | |
| Read aloud, OpenAI or ElevenLabs | The prose of each reply you play or that is read automatically, in pieces of up to 2,500 characters | That provider, with your key |

Keys are kept in the same credential store as provider sign-ins, are shown back only by their last
four characters, and are never read by the webview. No other provider is ever tried in place of
the one you picked.

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
- `src/voice/dictation.ts` sends each finished phrase to the sidecar, or to the engine when a
  cloud provider is picked. Phrases are transcribed one at a time through a promise chain so
  they reach the draft in the order they were spoken.
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
- **Voice keys are their own.** A voice provider uses only the key saved in Settings > Voice, never
  an OpenAI sign-in from Models or an environment variable, so audio and text go to a cloud only
  after an explicit choice. One key serves both directions for OpenAI and ElevenLabs.
- **One model per provider.** Each cloud provider uses the model in the table above, with no
  picker; the defaults are the providers' current recommended ones and a picker can come when
  someone needs another.
- **Choices are per device.** Like the rest of dictation's settings, the chosen providers, voices
  and switches are kept in the app's local storage; only keys live in the engine.

## Cloud transcription

Beside the local whisper models, Settings > Voice > Transcription offers five providers. Each one
needs its own key, saved in the row that appears when it is picked.

| Provider | Request | Model | Vocabulary |
| --- | --- | --- | --- |
| OpenAI | `POST /v1/audio/transcriptions`, multipart | `gpt-transcribe` | `keywords[]`; the language goes in `languages[]`, which replaces `language` for this model |
| Groq | `POST /openai/v1/audio/transcriptions`, multipart | `whisper-large-v3-turbo` | a whisper prompt, as local dictation does |
| Deepgram | `POST /v1/listen`, the WAV as the body | `nova-3` with `smart_format` | one `keyterm` per term; no language means `detect_language` |
| ElevenLabs Scribe | `POST /v1/speech-to-text`, multipart | `scribe_v2` (`scribe_v1` is removed on 2026-07-09) | not sent: Scribe's keyterms cost 20% more and refuse brackets |
| AssemblyAI | `POST /v2/upload`, `POST /v2/transcript`, then `GET /v2/transcript/{id}` every 0.5 s | its default (`universal-3-5-pro`, falling back to `universal-2`) | `keyterms_prompt`; no language means `language_detection` |

Shapes were taken from each provider's current documentation on 2026-10-19.

**Why the engine makes the request.** Local whisper stays in the shell because it runs the bundled
sidecar. Cloud requests need a key, and keys already live in the engine's credential store
(`Credentials::secret`, under `voice:<provider>`), as web search's do. Putting the requests in the
engine (`crates/drift-engine/src/voice/`, routes `GET /voice`, `PUT/DELETE /voice/keys`,
`POST /voice/transcribe`, `POST /voice/speak`, `GET /voice/voices`) keeps the key on the engine's side of the API, lets
the phone and the browser use cloud dictation through the engine connection they already have, and
lets the wire shapes be tested against a stand-in server in the engine's own tests. The multipart
bodies are built by hand (`Form` in `voice/transcribe.rs`) rather than turning on reqwest's
`multipart` feature for five small bodies.

**Every provider transcribes per phrase.** The segmenter already cuts speech at pauses, so each
phrase is sent as one request and only a finished phrase reaches the draft, exactly as with
whisper. Streaming (Deepgram's and AssemblyAI's WebSockets, OpenAI's realtime transcription) would
fill the composer while you speak but needs a socket per session and a partial-text model that
can rewrite itself; it is left for the voice mode in `docs/research/voice-mode-proposal.md`.

**A failed phrase is kept.** The error says why (a refused key, a rate limit, the provider's own
message) and the composer offers Retry and Discard. While a phrase is kept, later phrases wait
behind it instead of being sent, so a retry keeps the spoken order and a broken key costs one
request rather than one per sentence. Retry uses the provider chosen at that moment; nothing ever
falls back to another provider or to whisper by itself.

## Read aloud

Turning on Settings > Voice > Read aloud adds a speaker button to every finished reply, beside
copy. A second switch, "Read replies aloud", speaks each reply as it finishes in the open
conversation; a reply that was already finished when it loaded is never read on its own.

Playback stops on a new prompt, on Stop, on switching conversations, and when the button is pressed
again. Starting another reply stops the first. Errors (no key, a refused key, no local voice)
appear as a notice.

**What is spoken.** `speakableText` (`src/voice/speakable.ts`) turns the reply's Markdown into
prose. Fenced code blocks are spoken as "Code block" and tables as "A table". Links are read by
their text, bare URLs by their host, and file references (a path in backticks or plain text, with
or without `:line`) by their file name, so `src/ui/composer.tsx:42` is read as "composer.tsx".
Headings, list markers, quotes and emphasis are dropped, and each block ends with a full stop so
voices pause between them. A long reply is cut by paragraph, then sentence, then word into pieces
of up to 2,500 characters, under OpenAI's 4,096; the next piece is requested while the current one
plays.

**Cloud voices.** OpenAI uses `gpt-4o-mini-tts` with a voice picked from its built-in list
(`marin` by default) and returns MP3. ElevenLabs uses `eleven_flash_v2_5` with
`mp3_44100_128`, defaulting to "George" (`JBFqnCBsd6RMkjVDRZzb`), the premade voice every account
has. The engine checks a voice ID is letters, digits, `-` and `_` before putting it in ElevenLabs'
URL.

**Listing voices.** With an ElevenLabs key saved, the Voice picker lists the account's voices from
`GET /v2/voices` (`crates/drift-engine/src/voice/voices.rs`, up to ten pages of 100), grouped as
"My voices" (cloned, designed, trained), "Saved from the library" (shared voices the account added,
`sharing.status` starting with `copied`), "Workspace" (voices shared to it, not owned) and the
premade defaults, with gender, age and accent from their labels. The public Voice Library itself
is not browsed: save a voice to My Voices on ElevenLabs and it appears. A saved voice the listing no
longer returns stays selectable, and the list is fetched again when the key changes. OpenAI has no
API that lists voices, so its picker is the fixed built-in list; its custom `voice_...` voices are
not supported.

**Why the local voice is the system's own.** The local voice is the Web Speech API's
`speechSynthesis` in the webview, which on Windows speaks with the installed system voices.
It needs no model, no download, no build step and no new dependency; where whisper needs a CMake
build and a model fetch, this needs nothing. Only voices with `localService` set are offered or
used, because WebView2 also lists Microsoft's online "Natural" voices, which send the text to
Microsoft. The alternatives were weighed:

- Piper or Kokoro through ONNX Runtime would need a second native runtime in the build and the
  installer, plus a voice model download of tens to hundreds of megabytes, for a feature that is
  off by default.
- sherpa-onnx bundles the same ONNX Runtime through a C API; the build cost is the same.
- Calling WinRT's `SpeechSynthesizer` from the shell would reach the same voices with more code and
  a round trip for every sentence, and would not work in the browser.

The system voices sound plainer than the cloud ones; picking OpenAI or ElevenLabs is the way to
better speech, and the Settings note says that the reply text then leaves the machine.

## Limits

An Android WebView may offer no `speechSynthesis` or no local voice; there the local voice reports
that it cannot speak, while cloud voices still work, because the engine makes the request and the
phone only plays the MP3. Unverified on a phone so far.
In the browser dev loop local whisper is unavailable (it needs the shell), while cloud dictation and
both kinds of read aloud work.

The very first transcription after installing takes around nine seconds while the graphics
driver compiles and caches the Vulkan pipelines. That cost is paid once per machine, not once
per session, and every run afterwards is about a second.

The model is loaded for each phrase because the sidecar is invoked per utterance. That is
included in the timings above and is comfortably fast enough, but running whisper.cpp in its
server mode would keep the model resident and cut the remaining latency further.

Dictation itself can run as long as you like, because finalized text is committed to the
draft as it arrives rather than held in memory. The finished message is still bounded by the
context window of the model you send it to.
