import { createSignal } from "solid-js"
import { backendInvoke } from "../backend"
import { shellInvoke } from "../shell"
import { t } from "../state/i18n"
import {
  dictationEnabled,
  dictationKeyterms,
  dictationLanguage,
  dictationModel,
  keytermPrompt,
  persistDictationEnabled,
} from "../state/voice"
import { blockEnergy, createSegmenter, drainSegmenter, encodePcm16, phraseSeconds, pushBlock } from "./audio"
import { startCapture, type Capture } from "./capture"
import { selectedCaptureDeviceId } from "./devices"
import { cleanTranscript } from "./transcript"

export type DictationStatus = "idle" | "starting" | "listening"

/** Shorter than this is a knock or a click rather than a word. */
const minPhraseSeconds = 0.4
const levelEveryBlocks = 4

const [status, setStatus] = createSignal<DictationStatus>("idle")
const [error, setError] = createSignal("")
const [elapsed, setElapsed] = createSignal(0)
const [level, setLevel] = createSignal(0)
const [pending, setPending] = createSignal(0)

export const dictationStatus = status
export const dictationError = error
export const dictationElapsed = elapsed
export const dictationLevel = level
export const dictationPending = pending

let capture: Capture | undefined
let segmenter = createSegmenter()
let ticker: ReturnType<typeof setInterval> | undefined
let queue: Promise<void> = Promise.resolve()
let onSegment: ((text: string) => void) | undefined
let startedAt = 0
let blocks = 0

export function dictationActive() {
  return status() !== "idle"
}

export function dismissDictationError() {
  setError("")
}

export async function toggleDictation(emit: (text: string) => void) {
  if (dictationActive()) return stopDictation()
  await startDictation(emit)
}

export async function startDictation(emit: (text: string) => void) {
  if (dictationActive() || !dictationEnabled()) return
  onSegment = emit
  segmenter = createSegmenter()
  blocks = 0
  setError("")
  setElapsed(0)
  setLevel(0)
  setStatus("starting")
  try {
    capture = await startCapture(handleBlock, selectedCaptureDeviceId())
  } catch (cause) {
    setStatus("idle")
    return setError(captureError(cause))
  }
  // Stopped while the permission prompt was open.
  if (status() === "idle" || !dictationEnabled()) return void capture.stop().catch(() => undefined)
  startedAt = Date.now()
  ticker = setInterval(() => setElapsed(Date.now() - startedAt), 1000)
  setStatus("listening")
}

export function stopDictation() {
  if (!dictationActive()) return
  const active = capture
  capture = undefined
  clearInterval(ticker)
  ticker = undefined
  setStatus("idle")
  setLevel(0)
  setElapsed(0)
  const tail = drainSegmenter(segmenter)
  if (tail) enqueue(tail)
  void active?.stop().catch(() => undefined)
}

export async function setDictationEnabled(enabled: boolean) {
  if (!enabled) {
    persistDictationEnabled(false)
    stopDictation()
  }
  const invoke = shellInvoke()
  try {
    await invoke?.("voice_dictation_set_enabled", { enabled })
    if (enabled) persistDictationEnabled(true)
  } catch (cause) {
    setError(cause instanceof Error ? cause.message : String(cause))
  }
}

export async function syncDictationConsent() {
  await shellInvoke()?.("voice_dictation_set_enabled", { enabled: dictationEnabled() })
}

function handleBlock(block: Float32Array) {
  blocks += 1
  if (blocks % levelEveryBlocks === 0) setLevel(blockEnergy(block))
  const phrase = pushBlock(segmenter, block)
  if (phrase) enqueue(phrase)
}

// Phrases are transcribed one at a time so they reach the draft in the order they were spoken.
function enqueue(phrase: Float32Array) {
  if (phraseSeconds(phrase) < minPhraseSeconds) return
  setPending((count) => count + 1)
  queue = queue.then(() => transcribe(phrase)).finally(() => setPending((count) => Math.max(0, count - 1)))
}

async function transcribe(phrase: Float32Array) {
  const invoke = backendInvoke()
  if (!invoke) return
  try {
    const text = await invoke<string>("voice_transcribe", {
      id: dictationModel(),
      audio: encodePcm16(phrase),
      language: dictationLanguage(),
      prompt: keytermPrompt(dictationKeyterms()),
    })
    const clean = cleanTranscript(text)
    if (clean) onSegment?.(clean)
  } catch (cause) {
    setError(cause instanceof Error ? cause.message : String(cause))
  }
}

export function captureErrorKey(cause: unknown) {
  const name = cause instanceof Error ? cause.name : ""
  if (name === "NotSupportedError") return "drift.voice.error.unsupported"
  if (name === "NotAllowedError" || name === "SecurityError") return "drift.voice.error.permission"
  if (name === "NotFoundError") return "drift.voice.error.noMicrophone"
  return "drift.voice.error.microphone"
}

function captureError(cause: unknown) {
  return t(captureErrorKey(cause))
}
