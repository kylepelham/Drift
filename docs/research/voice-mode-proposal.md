# Voice mode: proposal

Written 2026-10-19 for the "Design first" item in "After the opencode v2 review"
([engine-rewrite.md](../engine-rewrite.md)). No code until Kyle picks.

## The question

A spoken conversation with the agent: Kyle talks, the agent works, the agent answers aloud. Two
shapes:

- **Realtime front.** A speech-to-speech model (OpenAI GPT-Live or Realtime, Gemini Live) holds the
  conversation and hands coding work to a normal Drift turn, then speaks the outcome.
- **Pipeline.** Transcription, a normal Drift turn, read-aloud. The two pieces already planned under
  "Voice and media" (cloud transcription, read aloud) plus a loop that ties them together.

## What exists

- **Drift.** Dictation only (`docs/voice.md`, `src/voice/`). The webview captures 16 kHz audio,
  `audio.ts` cuts phrases on 22 silent 32 ms blocks (about 0.7 s), `voice_transcribe` runs local
  whisper.cpp (about 1 s for a 6 s phrase on Vulkan, 10.7 s on CPU with the large model, 0.87 s with
  `base-q5_1`). Text only grows the draft; silence never sends. Cloud transcription and read aloud
  are planned rows, not built. The Android app records on the phone and lets the PC transcribe
  (`docs/android.md`, phase 4).
- **opencode v2.** `packages/core/src/voice.ts` (88 lines) and `packages/ai/src/experimental/realtime.ts`
  (150 lines) are a broker only: `POST /api/experimental/voice/session` (#55951, 2026-10-16) takes a
  WebRTC offer or asks for a WebSocket and returns the SDP answer or a 60 s client secret, for
  `gpt-live*`, `gpt-realtime*` and `grok-voice*`. It needs an API key: "ChatGPT sign-in can't start
  voice sessions." The voice model runs in the iOS app, which hands work to the server as a plain
  prompt starting `Voice delegation.` with a JSON body (`instruction`, `voice`); the web app only
  parses it to show a Voice message (`app/src/session/timeline/voice-delegation.ts`, #54426). So v2
  is a realtime front whose delegation is an ordinary prompt.
- **GPT-Live** (OpenAI docs, `guides/live-delegation`): full duplex, built to delegate. With client
  delegation the app gets `session.delegation.created` (metadata only, no task text), builds the
  request from the transcript it kept, runs its own backend, and returns results with
  `session.commentary.append` (spoken) or `session.thinking.append` (quiet progress), 500 tokens per
  append. The docs say the application owns permissions and confirmations, and interrupting speech
  leaves backend work running.
- **Realtime API**: sessions last at most 60 minutes; with WebRTC the server truncates unplayed audio
  on barge-in, with WebSocket the client must (`guides/realtime-conversations`). The whole
  conversation is re-read on every response (`guides/voice-latency-cost`).
- **Gemini Live**: audio-only sessions end at 15 minutes without context window compression, a
  connection lasts about 10 minutes, resumption handles last 2 hours (`gemini-api/docs/live-session`).
  Functions can be `NON_BLOCKING` with results scheduled `INTERRUPT`, `WHEN_IDLE` or `SILENT`
  (`live-tools`), which suits a long Drift turn.

## Cost per minute (list prices, 2026-10-19)

| Path | Per minute | Source |
| --- | --- | --- |
| GPT-Live 1 | $0.05 of open session, billed per second, silence included; backend billed separately | OpenAI pricing, `models/gpt-live-1` |
| gpt-realtime-2.1 | $0.019 per minute of Kyle's audio (600 tokens at $32/M), $0.077 per minute of model speech (1,200 tokens at $64/M), plus the growing context re-read each response ($0.40/M cached) | OpenAI pricing; 1 token per 100 ms in, per 50 ms out |
| gpt-realtime-2.1-mini | about a third of the above | OpenAI pricing |
| Gemini 3.8 Live | $0.005 per minute of audio in, $0.018 per minute out | Gemini pricing page |
| Pipeline, local | $0 for transcription (whisper on the PC); speech output from a local voice is $0 | `docs/voice.md` |
| Pipeline, cloud | `gpt-transcribe` $0.0045 per minute of speech, `gpt-live-transcribe` $0.017; speech output `gpt-4o-mini-tts` $12/M audio tokens | OpenAI pricing |

The Drift turn itself costs the same in every shape. For coding, the turn dominates: a voice
minute is cents, a 20-request turn on Opus 5.5 is tens of cents to dollars.

## The points compared

| Point | Realtime front | Pipeline |
| --- | --- | --- |
| Latency to first spoken word | Sub-second for chat (not measured here); for work, the same wait as the Drift turn | Pause to end the utterance (0.7 s today, about 1.2 s needed so a breath does not send), about 1 s whisper on GPU, the turn's first sentence, then TTS start (not measured) |
| Interruption | Native barge-in and truncation (WebRTC) | Drift must build it: keep the mic open with echo cancellation, stop playback when Kyle speaks; the turn keeps running unless he says stop or presses Stop |
| Long tool runs | The voice session sits open; GPT-Live bills $3 an hour of waiting; Realtime caps at 60 min, Gemini at 15 min without compression | Nothing is billed while the turn runs; short spoken notices at milestones (a todo item done, a question, an ask, the end) |
| Two models | The voice model paraphrases Drift's result and may get it wrong or answer from its own knowledge instead of delegating | One model; what is spoken is the reply's prose |
| Accounts | API keys only (OpenAI, xAI, Google); no ChatGPT or Claude sign-in | Works with every provider and sign-in Drift has; transcription and speech can stay local |
| Privacy | All audio leaves the machine | Local by default; cloud only if Kyle picks it |
| Android | The phone's WebView would open WebRTC to OpenAI with a client secret minted by the PC, which breaks the app's rule that the WebView never makes a TLS connection; relaying audio through the PC keeps the rule and adds a hop | Already the planned shape: phone records, PC transcribes; speech comes back as audio through the pinned proxy, or from Android's own TTS added to the existing Kotlin plugin |

**Permission asks by voice, either shape.** Drift's ask card stays the authority; the voice model is
never it. An ask is read aloud ("Drift wants to run bun test. Allow?"). Only a clear "allow" or "deny"
answers it, echoed back before it takes effect; "always allow" and anything outside the workspace
stay tap-only, because a misheard yes cannot be undone. A question the agent asks is answered by
voice as free text, like a typed answer.

## Recommendation

Build the pipeline as voice mode, after cloud transcription and read aloud land, and leave the
realtime front out for now.

The work Kyle hands Drift takes minutes and many tool calls; a realtime model adds a second model,
an API-key-only route, a per-minute meter that runs while Drift works, and a session limit shorter
than many turns, to save about a second at each end. v2 shows the shape such a front would take
(broker plus plain delegation prompt); if the pipeline proves too slow in conversation, GPT-Live
with client delegation is the one to add, because its delegation and quiet-progress events map onto
a Drift turn and its events directly.

## What would be built

1. **Conversation mode** in the composer (`src/voice/`): a switch beside the microphone. Listening
   sends after a configurable pause (default 1.2 s) or the word "send"; "stop" stops the turn.
2. **Barge-in**: the mic stays open while a reply is spoken, with the browser's echo cancellation;
   speech stops playback, never the turn.
3. **Spoken progress**: the engine already publishes todo updates, asks, questions and turn end;
   a small mapper turns them into one-line notices, at most one every 20 s.
4. **Voice asks** as described, with a test that "always allow" cannot be given by voice.
5. **Android**: speech output streamed from the PC, or Android TTS, picked in phone settings.
6. `docs/voice.md` says what leaves the machine in each choice.

Rough size: 700 to 1,000 lines of frontend and shell plus tests, about a week on top of the two
planned rows. A later GPT-Live front: a broker like v2's (about 250 lines), WebRTC in the webview, a
delegation bridge into `POST /sessions/{id}/prompt` and transcript storage, roughly 2,000 lines and
two weeks.
