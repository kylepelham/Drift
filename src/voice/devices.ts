import { createSignal } from "solid-js"
import { dictationInputDeviceId } from "../state/voice"

export type AudioInputDevice = { deviceId: string; label: string }

const [devices, setDevices] = createSignal<AudioInputDevice[]>([])
export const audioInputDevices = devices

export function filterAudioInputDevices(items: Pick<MediaDeviceInfo, "deviceId" | "kind" | "label">[]) {
  const found = new Map<string, AudioInputDevice>()
  for (const item of items) {
    if (item.kind !== "audioinput" || !item.deviceId || item.deviceId === "default" || found.has(item.deviceId)) continue
    found.set(item.deviceId, { deviceId: item.deviceId, label: item.label })
  }
  return [...found.values()]
}

export async function enumerateAudioInputs(media = globalThis.navigator?.mediaDevices) {
  if (!media?.enumerateDevices) return []
  return filterAudioInputDevices(await media.enumerateDevices())
}

export async function refreshAudioInputDevices(media = globalThis.navigator?.mediaDevices) {
  const next = await enumerateAudioInputs(media).catch(() => [])
  setDevices(next)
  return next
}

export function watchAudioInputDevices(
  media: MediaDevices | undefined = globalThis.navigator?.mediaDevices,
  refresh: () => void = () => void refreshAudioInputDevices(media),
) {
  refresh()
  media?.addEventListener?.("devicechange", refresh)
  return () => media?.removeEventListener?.("devicechange", refresh)
}

export function availableCaptureDeviceId(saved: string | null, available: AudioInputDevice[]) {
  return saved && available.some((device) => device.deviceId === saved) ? saved : undefined
}

export function selectedCaptureDeviceId() {
  return availableCaptureDeviceId(dictationInputDeviceId(), devices())
}

export function selectedDeviceMissing() {
  const saved = dictationInputDeviceId()
  return !!saved && !devices().some((device) => device.deviceId === saved)
}
