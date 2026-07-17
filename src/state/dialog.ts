import { shellInvoke } from "./store"

export async function pickFolder(): Promise<string | null> {
  const invoke = shellInvoke()
  if (invoke) return (await invoke<string | null>("pick_folder")) ?? null
  return window.prompt("Workspace directory path:")
}
