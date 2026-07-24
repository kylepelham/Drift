import { createSignal } from "solid-js"

export function persisted<T>(key: string, initial: T, normalize: (value: unknown) => T = (value) => value as T) {
  let stored = initial
  try {
    const raw = localStorage.getItem(key)
    if (raw) stored = normalize(JSON.parse(raw))
  } catch {}
  const [value, setValue] = createSignal<T>(stored)
  const set = (next: T) => {
    setValue(() => next)
    try {
      localStorage.setItem(key, JSON.stringify(next))
    } catch {}
  }
  return [value, set] as const
}
