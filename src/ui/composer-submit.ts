import type { ComposerDraft } from "../state/composer"

export type PromptAdmission = { ok: true } | { ok: false; error: string }
export type ComposerSubmitResult = "submitted" | "ignored" | "failed"

type SubmissionLease = {
  hold: (scope: string) => boolean
  release: () => void
}

type ComposerWorkspace = { id: string; name: string; path: string }

export function createComposerSubmissionGuard(onChange: () => void = () => undefined) {
  const held = new Set<string>()

  return {
    has(scope: string) {
      return held.has(scope)
    },
    acquire(scope: string): SubmissionLease | undefined {
      if (held.has(scope)) return
      held.add(scope)
      onChange()
      const scopes = [scope]
      let released = false
      return {
        hold(next: string) {
          if (scopes.includes(next)) return true
          if (held.has(next)) return false
          held.add(next)
          scopes.push(next)
          onChange()
          return true
        },
        release() {
          if (released) return
          released = true
          for (const key of scopes) held.delete(key)
          onChange()
        },
      }
    },
  }
}

type ComposerSubmitEnvironment<Prepared> = {
  scope: () => string
  session: () => string | null
  workspace: () => ComposerWorkspace | null
  online: () => boolean
  draft: (scope: string) => ComposerDraft
  prepare: (sessionId: string | null) => Prepared
  transform: (input: {
    text: string
    sessionId: string | null
    workspace: ComposerWorkspace | null
  }) => Promise<string | null>
  newSession: () => Promise<{ id: string; discard: () => Promise<void> } | undefined>
  sessionScope: (sessionId: string, workspaceId: string) => string
  migrateDraft: (from: string, to: string) => void
  selectSession: (sessionId: string) => void
  sessionCreated: (sessionId: string) => void
  send: (
    sessionId: string,
    text: string,
    snapshot: ComposerDraft,
    workspace: ComposerWorkspace,
    prepared: Prepared,
  ) => Promise<PromptAdmission>
  admitted: (scope: string, snapshot: ComposerDraft, historyDraft: ComposerDraft) => void
  failed?: (error: unknown) => void
}

export function createComposerSubmit<Prepared>(
  environment: ComposerSubmitEnvironment<Prepared>,
  guard = createComposerSubmissionGuard(),
) {
  return async function submit(): Promise<ComposerSubmitResult> {
    const sourceScope = environment.scope()
    const lease = guard.acquire(sourceScope)
    if (!lease) return "ignored"

    let draftScope = sourceScope
    try {
      const snapshot = environment.draft(sourceScope)
      const existing = environment.session()
      const workspace = environment.workspace()
      const prepared = environment.prepare(existing)
      const initial = snapshot.text.trim()
      const text = initial ? await environment.transform({ text: initial, sessionId: existing, workspace }) : ""
      if (
        text === null ||
        (!text && snapshot.staged.length === 0) ||
        !workspace ||
        !environment.online() ||
        environment.scope() !== sourceScope
      )
        return "ignored"

      let sessionId = existing
      if (!sessionId) {
        const session = await environment.newSession()
        if (!session) return "failed"
        if (environment.scope() !== sourceScope || environment.workspace()?.id !== workspace.id) {
          await session.discard()
          return "ignored"
        }
        sessionId = session.id
        draftScope = environment.sessionScope(session.id, workspace.id)
        if (!lease.hold(draftScope)) return "failed"
        environment.migrateDraft(sourceScope, draftScope)
        environment.sessionCreated(session.id)
        environment.selectSession(session.id)
      }

      const admission = await environment.send(sessionId, text, snapshot, workspace, prepared)
      if (!admission.ok) return "failed"
      environment.admitted(draftScope, snapshot, { ...snapshot, text: initial })
      return "submitted"
    } catch (error) {
      environment.failed?.(error)
      return "failed"
    } finally {
      lease.release()
    }
  }
}
