import { createSignal } from "solid-js"
import { backendInvoke } from "../backend"
import { shellEvents, type ShellInvoke } from "../shell"
import { t } from "../state/i18n"
import type { DictationModel } from "../state/voice"

export type VoiceModelInfo = { id: DictationModel; bytes: number; installed: boolean }
export type VoiceProgress = { id: string; received: number; total: number }
type ModelTask = "loading" | "downloading" | "removing"

const [models, setModels] = createSignal<VoiceModelInfo[]>([])
const [progress, setProgress] = createSignal<VoiceProgress | null>(null)
const [busy, setBusy] = createSignal<ModelTask | null>(null)
const [error, setError] = createSignal("")
const [supported, setSupported] = createSignal(true)
const [accelerated, setAccelerated] = createSignal(false)

export const voiceModels = models
export const voiceProgress = progress
export const voiceModelBusy = busy
export const voiceModelError = error
export const voiceSupported = supported
export const voiceAccelerated = accelerated

let listening = false

export function modelInstalled(id: DictationModel) {
  return models().some((model) => model.id === id && model.installed)
}

export function modelInfo(id: DictationModel) {
  return models().find((model) => model.id === id)
}

export async function refreshVoiceModels() {
  const invoke = backendInvoke()
  if (!invoke) return setSupported(false)
  watchProgress()
  setBusy("loading")
  try {
    setSupported(await invoke<boolean>("voice_supported"))
    setAccelerated(await invoke<boolean>("voice_acceleration"))
    setModels(await invoke<VoiceModelInfo[]>("voice_models"))
  } catch (cause) {
    setError(reason(cause))
  } finally {
    setBusy(null)
  }
}

export async function downloadVoiceModel(id: DictationModel) {
  const invoke = backendInvoke()
  if (!invoke || busy()) return
  watchProgress()
  setError("")
  setBusy("downloading")
  setProgress({ id, received: 0, total: modelInfo(id)?.bytes ?? 0 })
  try {
    await invoke("voice_model_download", { id })
  } catch (cause) {
    setError(reason(cause))
  } finally {
    setBusy(null)
    setProgress(null)
    await reload(invoke)
  }
}

export async function removeVoiceModel(id: DictationModel) {
  const invoke = backendInvoke()
  if (!invoke || busy()) return
  setError("")
  setBusy("removing")
  try {
    await invoke("voice_model_remove", { id })
  } catch (cause) {
    setError(reason(cause))
  } finally {
    setBusy(null)
    await reload(invoke)
  }
}

export function cancelVoiceModelDownload() {
  void backendInvoke()?.("voice_model_cancel").catch(() => undefined)
}

export function dismissVoiceModelError() {
  setError("")
}

export function downloadPercent(value: VoiceProgress | null) {
  if (!value?.total) return 0
  return Math.min(100, Math.round((value.received / value.total) * 100))
}

export function formatBytes(bytes: number) {
  if (bytes >= 1024 ** 3) return `${(bytes / 1024 ** 3).toFixed(1)} GB`
  if (bytes >= 1024 ** 2) return `${Math.round(bytes / 1024 ** 2)} MB`
  return `${Math.max(1, Math.round(bytes / 1024))} KB`
}

async function reload(invoke: ShellInvoke) {
  await invoke<VoiceModelInfo[]>("voice_models")
    .then(setModels)
    .catch(() => undefined)
}

function watchProgress() {
  const events = shellEvents()
  if (listening || !events) return
  listening = true
  void events
    .listen<VoiceProgress>("voice-model-progress", (event) => setProgress(event.payload))
    .catch(() => {
      listening = false
    })
}

function reason(cause: unknown) {
  const message = cause instanceof Error ? cause.message : String(cause)
  return message === "cancelled" ? "" : message || t("drift.voice.error.download")
}
