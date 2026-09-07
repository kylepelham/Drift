import { createEffect, createMemo, createSignal, For, onCleanup, onMount, Show, untrack } from "solid-js"
import { useEngine } from "../engine"
import { modelInfo, resolveModel, sessionBusy, type QuestionRequest } from "../engine/store"
import { emitThreadCreated, transformComposerSubmit } from "../plugins"
import {
  autoAcceptGlobal,
  autoAcceptSessions,
  modelVisible,
  orderedModelProviderIds,
  prefsFor,
  seedPrefs,
  toggleAutoAccept,
  updatePrefs,
} from "../state/prefs"
import { onKeybind } from "../state/keybinds"
import { agentLabel, reasoningLevelLabel, t } from "../state/i18n"
import type { Permission } from "@opencode-ai/sdk/client"
import {
  canNavigateComposerHistory,
  clearComposerDraft,
  composerDraft,
  composerHistory,
  composerScope,
  migrateComposerDraft,
  navigateComposerHistory,
  patchComposerDraft,
  recordComposerHistory,
  setComposerDraft,
  type ComposerDraft,
  type StagedFile,
} from "../state/composer"
import { selectedSession, selectSession } from "../state/selection"
import { formatModelContext, lmStudioModelReady } from "../state/lm-studio"
import { shellInvoke } from "../shell"
import { activeWorkspace, selectWorkspace, workspaces } from "../state/workspaces"
import { normalizeDir } from "../engine/store"
import { localAsks, resolveAsk } from "../state/asks"
import { permissionRequiresAttention, permissionShouldAutoReply } from "../state/permission-attention"
import { AttentionStrip, PermissionCard, QuestionCard } from "./attention"
import { IconMic, IconPaperclip, IconShieldCheck, IconX } from "./icons"
import { dictationEnabled, dictationModel } from "../state/voice"
import {
  dictationActive,
  dictationElapsed,
  dictationError,
  dictationPending,
  dictationStatus,
  dismissDictationError,
  stopDictation,
  toggleDictation,
} from "../voice/dictation"
import { modelInstalled, refreshVoiceModels } from "../voice/models"
import { appendDictation, formatDictationElapsed } from "../voice/transcript"
import { openSettings } from "./settings"
import { createMentionAutocomplete, mentionFiles } from "./composer-mentions"
import { createSlashMenu } from "./composer-slash"
import { openLightbox } from "./lightbox"
import { Picker, type PickerItem } from "./picker"
import { defaultVisibleModelIds, ModelManager } from "./model-manager"
import { ProviderIcon } from "./provider-icon"
import { createComposerSubmissionGuard, createComposerSubmit } from "./composer-submit"
import {
  formatAttachmentBytes,
  prepareAttachment,
  prepareAttachmentsForSend,
  resolveAttachmentKind,
  unsupportedModelAttachment,
  type AttachmentFailure,
  type AttachmentKind,
} from "../attachments"
import { interruptResponseAnimations } from "./response-animation"
import { dragHasFiles, dropStagesAttachment, dropTargetActive, nextDragDepth, splitDroppedFiles } from "./drag-drop"


// Autosize ceiling for the textarea. Must stay in sync with the `max-h-50` class on the textarea
// (Tailwind spacing 50 = 12.5rem = 200px); otherwise the element and its inline height disagree.
const maxComposerHeightPx = 200
// The OS clipboard is written after the browser finishes its own copy, so ours lands last and wins.
const clipboardRepublishDelayMs = 100

export function firstManualPermission(permissions: Permission[], autoAccepted: (permission: Permission) => boolean) {
  return permissions.find((permission) => !autoAccepted(permission))
}

export function focusedQuestion(questions: QuestionRequest[], requestID?: string) {
  return questions.find((question) => question.id === requestID) ?? questions[0]
}

export function composerSelection(value: string, start: number, end: number) {
  return value.slice(Math.min(start, end), Math.max(start, end))
}

export function selectOwningSession(
  sessionID: string,
  directory: string | undefined,
  availableWorkspaces: { id: string; path: string }[],
  activeWorkspaceID: string | undefined,
  chooseWorkspace: (id: string) => void,
  chooseSession: (id: string) => void,
) {
  const workspace = directory
    ? availableWorkspaces.find((item) => normalizeDir(item.path) === normalizeDir(directory))
    : undefined
  if (workspace && workspace.id !== activeWorkspaceID) chooseWorkspace(workspace.id)
  chooseSession(sessionID)
}

export function Composer() {
  const engine = useEngine()
  const [manageModels, setManageModels] = createSignal(false)
  const [fileError, setFileError] = createSignal("")
  const [dropActive, setDropActive] = createSignal(false)
  const [focusedQuestionID, setFocusedQuestionID] = createSignal<string>()
  const [submissionVersion, setSubmissionVersion] = createSignal(0)
  const [historyNavigation, setHistoryNavigation] = createSignal<{
    scope: string
    index: number
    saved: ComposerDraft | null
    displayed: ComposerDraft
  } | null>(null)
  let area!: HTMLTextAreaElement
  let areaFrame!: HTMLDivElement
  let filePicker!: HTMLInputElement

  const submissionGuard = createComposerSubmissionGuard(() => setSubmissionVersion((value) => value + 1))

  const scope = () => composerScope(selectedSession(), activeWorkspace()?.id)
  const draft = () => composerDraft(scope()).text
  const staged = () => composerDraft(scope()).staged
  const mentions = () => composerDraft(scope()).mentions
  const setDraft = (text: string) => {
    setHistoryNavigation(null)
    patchComposerDraft(scope(), { text })
  }
  const setStaged = (value: StagedFile[] | ((current: StagedFile[]) => StagedFile[])) => {
    setHistoryNavigation(null)
    const key = scope()
    const current = composerDraft(key).staged
    patchComposerDraft(key, { staged: typeof value === "function" ? value(current) : value })
  }
  const setMentions = (mentions: string[]) => patchComposerDraft(scope(), { mentions })

  // Only finalized speech reaches the draft, so live text can never rewrite what was typed.
  function appendVoice(segment: string) {
    const key = scope()
    setHistoryNavigation(null)
    patchComposerDraft(key, { text: appendDictation(composerDraft(key).text, segment) })
  }

  function toggleVoice() {
    if (!modelInstalled(dictationModel())) return openSettings("Voice")
    void toggleDictation(appendVoice)
  }

  const voiceBusy = () => dictationActive() || dictationPending() > 0

  const voiceHint = () => {
    if (dictationStatus() === "starting") return t("drift.voice.starting")
    return dictationPending() > 0 ? t("drift.voice.transcribing") : t("drift.voice.listening")
  }

  async function addFiles(files: Iterable<File>) {
    const key = scope()
    setHistoryNavigation(null)
    setFileError("")
    await Promise.all([...files].map((file) => addFile(file, key)))
  }

  async function addFile(file: File, key: string) {
    const resolved = resolveAttachmentKind({ filename: file.name, mime: file.type })
    const id = crypto.randomUUID()
    patchComposerDraft(key, {
      staged: [
        ...composerDraft(key).staged,
        { id, filename: file.name, mime: resolved.mime, size: file.size, status: "processing", meta: {} },
      ],
    })
    const prepared = await prepareAttachment(file, id)
    if (!prepared.ok) {
      patchComposerDraft(key, { staged: composerDraft(key).staged.filter((item) => item.id !== id) })
      showFileFailure(key, file.name, prepared.reason, prepared.kind, prepared.limit)
      return
    }
    const unsupported = unsupportedModelAttachment(
      [{ filename: prepared.attachment.filename, mime: prepared.attachment.mime }],
      modelInfo(engine.state, resolveModel(engine.state, prefs().model)),
    )
    if (unsupported) {
      patchComposerDraft(key, { staged: composerDraft(key).staged.filter((item) => item.id !== id) })
      const selected = modelInfo(engine.state, resolveModel(engine.state, prefs().model))
      setFileError(
        t("drift.composer.modelUnsupported", {
          filename: file.name,
          kind: t(`drift.attachment.kind.${unsupported.kind}`),
          model: selected?.name ?? t("command.category.model"),
        }),
      )
      return
    }
    patchComposerDraft(key, {
      staged: composerDraft(key).staged.map((item) => (item.id === id ? prepared.attachment : item)),
    })
  }

  function showFileFailure(key: string, filename: string, reason: AttachmentFailure, kind?: AttachmentKind, limit?: number) {
    if (scope() !== key) return
    if (reason === "archive" || reason === "binary")
      return setFileError(t("drift.composer.fileUnsupported", { filename }))
    if (reason === "invalid-utf8") return setFileError(t("drift.composer.fileInvalidUtf8", { filename }))
    if (reason === "too-large")
      return setFileError(
        t("drift.composer.fileTooLarge", {
          filename,
          kind: t(`drift.attachment.kind.${kind}`),
          limit: formatAttachmentBytes(limit ?? 0),
        }),
      )
    setFileError(t("drift.composer.fileReadFailed", { filename }))
  }

  // Window-level so a drop anywhere over the chat/composer area attaches instead of navigating.
  // The desktop shell sets `dragDropEnabled: false` (tauri.conf.json) so WebView2 delivers these
  // HTML5 events with real File objects; the remote-browser runtime gets them natively.
  onMount(() => {
    let depth = 0
    const update = (transition: Parameters<typeof nextDragDepth>[1]) => {
      depth = nextDragDepth(depth, transition)
      setDropActive(dropTargetActive(depth))
    }
    const onDragEnter = (event: DragEvent) => {
      if (!dragHasFiles(event.dataTransfer?.types)) return
      event.preventDefault()
      update("enter")
    }
    const onDragOver = (event: DragEvent) => {
      if (!dragHasFiles(event.dataTransfer?.types)) return
      // preventDefault is required for the drop event to fire at all in WebView2.
      event.preventDefault()
      if (event.dataTransfer) event.dataTransfer.dropEffect = ready() ? "copy" : "none"
    }
    const onDragLeave = (event: DragEvent) => {
      if (!dragHasFiles(event.dataTransfer?.types)) return
      update("leave")
    }
    const onDragEnd = () => update("end")
    const onDrop = (event: DragEvent) => {
      update("drop")
      if (!dragHasFiles(event.dataTransfer?.types)) return
      // A missed drop must never make the browser navigate to the dropped file, wherever it landed.
      event.preventDefault()
      if (!ready() || !event.dataTransfer || !dropStagesAttachment(event.target)) return
      const dropped = splitDroppedFiles(Array.from(event.dataTransfer.items ?? []), Array.from(event.dataTransfer.files ?? []))
      if (dropped.files.length) void addFiles(dropped.files)
      // After addFiles' synchronous error reset, so the notice survives staging kicking off.
      if (dropped.directories) setFileError(t("drift.composer.folderUnsupported"))
    }
    window.addEventListener("dragenter", onDragEnter)
    window.addEventListener("dragover", onDragOver)
    window.addEventListener("dragleave", onDragLeave)
    window.addEventListener("dragend", onDragEnd)
    window.addEventListener("drop", onDrop)
    onCleanup(() => {
      window.removeEventListener("dragenter", onDragEnter)
      window.removeEventListener("dragover", onDragOver)
      window.removeEventListener("dragleave", onDragLeave)
      window.removeEventListener("dragend", onDragEnd)
      window.removeEventListener("drop", onDrop)
    })
  })

  let previousScope = scope()
  createEffect(() => {
    const nextScope = scope()
    if (nextScope !== previousScope) {
      const navigation = untrack(historyNavigation)
      if (navigation?.saved && composerDraft(navigation.scope) === navigation.displayed) {
        setComposerDraft(navigation.scope, navigation.saved)
      }
      setHistoryNavigation(null)
      previousScope = nextScope
    }
    slash.setDismissed(false)
    mention.setQuery(null)
    setFileError("")
  })

  // Tracks draft, not just scope: programmatic restores (revert, /undo) must re-measure.
  createEffect(() => {
    draft()
    queueMicrotask(() => {
      if (!area) return
      resize()
    })
  })

  const slash = createSlashMenu({
    engine,
    area: () => area,
    draft,
    setDraft,
    resize: () => resize(),
  })

  const mention = createMentionAutocomplete({
    area: () => area,
    draft,
    setDraft,
    mentions,
    setMentions,
    ready: () => ready(),
    findFiles: (query) => engine.actions.findFiles(query),
    resize: () => resize(),
  })

  const busy = () => {
    const id = selectedSession()
    return !!id && sessionBusy(engine.state, id)
  }
  const online = () => engine.state.connection === "online"
  const ready = () => online() && !!activeWorkspace()
  const placeholder = () => {
    if (!activeWorkspace()) return t("drift.composer.selectWorkspace")
    if (!online()) return t("drift.composer.connecting")
    return busy() ? `${t("drift.prompt.steer")}...` : t("prompt.placeholder.simple")
  }

  const availableModelItems = createMemo<PickerItem[]>(() => {
    const providers = engine.state.providers.filter((provider) => {
      if (provider.id === "lmstudio") return engine.state.connected.includes(provider.id)
      return (
        engine.state.connected.includes(provider.id) ||
        (engine.state.connection !== "online" && engine.state.connected.length === 0)
      )
    })
    const order = orderedModelProviderIds(providers.map((provider) => provider.id))
    return order.flatMap((providerID) => {
      const provider = providers.find((item) => item.id === providerID)
      if (!provider) return []
      return Object.values(provider.models)
        .filter((model) =>
          provider.id === "lmstudio" ? lmStudioModelReady(model) : model.capabilities.toolcall,
        )
        .sort((a, b) => a.name.localeCompare(b.name))
        .map((model) => ({
          id: `${provider.id}/${model.id}`,
          label: model.name,
          group: provider.name,
          detail:
            provider.id === "lmstudio"
              ? `${model.id} | ${formatModelContext(model.limit.context)} context`
              : undefined,
          providerID: provider.id,
          family: model.family,
          releaseDate: model.release_date,
        }))
    })
  })
  const defaultModelIds = createMemo(() => defaultVisibleModelIds(availableModelItems()))
  const modelItems = createMemo(() =>
    availableModelItems().filter((item) => modelVisible(item.id, defaultModelIds().has(item.id))),
  )

  const agentItems = createMemo<PickerItem[]>(() =>
    engine.state.agents
      .filter((agent) => agent.mode !== "subagent" && !(agent as { hidden?: boolean }).hidden)
      .map((agent) => ({ id: agent.name, label: agentLabel(agent.name), hint: agent.description })),
  )

  const prefs = () => prefsFor(selectedSession())
  const model = () => resolveModel(engine.state, prefs().model)
  const modelId = () => {
    const ref = model()
    return ref ? `${ref.providerID}/${ref.modelID}` : undefined
  }

  const variants = createMemo(() => Object.keys(modelInfo(engine.state, model())?.variants ?? {}))
  const variantItems = createMemo<PickerItem[]>(() => [
    { id: "default", label: t("common.default") },
    ...variants().map((name) => ({ id: name, label: reasoningLevelLabel(name) })),
  ])
  const variant = () => {
    const pref = prefs().variant
    return pref && variants().includes(pref) ? pref : undefined
  }

  const submit = createComposerSubmit(
    {
      scope,
      session: selectedSession,
      workspace: activeWorkspace,
      online,
      draft: composerDraft,
      prepare(existing) {
        const selectedPrefs = prefsFor(existing)
        const selectedModel = resolveModel(engine.state, selectedPrefs.model)
        const selectedVariants = Object.keys(modelInfo(engine.state, selectedModel)?.variants ?? {})
        const selectedVariant =
          selectedPrefs.variant && selectedVariants.includes(selectedPrefs.variant) ? selectedPrefs.variant : undefined
        return { selectedPrefs, selectedModel, selectedVariant }
      },
      transform: transformComposerSubmit,
      newSession: engine.actions.newSession,
      sessionScope: (id) => composerScope(id),
      migrateDraft: migrateComposerDraft,
      selectSession,
      sessionCreated(id) {
        seedPrefs(id)
        emitThreadCreated(id)
      },
      async send(id, text, snapshot, workspace, prepared) {
        const unsupported = unsupportedModelAttachment(snapshot.staged, modelInfo(engine.state, prepared.selectedModel))
        if (unsupported) {
          const message = t("drift.composer.modelUnsupported", {
            filename: unsupported.attachment.filename,
            kind: t(`drift.attachment.kind.${unsupported.kind}`),
            model: modelInfo(engine.state, prepared.selectedModel)?.name ?? t("command.category.model"),
          })
          setFileError(message)
          return { ok: false as const, error: message }
        }
        const attachments = await prepareAttachmentsForSend(snapshot.staged)
        const files = [
          ...mentionFiles(text, snapshot.mentions, workspace.path),
          ...attachments.files,
        ]
        const prompt = [text, attachments.text].filter(Boolean).join("\n\n")
        return engine.actions.send(id, prompt, {
          model: prepared.selectedModel,
          agent: prepared.selectedPrefs.agent,
          variant: prepared.selectedVariant,
          files,
        })
      },
      admitted(key, snapshot, historyDraft) {
        stopDictation()
        recordComposerHistory(historyDraft)
        setHistoryNavigation(null)
        clearComposerDraft(key, snapshot)
        setFileError("")
        resize()
        queueMicrotask(() => area.focus())
      },
      failed(error) {
        engine.actions.notice({ message: error instanceof Error ? error.message : String(error), variant: "error" })
      },
    },
    submissionGuard,
  )

  const submitting = () => {
    submissionVersion()
    return submissionGuard.has(scope())
  }

  function onKey(event: KeyboardEvent) {
    if (event.key === "Enter" && !event.shiftKey && submitting()) {
      event.preventDefault()
      return
    }
    if (mention.open() && mention.handleKey(event)) return
    if (slash.open() && slash.handleKey(event)) return
    if ((event.key === "ArrowUp" || event.key === "ArrowDown") && browseHistory(event)) return
    if (event.key === "Tab") {
      event.preventDefault()
      cycleAgent(event.shiftKey ? -1 : 1)
      return
    }
    if (event.key !== "Enter" || event.shiftKey) return
    event.preventDefault()
    void submit()
  }

  function browseHistory(event: KeyboardEvent) {
    if (event.isComposing || event.altKey || event.ctrlKey || event.metaKey || event.shiftKey) return false
    if (area.selectionStart !== area.selectionEnd) return false
    const direction = event.key === "ArrowUp" ? "up" : "down"
    const active = historyNavigation()
    if (!canNavigateComposerHistory(direction, draft(), area.selectionStart, !!active)) return false
    const result = navigateComposerHistory(
      composerHistory(),
      { index: active?.index ?? -1, saved: active?.saved ?? null },
      composerDraft(scope()),
      direction,
    )
    if (!result) return false
    const key = scope()
    setComposerDraft(key, result.draft)
    setHistoryNavigation(
      result.navigation.index < 0
        ? null
        : { scope: key, index: result.navigation.index, saved: result.navigation.saved, displayed: result.draft },
    )
    slash.setDismissed(true)
    mention.setQuery(null)
    event.preventDefault()
    queueMicrotask(() => {
      resize()
      area.focus()
      const position = result.cursor === "start" ? 0 : result.draft.text.length
      area.setSelectionRange(position, position)
    })
    return true
  }

  function cycleAgent(step: number) {
    const items = agentItems()
    if (items.length < 2) return
    const index = items.findIndex((item) => item.id === prefs().agent)
    updatePrefs(selectedSession(), { agent: items[(index + step + items.length) % items.length].id })
  }

  function resize() {
    // Keep the composer's outer height stable while the live textarea is temporarily `auto` for
    // measurement. Otherwise every key collapses a capped draft to one row, lets the transcript
    // viewport grow and clamp its scroll position, then snaps it back after the height is restored.
    const current = area.offsetHeight
    areaFrame.style.height = `${current}px`
    area.style.height = "auto"
    const next = Math.min(area.scrollHeight, maxComposerHeightPx)
    area.style.height = `${next}px`
    if (next !== current) areaFrame.style.height = `${next}px`
  }

  function republishComposerSelection(event: ClipboardEvent & { currentTarget: HTMLTextAreaElement }) {
    const target = event.currentTarget
    const text = composerSelection(target.value, target.selectionStart, target.selectionEnd)
    const invoke = shellInvoke()
    if (!text || !invoke) return
    setTimeout(() => void invoke("clipboard_write_text", { text }).catch(() => undefined), clipboardRepublishDelayMs)
  }

  const autoAcceptOn = () => {
    const id = selectedSession()
    return autoAcceptGlobal() || (!!id && autoAcceptSessions().includes(id))
  }

  onMount(() => {
    if (dictationEnabled()) void refreshVoiceModels()
    return onKeybind("autoAccept", () => {
      if (autoAcceptGlobal()) return
      const id = selectedSession()
      if (id) toggleAutoAccept(id)
    })
  })

  createEffect(() => {
    for (const permission of Object.values(engine.state.permissions).flat()) {
      if (!permissionShouldAutoReply(permission, engine.state)) continue
      untrack(() => void engine.actions.replyPermission(permission.sessionID, permission.id, "once"))
    }
  })

  const permissions = () => Object.values(engine.state.permissions).flat()
  const questions = () => Object.values(engine.state.questions).flat()
  const pendingPermission = () => firstManualPermission(permissions(), (permission) => !permissionRequiresAttention(permission, engine.state))
  const pendingQuestion = () => focusedQuestion(questions(), focusedQuestionID())
  const pendingAsk = () => localAsks()[0]

  createEffect(() => {
    const next = pendingQuestion()?.id
    if (next !== focusedQuestionID()) setFocusedQuestionID(next)
  })

  function openAttentionSession(sessionID: string, directory?: string) {
    selectOwningSession(
      sessionID,
      directory ?? engine.state.sessions[sessionID]?.directory,
      workspaces(),
      activeWorkspace()?.id,
      selectWorkspace,
      selectSession,
    )
  }

  return (
    <div class="composer-shell relative z-10">
      <div class="composer-attention-stack mx-auto flex w-full max-w-3xl flex-col gap-2">
        <AttentionStrip />
        <Show when={pendingPermission()}>
          {(permission) => (
            <div class="flow-root">
              <PermissionCard
                permission={permission()}
                thread={
                  permission().sessionID !== selectedSession()
                    ? {
                        label: t("drift.composer.pendingInThread", {
                          thread: engine.state.sessions[permission().sessionID]?.title || t("drift.composer.anotherThread"),
                        }),
                        onOpen: () =>
                          openAttentionSession(
                            permission().sessionID,
                            permission().metadata?.directory as string | undefined,
                          ),
                      }
                    : undefined
                }
              />
            </div>
          )}
        </Show>
        <Show when={questions().length > 1}>
          <label class="flex min-w-0 items-center gap-2 px-1 text-xs text-ink-muted">
            <span class="shrink-0">{t("drift.question.pending", { count: questions().length })}</span>
            <select
              class="min-w-0 flex-1 rounded-md border border-edge bg-surface px-2 py-1.5 text-ink"
              value={pendingQuestion()?.id ?? ""}
              onChange={(event) => setFocusedQuestionID(event.currentTarget.value)}
            >
              <For each={questions()}>
                {(request) => (
                  <option value={request.id}>
                    {request.async ? "" : `${t("drift.question.blocking")}: `}
                    {request.questions[0]?.header || t("drift.question.number", { number: 1 })}
                    {" - "}{engine.state.sessions[request.sessionID]?.title || t("drift.composer.anotherThread")}
                  </option>
                )}
              </For>
            </select>
          </label>
        </Show>
        <Show keyed when={pendingQuestion()?.id}>
          {(questionID) => {
            const question = () => questions().find((item) => item.id === questionID)
            return (
              <Show when={question()}>
                {(request) => (
                  <div class="flow-root">
                    <QuestionCard
                      requestID={questionID}
                      async={request().async}
                      questions={[...request().questions]}
                      thread={
                        request().sessionID !== selectedSession()
                          ? {
                              label: t("drift.composer.pendingInThread", {
                                thread: engine.state.sessions[request().sessionID]?.title || t("drift.composer.anotherThread"),
                              }),
                              onOpen: () => openAttentionSession(request().sessionID, request().directory),
                            }
                          : undefined
                      }
                      onAnswer={(answers) => engine.actions.answerQuestion(request().sessionID, questionID, answers)}
                    />
                  </div>
                )}
              </Show>
            )
          }}
        </Show>
        <Show when={pendingAsk()}>
          {(ask) => (
            <div class="flow-root">
              <QuestionCard
                requestID={ask().id}
                questions={ask().questions}
                onAnswer={(answers) => {
                  resolveAsk(ask().id, answers)
                  return true
                }}
              />
            </div>
          )}
        </Show>
      </div>
      <div class="relative mx-auto max-w-3xl rounded-xl border border-edge bg-surface transition-colors focus-within:border-edge-strong">
        <Show when={dropActive() && ready()}>
          <div class="pointer-events-none absolute inset-0 z-30 flex items-center justify-center rounded-xl border-2 border-dashed border-accent bg-surface/85">
            <span class="text-sm font-medium text-accent">{t("drift.composer.dropFiles")}</span>
          </div>
        </Show>
        <Show when={mention.open()}>
          <div class="pop-in absolute bottom-full left-3 z-20 mb-2 w-96 overflow-hidden rounded-lg border border-edge bg-overlay py-1 shadow-xl shadow-black/30">
            <For each={mention.hits()}>
              {(path, index) => (
                <button
                  class="flex w-full items-center px-3 py-1.5 text-left font-mono text-xs transition-colors"
                  classList={{
                    "bg-raised text-ink": index() === mention.activeIndex(),
                    "text-ink-muted": index() !== mention.activeIndex(),
                  }}
                  onMouseEnter={() => mention.setCursor(index())}
                  onClick={() => mention.pick(path)}
                >
                  <span class="truncate">{path}</span>
                </button>
              )}
            </For>
          </div>
        </Show>
        <Show when={slash.open()}>
          <div class="pop-in absolute bottom-full left-3 z-20 mb-2 w-80 overflow-hidden rounded-lg border border-edge bg-overlay py-1 shadow-xl shadow-black/30">
            <Show
              when={slash.argumentItem()}
              fallback={
                <For each={slash.matches()}>
                  {(item, index) => (
                    <button
                      class="flex w-full items-baseline gap-2.5 px-3 py-1.5 text-left text-sm transition-colors"
                      classList={{ "bg-raised": index() === slash.activeMatchIndex() }}
                      onMouseEnter={() => slash.setCursor(index())}
                      onClick={() => void slash.pick(item)}
                    >
                      <span class="shrink-0 font-mono text-xs text-accent">/{item.name}</span>
                      <span class="min-w-0 truncate text-xs text-ink-faint">{item.description}</span>
                      <Show when={item.usage}>
                        <span class="ml-auto shrink-0 font-mono text-[0.65rem] text-ink-faint">{item.usage}</span>
                      </Show>
                    </button>
                  )}
                </For>
              }
            >
              {(item) => (
                <Show
                  when={slash.argumentPresets().length > 0}
                  fallback={<div class="px-3 py-2 text-xs text-ink-faint">{item().usage}</div>}
                >
                  <For each={slash.argumentPresets()}>
                    {(preset, index) => (
                      <button
                        class="flex w-full items-start gap-2.5 px-3 py-1.5 text-left transition-colors"
                        classList={{ "bg-raised": index() === slash.activePresetIndex() }}
                        onMouseEnter={() => slash.setCursor(index())}
                        onClick={() => void slash.pickPreset(item(), preset)}
                      >
                        <span class="shrink-0 font-mono text-xs text-accent">{preset.label}</span>
                        <span class="min-w-0 text-xs text-ink-faint">{preset.description}</span>
                      </button>
                    )}
                  </For>
                </Show>
              )}
            </Show>
          </div>
        </Show>
        <Show when={staged().length > 0 || fileError()}>
          <div class="flex flex-wrap items-center gap-2 px-3 pt-2.5">
            <For each={staged()}>
              {(file) => {
                const remove = () => setStaged(staged().filter((item) => item.id !== file.id))
                return <AttachmentChip file={file} remove={remove} />
              }}
            </For>
            <Show when={fileError()}>
              <span class="text-xs text-danger">{fileError()}</span>
            </Show>
          </div>
        </Show>
        <Show when={voiceBusy() || dictationError()}>
          <div class="flex items-center gap-2 px-4 pt-2.5 text-xs">
            <Show
              when={voiceBusy()}
              fallback={
                <button
                  class="min-w-0 truncate text-left text-danger hover:underline"
                  title={t("common.dismiss")}
                  onClick={dismissDictationError}
                >
                  {dictationError()}
                </button>
              }
            >
              <span class="size-1.5 shrink-0 animate-pulse rounded-full bg-danger" />
              <Show when={dictationActive()}>
                <span class="shrink-0 font-mono text-ink-faint">{formatDictationElapsed(dictationElapsed())}</span>
              </Show>
              <span class="min-w-0 truncate text-ink-faint italic">{voiceHint()}</span>
            </Show>
          </div>
        </Show>
        <div ref={areaFrame} class="w-full">
          <textarea
            ref={area}
            rows={1}
            class="max-h-50 w-full resize-none bg-transparent px-4 pt-3 pb-1 text-[0.925rem] outline-none placeholder:text-ink-faint"
            placeholder={placeholder()}
            disabled={!ready()}
            value={draft()}
            onInput={(event) => {
              setDraft(event.currentTarget.value)
              slash.setDismissed(false)
              slash.setCursor(0)
              mention.refresh()
            }}
            onClick={() => mention.refresh()}
            onCopy={republishComposerSelection}
            onCut={republishComposerSelection}
            onPaste={(event) => {
              if (!event.clipboardData?.files.length) return
              event.preventDefault()
              void addFiles(event.clipboardData.files)
            }}
            onKeyDown={onKey}
          />
        </div>
        <div class="composer-actions flex min-w-0 items-center gap-1 px-2.5 pb-2">
          <div class="composer-options relative flex min-w-0 flex-1 items-center gap-1">
            <Picker
              label={t("command.category.agent")}
              items={agentItems()}
              selected={prefs().agent}
              fallbackLabel={agentLabel(prefs().agent)}
              onPick={(id) => updatePrefs(selectedSession(), { agent: id })}
            />
            <Picker
              label={t("command.category.model")}
              items={modelItems()}
              selected={modelId()}
              icon={<ProviderIcon id={model()?.providerID} class="size-3.5 shrink-0" />}
              fallbackLabel={modelInfo(engine.state, model())?.name}
              onManage={() => setManageModels(true)}
              onPick={(id) => {
                const [providerID, ...rest] = id.split("/")
                updatePrefs(selectedSession(), { model: { providerID, modelID: rest.join("/") } })
              }}
            />
            <Show when={variants().length > 0}>
              <Picker
                label={t("drift.composer.thinkingLevel")}
                items={variantItems()}
                selected={variant() ?? "default"}
                onPick={(id) => updatePrefs(selectedSession(), { variant: id === "default" ? null : id })}
              />
            </Show>
          </div>
          <div class="composer-action-buttons ml-auto flex shrink-0 items-center gap-1">
            <Show when={autoAcceptOn()}>
              <button
                class="flex size-7 items-center justify-center rounded-md text-ink-faint transition-colors hover:bg-raised hover:text-ink disabled:cursor-default disabled:opacity-60"
                title={autoAcceptGlobal() ? t("drift.permissions.autoGlobal") : t("drift.permissions.autoThread")}
                aria-label={t("command.permissions.autoaccept.disable")}
                disabled={autoAcceptGlobal()}
                onClick={() => toggleAutoAccept(selectedSession()!)}
              >
                <IconShieldCheck class="size-3.5" />
              </button>
            </Show>
            <input
              ref={filePicker}
              type="file"
              multiple
              class="hidden"
              onChange={(event) => {
                if (event.currentTarget.files) void addFiles(event.currentTarget.files)
                event.currentTarget.value = ""
              }}
            />
            <Show when={dictationEnabled()}>
              <button
                title={dictationActive() ? t("drift.voice.stop") : t("drift.voice.start")}
                aria-label={dictationActive() ? t("drift.voice.stop") : t("drift.voice.start")}
                aria-pressed={dictationActive()}
                class="flex size-7 items-center justify-center rounded-md transition-colors hover:bg-raised disabled:cursor-default disabled:opacity-60"
                classList={{
                  "text-ink-faint hover:text-ink": !dictationActive(),
                  "text-danger": dictationActive(),
                }}
                disabled={!ready()}
                onClick={toggleVoice}
              >
                <IconMic class="size-4" />
              </button>
            </Show>
            <button
              title={t("prompt.action.attachFile")}
              class="flex size-7 items-center justify-center rounded-md text-ink-faint transition-colors hover:bg-raised hover:text-ink"
              disabled={!ready()}
              onClick={() => filePicker.click()}
            >
              <IconPaperclip class="size-4" />
            </button>
            <Show when={busy()}>
              <button
                class="rounded-md border border-edge px-3 py-1 text-xs text-ink-muted transition-colors hover:border-danger hover:text-danger"
                title={t("prompt.action.stop")}
                onClick={() => {
                  interruptResponseAnimations()
                  void engine.actions.abort(selectedSession()!)
                }}
              >
                {t("prompt.action.stop")}
              </button>
            </Show>
            <button
              class="composer-submit rounded-md bg-accent px-3 py-1 text-xs font-medium text-accent-ink transition-opacity disabled:opacity-40"
              title={busy() ? t("drift.prompt.steer") : t("prompt.action.send")}
              disabled={
                (!draft().trim() && staged().length === 0) ||
                staged().some((file) => file.status === "processing") ||
                !ready() ||
                submitting()
              }
              onClick={() => void submit()}
            >
              {busy() ? t("drift.prompt.steer") : t("prompt.action.send")}
            </button>
          </div>
        </div>
      </div>
      <Show when={manageModels()}>
        <ModelManager items={availableModelItems()} onClose={() => setManageModels(false)} />
      </Show>
    </div>
  )
}

function AttachmentChip(props: { file: StagedFile; remove: () => void }) {
  const kind = () => resolveAttachmentKind(props.file).kind
  const label = () => t(`drift.attachment.kind.${kind()}`)
  const detail = () => {
    if (props.file.status === "processing") return t("drift.attachment.processing")
    if (kind() === "text" && props.file.meta.lines !== undefined)
      return t("drift.attachment.lines", { count: props.file.meta.lines })
    if (kind() === "csv" && props.file.meta.rows !== undefined)
      return t("drift.attachment.table", { rows: props.file.meta.rows, columns: props.file.meta.columns ?? 0 })
    if (kind() === "pdf" && props.file.meta.pages !== undefined)
      return t("drift.attachment.pages", { count: props.file.meta.pages })
    return formatAttachmentBytes(props.file.size)
  }
  const title = () => [props.file.filename, props.file.meta.preview].filter(Boolean).join("\n\n")
  const remove = (
    <button
      title={t("prompt.attachment.remove")}
      class="flex size-4 shrink-0 items-center justify-center rounded text-ink-faint hover:bg-overlay hover:text-ink"
      onClick={props.remove}
    >
      <IconX class="size-3" />
    </button>
  )

  return (
    <Show
      when={kind() === "image" && props.file.dataUrl}
      fallback={
        <span
          class="group/chip flex max-w-64 items-center gap-2 rounded-md border border-edge bg-raised py-1 pr-1 pl-1.5 text-xs text-ink-muted"
          title={title()}
        >
          <Show when={kind() === "pdf" && props.file.meta.thumbnail}>
            {(thumbnail) => <img src={thumbnail()} alt="" class="h-10 w-8 rounded-sm border border-edge object-cover" />}
          </Show>
          <span class="rounded bg-overlay px-1 py-0.5 font-mono text-[0.6rem] font-semibold text-accent uppercase">
            {label()}
          </span>
          <span class="min-w-0">
            <span class="block truncate">{props.file.filename}</span>
            <span class="block truncate text-[0.65rem] text-ink-faint">{detail()}</span>
          </span>
          {remove}
        </span>
      }
    >
      {(url) => (
        <div class="group/chip relative">
          <img
            src={url()}
            alt={props.file.filename}
            title={props.file.filename}
            class="size-16 cursor-pointer rounded-md border border-edge object-cover transition-colors hover:border-edge-strong"
            onClick={() => openLightbox({ url: url(), filename: props.file.filename, mime: props.file.mime })}
          />
          <div class="pointer-events-none absolute right-0 bottom-0 left-0 rounded-b-md bg-black/50 px-1 py-0.5">
            <span class="block truncate text-[0.6rem] text-white">{props.file.filename}</span>
          </div>
          <button
            title={t("prompt.attachment.remove")}
            class="absolute -top-1.5 -right-1.5 flex size-5 items-center justify-center rounded-full border border-edge bg-overlay text-ink-muted opacity-0 transition-opacity group-hover/chip:opacity-100 hover:bg-raised hover:text-ink"
            onClick={props.remove}
          >
            <IconX class="size-3" />
          </button>
        </div>
      )}
    </Show>
  )
}
