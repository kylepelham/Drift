import { createSignal } from "solid-js"

export function persisted<T>(key: string, initial: T) {
  const raw = localStorage.getItem(key)
  const [value, setValue] = createSignal<T>(raw ? (JSON.parse(raw) as T) : initial)
  const set = (next: T) => {
    setValue(() => next)
    localStorage.setItem(key, JSON.stringify(next))
  }
  return [value, set] as const
}
