import type { Engine } from "../engine"
import { nextUserMessage, previousUserMessage, resolveModel } from "../engine/store"
import { emitThreadArchived } from "../plugins"
import { clearComposerDraft, composerScope, draftFromMessage, setComposerDraft } from "../state/composer"
import { prefsFor } from "../state/prefs"
import { selectedSession, selectSession } from "../state/selection"
import { setTheme, theme, themes } from "../state/theme"
import { activeWorkspace, archiveSession } from "../state/workspaces"
import { t } from "../state/i18n"
import { openMcpServers } from "./mcp"

export type SlashItem = { name: string; description: string; needsSession?: boolean; engine?: boolean }

const builtins: SlashItem[] = [
  { name: "new", description: "command.session.new" },
  { name: "fork", description: "command.session.fork.description", needsSession: true },
  { name: "archive", description: "command.session.archive", needsSession: true },
  { name: "undo", description: "command.session.undo.description", needsSession: true },
  { name: "redo", description: "command.session.redo.description", needsSession: true },
  { name: "compact", description: "command.session.compact.description", needsSession: true },
  { name: "share", description: "command.session.share.description", needsSession: true },
  { name: "unshare", description: "command.session.unshare.description", needsSession: true },
  { name: "theme", description: "command.theme.cycle" },
  { name: "mcp", description: "drift.slash.mcp" },
]

export function parseSlash(draft: string) {
  if (!draft.startsWith("/") || draft.startsWith("//")) return null
  const body = draft.slice(1)
  const space = body.search(/\s/)
  if (space < 0) return { query: body, args: "" }
  return { query: body.slice(0, space), args: body.slice(space + 1).trim() }
}

export function slashItems(engine: Engine, query: string): SlashItem[] {
  const q = query.toLowerCase()
  const engineItems: SlashItem[] = engine.state.commands.map((command) => ({
    name: command.name,
    description: command.description ?? t("drift.slash.workspaceCommand"),
    engine: true,
  }))
  return [...builtins.map((item) => ({ ...item, description: t(item.description) })), ...engineItems]
    .filter((item) => !item.needsSession || selectedSession())
    .filter((item) => item.name.toLowerCase().startsWith(q))
    .slice(0, 8)
}

export async function runSlash(engine: Engine, item: SlashItem, args: string) {
  const current = selectedSession()
  if (item.engine) {
    const id = current ?? (await engine.actions.newSession())?.id
    if (!id) return
    selectSession(id)
    return engine.actions.runCommand(id, item.name, args)
  }
  if (item.name === "new") return selectSession(null)
  if (item.name === "fork" && current) {
    const session = await engine.actions.fork(current)
    if (session) selectSession(session.id)
    return
  }
  if (item.name === "archive" && current) {
    const workspace = activeWorkspace()
    if (!workspace) return
    selectSession(null)
    emitThreadArchived(current)
    return archiveSession(current, workspace.id)
  }
  if (item.name === "compact" && current) {
    return engine.actions.summarize(current, resolveModel(engine.state, prefsFor(current).model))
  }
  if (item.name === "share" && current) {
    const url = await engine.actions.share(current)
    if (url) await navigator.clipboard.writeText(url)
    return
  }
  if (item.name === "unshare" && current) return engine.actions.unshare(current)
  if (item.name === "undo" && current) {
    const marker = engine.state.sessions[current]?.revert?.messageID
    const target = previousUserMessage(engine.state.transcripts[current] ?? [], marker)
    if (!target) return
    const restored = draftFromMessage(target)
    if (await engine.actions.revert(current, target.info.id)) setComposerDraft(composerScope(current), restored)
    return
  }
  if (item.name === "redo" && current) {
    const marker = engine.state.sessions[current]?.revert?.messageID
    if (!marker) return
    const next = nextUserMessage(engine.state.transcripts[current] ?? [], marker)
    if (next) {
      const restored = draftFromMessage(next)
      if (await engine.actions.revert(current, next.info.id)) setComposerDraft(composerScope(current), restored)
      return
    }
    if (await engine.actions.unrevert(current)) clearComposerDraft(composerScope(current))
    return
  }
  if (item.name === "theme") setTheme(themes[(themes.indexOf(theme()) + 1) % themes.length])
  if (item.name === "mcp") openMcpServers()
}
