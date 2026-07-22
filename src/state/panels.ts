import { createSignal } from "solid-js"

export const [debugPanelOpen, setDebugPanelOpen] = createSignal(false)

export function toggleDebugPanel() {
  setDebugPanelOpen(!debugPanelOpen())
}
