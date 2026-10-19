import { blockEnergy, createSegmenter, drainSegmenter, encodePcm16, phraseSeconds, pushBlock } from "./audio";
import { startCapture, type Capture } from "./capture";
import { selectedCaptureDeviceId } from "./devices";
import { cleanTranscript } from "./transcript";
import { backendInvoke } from "../backend";
import { createSignal } from "solid-js";
import { shellInvoke } from "../shell";
import { t } from "../state/i18n";
import {
    dictationEnabled,
    dictationKeyterms,
    dictationLanguage,
    dictationModel,
    dictationProvider,
    keytermPrompt,
    persistDictationEnabled,
} from "../state/voice";

export type DictationStatus = "idle" | "starting" | "listening";
/** Sends one phrase to a cloud provider through the engine and answers its text. */
export type CloudTranscriber = (body: {
    provider: string;
    audio: string;
    language: string;
    keyterms: string[];
}) => Promise<string>;

/** Shorter than this is a knock or a click rather than a word. */
const minPhraseSeconds = 0.4;
const levelEveryBlocks = 4;

const [status, setStatus] = createSignal<DictationStatus>("idle");
const [error, setError] = createSignal("");
const [elapsed, setElapsed] = createSignal(0);
const [level, setLevel] = createSignal(0);
const [pending, setPending] = createSignal(0);
const [kept, setKept] = createSignal(0);

export const dictationStatus = status;
export const dictationError = error;
export const dictationElapsed = elapsed;
export const dictationLevel = level;
export const dictationPending = pending;
/** Phrases whose transcription failed, kept so they can be sent again. */
export const dictationKept = kept;

let capture: Capture | undefined;
let segmenter = createSegmenter();
let ticker: ReturnType<typeof setInterval> | undefined;
let queue: Promise<void> = Promise.resolve();
let onSegment: ((text: string) => void) | undefined;
let cloud: CloudTranscriber | undefined;
let failed: Float32Array[] = [];
let startedAt = 0;
let blocks = 0;

export function dictationActive() {
    return status() !== "idle";
}

/** Drops the error and any recorded speech kept for a retry. */
export function dismissDictationError() {
    failed = [];
    setKept(0);
    setError("");
}

/** Sends the kept phrases again, in the order they were spoken, with the provider now chosen. */
export function retryDictation() {
    const phrases = failed;
    failed = [];
    setKept(0);
    setError("");
    for (const phrase of phrases) enqueue(phrase);
}

export async function toggleDictation(emit: (text: string) => void, transcriber?: CloudTranscriber) {
    if (dictationActive()) return stopDictation();
    await startDictation(emit, transcriber);
}

export async function startDictation(emit: (text: string) => void, transcriber?: CloudTranscriber) {
    if (dictationActive() || !dictationEnabled()) return;

    onSegment = emit;
    cloud = transcriber;
    segmenter = createSegmenter();
    blocks = 0;
    setError("");
    setElapsed(0);
    setLevel(0);
    setStatus("starting");

    try {
        capture = await startCapture(handleBlock, selectedCaptureDeviceId());
    } catch (cause) {
        setStatus("idle");
        return setError(captureError(cause));
    }
    // Dictation may have stopped while the microphone permission prompt was open.
    if (status() === "idle" || !dictationEnabled()) return void capture.stop().catch(() => undefined);

    startedAt = Date.now();
    ticker = setInterval(() => setElapsed(Date.now() - startedAt), 1000);
    setStatus("listening");
}

export function stopDictation() {
    if (!dictationActive()) return;

    const active = capture;
    capture = undefined;
    clearInterval(ticker);
    ticker = undefined;
    setStatus("idle");
    setLevel(0);
    setElapsed(0);

    const tail = drainSegmenter(segmenter);
    if (tail) enqueue(tail);

    void active?.stop().catch(() => undefined);
}

export async function setDictationEnabled(enabled: boolean) {
    if (!enabled) {
        persistDictationEnabled(false);
        stopDictation();
    }

    const invoke = shellInvoke();
    try {
        await invoke?.("voice_dictation_set_enabled", { enabled });
        if (enabled) persistDictationEnabled(true);
    } catch (cause) {
        setError(cause instanceof Error ? cause.message : String(cause));
    }
}

export async function syncDictationConsent() {
    await shellInvoke()?.("voice_dictation_set_enabled", { enabled: dictationEnabled() });
}

function handleBlock(block: Float32Array) {
    blocks += 1;
    if (blocks % levelEveryBlocks === 0) setLevel(blockEnergy(block));
    const phrase = pushBlock(segmenter, block);
    if (phrase) enqueue(phrase);
}

// Phrases are transcribed one at a time so they reach the draft in the order they were spoken.
function enqueue(phrase: Float32Array) {
    if (phraseSeconds(phrase) < minPhraseSeconds) return;

    setPending((count) => count + 1);
    queue = queue.then(() => transcribe(phrase)).finally(() => setPending((count) => Math.max(0, count - 1)));
}

// After a failure, later phrases wait behind the kept ones so a retry keeps the spoken order.
async function transcribe(phrase: Float32Array) {
    if (failed.length) return keep(phrase);

    try {
        const text = await recognize(phrase);
        const clean = cleanTranscript(text ?? "");
        if (clean) onSegment?.(clean);
    } catch (cause) {
        keep(phrase);
        setError(cause instanceof Error ? cause.message : String(cause));
    }
}

function keep(phrase: Float32Array) {
    failed.push(phrase);
    setKept(failed.length);
}

/** Local whisper through the shell, or the chosen cloud provider through the engine; never one for the other. */
async function recognize(phrase: Float32Array) {
    const provider = dictationProvider();
    const audio = encodePcm16(phrase);
    if (provider !== "local") {
        if (!cloud) throw new Error(t("drift.voice.error.cloudOffline"));
        return cloud({ provider, audio, language: dictationLanguage(), keyterms: dictationKeyterms() });
    }

    return backendInvoke()?.<string>("voice_transcribe", {
        id: dictationModel(),
        audio,
        language: dictationLanguage(),
        prompt: keytermPrompt(dictationKeyterms()),
    });
}

export function captureErrorKey(cause: unknown) {
    const name = cause instanceof Error ? cause.name : "";
    if (name === "NotSupportedError") return "drift.voice.error.unsupported";
    if (name === "NotAllowedError" || name === "SecurityError") return "drift.voice.error.permission";
    if (name === "NotFoundError") return "drift.voice.error.noMicrophone";
    return "drift.voice.error.microphone";
}

function captureError(cause: unknown) {
    return t(captureErrorKey(cause));
}
