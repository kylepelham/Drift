import { isRemoteRuntime } from "../runtime"
import { shellInvoke } from "../shell"

export async function pickFolder(): Promise<string | null> {
  if (isRemoteRuntime()) return window.prompt("Workspace directory path on the Drift host:")
  const invoke = shellInvoke()
  if (invoke) return (await invoke<string | null>("pick_folder")) ?? null
  return window.prompt("Workspace directory path:")
}
