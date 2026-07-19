import { createSignal } from "solid-js"

export const [restoredDraft, setRestoredDraft] = createSignal<string | null>(null)
