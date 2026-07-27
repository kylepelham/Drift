import { createEffect, createMemo, createSignal, For, onMount, Show, untrack } from "solid-js"
import { useEngine } from "../engine"
import { modelInfo, resolveModel, sessionBusy, type QuestionRequest } from "../engine/store"
import { emitThreadCreated, transformComposerSubmit } from "../plugins"
import {
  autoAcceptAllowed,
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
import { shellInvoke } from "../shell"
import { activeWorkspace, selectWorkspace, workspaces } from "../state/workspaces"
import { normalizeDir } from "../engine/store"
import { localAsks, resolveAsk } from "../state/asks"
import { PermissionCard, QuestionCard } from "./attention"
import { IconPaperclip, IconShieldCheck, IconX } from "./icons"
import { createMentionAutocomplete, mentionFiles } from "./composer-mentions"
import { createSlashMenu } from "./composer-slash"
import { readDataUrl } from "./files"
import { openLightbox } from "./lightbox"
import { Picker, type PickerItem } from "./picker"
import { defaultVisibleModelIds, ModelManager } from "./model-manager"
import { ProviderIcon } from "./provider-icon"
import { createComposerSubmissionGuard, createComposerSubmit } from "./composer-submit"


const maxFileBytes = 10 * 1024 * 1024
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
  const [focusedQuestionID, setFocusedQuestionID] = createSignal<string>()
  const [submissionVersion, setSubmissionVersion] = createSignal(0)
  const [historyNavigation, setHistoryNavigation] = createSignal<{
    scope: string
    index: number
    saved: ComposerDraft | null
    displayed: ComposerDraft
  } | null>(null)
  let area!: HTMLTextAreaElement
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

  async function addFiles(files: Iterable<File>) {
    const key = scope()
    setHistoryNavigation(null)
    setFileError("")
    for (const file of files) {
      if (file.size > maxFileBytes) {
        setFileError(t("drift.composer.fileTooLarge", { filename: file.name }))
        continue
      }
      const dataUrl = await readDataUrl(file).catch(() => null)
      if (!dataUrl) {
        setFileError(t("drift.composer.fileReadFailed", { filename: file.name }))
        continue
      }
      patchComposerDraft(key, {
        staged: [
          ...composerDraft(key).staged,
          {
            id: crypto.randomUUID(),
            filename: file.name,
            mime: file.type || "application/octet-stream",
            dataUrl,
            size: file.size,
          },
        ],
      })
    }
  }

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
    const providers = engine.state.providers.filter(
      (provider) => engine.state.connected.includes(provider.id) || engine.state.connected.length === 0,
    )
    const order = orderedModelProviderIds(providers.map((provider) => provider.id))
    return order.flatMap((providerID) => {
      const provider = providers.find((item) => item.id === providerID)
      if (!provider) return []
      return Object.values(provider.models)
        .filter((model) => model.capabilities.toolcall)
        .sort((a, b) => a.name.localeCompare(b.name))
        .map((model) => ({
          id: `${provider.id}/${model.id}`,
          label: model.name,
          group: provider.name,
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
      send(id, text, snapshot, workspace, prepared) {
        const files = [
          ...mentionFiles(text, snapshot.mentions, workspace.path),
          ...snapshot.staged.map((file) => ({ filename: file.filename, mime: file.mime, url: file.dataUrl })),
        ]
        return engine.actions.send(id, text, {
          model: prepared.selectedModel,
          agent: prepared.selectedPrefs.agent,
          variant: prepared.selectedVariant,
          files,
        })
      },
      admitted(key, snapshot, historyDraft) {
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
    area.style.height = "auto"
    area.style.height = `${Math.min(area.scrollHeight, maxComposerHeightPx)}px`
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

  function autoAccepted(sessionId: string) {
    return autoAcceptAllowed(
      autoAcceptGlobal(),
      autoAcceptSessions(),
      sessionId,
      engine.state.sessions[sessionId]?.parentID,
      engine.state.links[sessionId],
    )
  }

  onMount(() =>
    onKeybind("autoAccept", () => {
      if (autoAcceptGlobal()) return
      const id = selectedSession()
      if (id) toggleAutoAccept(id)
    }),
  )

  createEffect(() => {
    for (const permission of Object.values(engine.state.permissions).flat()) {
      if (!autoAccepted(permission.sessionID)) continue
      untrack(() => void engine.actions.replyPermission(permission.sessionID, permission.id, "once"))
    }
  })

  const permissions = () => Object.values(engine.state.permissions).flat()
  const questions = () => Object.values(engine.state.questions).flat()
  const pendingPermission = () => firstManualPermission(permissions(), (permission) => autoAccepted(permission.sessionID))
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
      <Show when={pendingPermission()}>
        {(permission) => (
          <div class="mx-auto max-w-3xl">
            <Show when={permission().sessionID !== selectedSession()}>
              <button
                class="mb-1 text-xs text-ink-faint transition-colors hover:text-ink"
                title={t("drift.composer.openThread")}
                onClick={() =>
                  openAttentionSession(
                    permission().sessionID,
                    permission().metadata?.directory as string | undefined,
                  )
                }
              >
                {t("drift.composer.pendingInThread", {
                  thread: engine.state.sessions[permission().sessionID]?.title || t("drift.composer.anotherThread"),
                })}
              </button>
            </Show>
            <PermissionCard permission={permission()} />
          </div>
        )}
      </Show>
      <Show keyed when={pendingPermission() ? undefined : pendingQuestion()?.id}>
        {(questionID) => {
          const question = () => questions().find((item) => item.id === questionID)
          return (
            <Show when={question()}>
              {(request) => (
                <div class="mx-auto max-w-3xl">
                  <Show when={request().sessionID !== selectedSession()}>
                    <button
                      class="mb-1 text-xs text-ink-faint transition-colors hover:text-ink"
                      title={t("drift.composer.openThread")}
                      onClick={() => openAttentionSession(request().sessionID, request().directory)}
                    >
                      {t("drift.composer.pendingInThread", {
                        thread: engine.state.sessions[request().sessionID]?.title || t("drift.composer.anotherThread"),
                      })}
                    </button>
                  </Show>
                  <QuestionCard
                    requestID={questionID}
                    questions={[...request().questions]}
                    onAnswer={(answers) => engine.actions.answerQuestion(request().sessionID, questionID, answers)}
                  />
                </div>
              )}
            </Show>
          )
        }}
      </Show>
      <Show when={pendingPermission() || pendingQuestion() ? undefined : pendingAsk()}>
        {(ask) => (
          <div class="mx-auto max-w-3xl">
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
      <div
        class="relative mx-auto max-w-3xl rounded-xl border border-edge bg-surface transition-colors focus-within:border-edge-strong"
        onDragOver={(event) => {
          if (event.dataTransfer?.types.includes("Files")) event.preventDefault()
        }}
        onDrop={(event) => {
          if (!event.dataTransfer?.files.length) return
          event.preventDefault()
          void addFiles(event.dataTransfer.files)
        }}
      >
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
                return (
                  <Show
                    when={file.mime.startsWith("image/")}
                    fallback={
                      <span class="group/chip flex items-center gap-1.5 rounded-md border border-edge bg-raised py-1 pr-1 pl-1.5 text-xs text-ink-muted">
                        <span class="max-w-40 truncate">{file.filename}</span>
                        <button
                          title={t("prompt.attachment.remove")}
                          class="flex size-4 items-center justify-center rounded text-ink-faint hover:bg-overlay hover:text-ink"
                          onClick={remove}
                        >
                          <IconX class="size-3" />
                        </button>
                      </span>
                    }
                  >
                    <div class="group/chip relative">
                      <img
                        src={file.dataUrl}
                        alt={file.filename}
                        title={file.filename}
                        class="size-16 cursor-pointer rounded-md border border-edge object-cover transition-colors hover:border-edge-strong"
                        onClick={() => openLightbox({ url: file.dataUrl, filename: file.filename, mime: file.mime })}
                      />
                      <div class="pointer-events-none absolute right-0 bottom-0 left-0 rounded-b-md bg-black/50 px-1 py-0.5">
                        <span class="block truncate text-[0.6rem] text-white">{file.filename}</span>
                      </div>
                      <button
                        title={t("prompt.attachment.remove")}
                        class="absolute -top-1.5 -right-1.5 flex size-5 items-center justify-center rounded-full border border-edge bg-overlay text-ink-muted opacity-0 transition-opacity group-hover/chip:opacity-100 hover:bg-raised hover:text-ink"
                        onClick={remove}
                      >
                        <IconX class="size-3" />
                      </button>
                    </div>
                  </Show>
                )
              }}
            </For>
            <Show when={fileError()}>
              <span class="text-xs text-danger">{fileError()}</span>
            </Show>
          </div>
        </Show>
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
            resize()
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
        <div class="flex items-center gap-1 px-2.5 pb-2">
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
          <div class="flex-1" />
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
              onClick={() => void engine.actions.abort(selectedSession()!)}
            >
              {t("prompt.action.stop")}
            </button>
          </Show>
          <button
            class="rounded-md bg-accent px-3 py-1 text-xs font-medium text-accent-ink transition-opacity disabled:opacity-40"
            title={busy() ? t("drift.prompt.steer") : t("prompt.action.send")}
            disabled={(!draft().trim() && staged().length === 0) || !ready() || submitting()}
            onClick={() => void submit()}
          >
            {busy() ? t("drift.prompt.steer") : t("prompt.action.send")}
          </button>
        </div>
      </div>
      <Show when={manageModels()}>
        <ModelManager items={availableModelItems()} onClose={() => setManageModels(false)} />
      </Show>
    </div>
  )
}
