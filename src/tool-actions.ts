import type { ToolPart } from "@opencode-ai/sdk/client"
import { backendInvoke } from "./backend"
import { activeWorkspace } from "./state/workspaces"

export type FileLocation = { line?: number; column?: number }
export type ToolContextAction = {
  id: string
  label: string
  detail?: string
  disabled?: boolean
  separator?: boolean
  run: () => unknown | Promise<unknown>
}
export type ToolContextActionProvider = (
  part: ToolPart,
) => ToolContextAction | ToolContextAction[] | null | undefined

type FileTarget = { path: string; label: string; line: number }
type OpenFileResult = { positioned: boolean }

const providers = new Map<string, Set<ToolContextActionProvider>>()

export function registerToolContextActions(tool: string, provider: ToolContextActionProvider) {
  const group = providers.get(tool) ?? new Set<ToolContextActionProvider>()
  providers.set(tool, group)
  group.add(provider)
  return () => {
    group.delete(provider)
    if (!group.size) providers.delete(tool)
  }
}

export function toolContextActions(part: ToolPart) {
  const actions: ToolContextAction[] = []
  for (const tool of ["*", part.tool]) {
    for (const provider of providers.get(tool) ?? []) {
      try {
        const result = provider(part)
        for (const action of [result].flat())
          if (action && typeof action.label === "string" && typeof action.run === "function") actions.push(action)
      } catch (error) {
        console.warn(`[Drift] Tool context actions for ${part.tool} failed`, error)
      }
    }
  }
  return actions
}

export function openFile(path: string, location?: FileLocation & { editorOnly?: boolean }) {
  const invoke = backendInvoke()
  if (!invoke) return Promise.reject(new Error("Opening files requires the Drift host backend"))
  return invoke<OpenFileResult>(location?.editorOnly ? "open_file_in_editor" : "open_file", {
    path,
    line: location?.line,
    column: location?.column,
  })
}

export function firstChangedLine(diff: string) {
  let line: number | undefined
  for (const value of diff.split("\n")) {
    const hunk = value.match(/^@@ -\d+(?:,\d+)? \+(\d+)(?:,\d+)? @@/)
    if (hunk) {
      line = Math.max(1, Number(hunk[1]))
      continue
    }
    if (line === undefined) continue
    if (value.startsWith("+") || value.startsWith("-")) return line
    if (value.startsWith(" ")) line++
  }
  return line ?? 1
}

export function builtinFileTargets(part: ToolPart, workspace = activeWorkspace()?.path): FileTarget[] {
  const input = part.state.input as { filePath?: string }
  const metadata = toolMetadata(part)
  if (part.tool === "write") return target(input.filePath, input.filePath, 1, workspace)
  if (part.tool === "edit") {
    const filediff = metadata?.filediff as { file?: string; patch?: string } | undefined
    const path = filediff?.file ?? input.filePath
    const diff = filediff?.patch ?? (metadata?.diff as string | undefined) ?? ""
    return target(path, input.filePath, firstChangedLine(diff), workspace)
  }
  if (part.tool !== "apply_patch" || !Array.isArray(metadata?.files)) return []
  return metadata.files.flatMap((value) => {
    if (!value || typeof value !== "object") return []
    const file = value as {
      filePath?: string
      relativePath?: string
      movePath?: string
      type?: string
      patch?: string
    }
    if (file.type === "delete") return []
    return target(
      file.movePath ?? file.filePath,
      file.relativePath ?? file.movePath ?? file.filePath,
      firstChangedLine(file.patch ?? ""),
      workspace,
    )
  })
}

function target(path: string | undefined, label: string | undefined, line: number, workspace?: string) {
  if (!path) return []
  return [{ path: resolvePath(path, workspace), label: label ?? path, line }]
}

function resolvePath(path: string, workspace?: string) {
  if (!workspace || /^(?:[a-z]:[\\/]|\\\\|\/)/i.test(path)) return path
  const separator = workspace.includes("\\") ? "\\" : "/"
  return `${workspace.replace(/[\\/]$/, "")}${separator}${path.replace(/^[\\/]/, "")}`
}

function toolMetadata(part: ToolPart) {
  const state = part.state
  return (("metadata" in state ? state.metadata : undefined) ?? part.metadata) as Record<string, unknown> | undefined
}

function fileActions(part: ToolPart) {
  const targets = builtinFileTargets(part)
  const multiple = targets.length > 1
  return targets.flatMap<ToolContextAction>((file, index) => [
    {
      id: `open:${file.path}`,
      label: multiple ? `Open ${file.label}` : "Open file",
      separator: index > 0,
      run: () => openFile(file.path),
    },
    {
      id: `open-change:${file.path}:${file.line}`,
      label: multiple ? `Open ${file.label} at change` : "Open at change",
      detail: `Line ${file.line}`,
      run: () => openFile(file.path, { line: file.line, column: 1 }),
    },
  ])
}

registerToolContextActions("edit", fileActions)
registerToolContextActions("write", fileActions)
registerToolContextActions("apply_patch", fileActions)
