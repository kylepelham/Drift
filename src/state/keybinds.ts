import { persisted } from "./persist"

export type KeybindAction = "palette" | "newThread" | "autoAccept" | "zoomIn" | "zoomOut" | "zoomReset"

export const keybindDefs: { action: KeybindAction; combo: string | null }[] = [
  { action: "palette", combo: "ctrl+k" },
  { action: "newThread", combo: "ctrl+n" },
  { action: "autoAccept", combo: "ctrl+shift+a" },
  { action: "zoomIn", combo: "ctrl+=" },
  { action: "zoomOut", combo: "ctrl+-" },
  { action: "zoomReset", combo: "ctrl+0" },
]

const [overrides, setOverrides] = persisted<Partial<Record<KeybindAction, string | null>>>("drift.keybinds", {})

export function comboFor(action: KeybindAction) {
  const current = overrides()
  if (action in current) return current[action] ?? null
  return keybindDefs.find((def) => def.action === action)?.combo ?? null
}

export function setCombo(action: KeybindAction, combo: string | null) {
  setOverrides({ ...overrides(), [action]: combo })
}

export function formatCombo(combo: string | null) {
  if (!combo) return "Unbound"
  return combo
    .split("+")
    .map((part) => (part.length === 1 ? part.toUpperCase() : part.charAt(0).toUpperCase() + part.slice(1)))
    .join("+")
}

export function eventCombo(event: KeyboardEvent): string | null {
  const key = event.key.toLowerCase()
  if (["control", "shift", "alt", "meta"].includes(key)) return null
  const normalized = key === "+" ? "=" : key
  const parts: string[] = []
  if (event.ctrlKey || event.metaKey) parts.push("ctrl")
  if (event.altKey) parts.push("alt")
  if (event.shiftKey && normalized !== "=") parts.push("shift")
  parts.push(normalized)
  return parts.join("+")
}

const handlers = new Map<KeybindAction, () => void>()

export function onKeybind(action: KeybindAction, handler: () => void) {
  handlers.set(action, handler)
}

export function initKeybinds() {
  document.addEventListener("keydown", (event) => {
    const combo = eventCombo(event)
    if (!combo) return
    for (const def of keybindDefs) {
      if (comboFor(def.action) !== combo) continue
      event.preventDefault()
      handlers.get(def.action)?.()
      return
    }
  })
}
