import { createSignal } from "solid-js"

export const [selectedSession, selectSession] = createSignal<string | null>(null)
