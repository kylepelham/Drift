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
  clearComposerDraft,
  composerDraft,
  composerScope,
  patchComposerDraft,
  type StagedFile,
} from "../state/composer"
import { selectedSession, selectSession } from "../state/selection"
import { activeWorkspace, selectWorkspace, workspaces } from "../state/workspaces"
import { normalizeDir } from "../engine/store"
import { localAsks, resolveAsk } from "../state/asks"
import { PermissionCard, QuestionCard } from "./attention"
import { IconPaperclip, IconShieldCheck, IconX } from "./icons"
import { openLightbox } from "./lightbox"
import { Picker, type PickerItem } from "./picker"
import { defaultVisibleModelIds, ModelManager } from "./model-manager"
import { ProviderIcon } from "./provider-icon"
import { parseSlash, runSlash, slashItems, type SlashItem } from "./slash"

const maxFileBytes = 10 * 1024 * 1024

function readDataUrl(file: File) {
  return new Promise<string>((resolve, reject) => {
    const reader = new FileReader()
    reader.onload = () => resolve(reader.result as string)
    reader.onerror = () => reject(reader.error)
    reader.readAsDataURL(file)
  })
}

export function firstManualPermission(permissions: Permission[], autoAccepted: (permission: Permission) => boolean) {
  return permissions.find((permission) => !autoAccepted(permission))
}

export function focusedQuestion(questions: QuestionRequest[], requestID?: string) {
  return questions.find((question) => question.id === requestID) ?? questions[0]
}

export function Composer() {
  const engine = useEngine()
  const [dismissed, setDismissed] = createSignal(false)
  const [cursor, setCursor] = createSignal(0)
  const [manageModels, setManageModels] = createSignal(false)
  const [fileError, setFileError] = createSignal("")
  const [focusedQuestionID, setFocusedQuestionID] = createSignal<string>()
  let area!: HTMLTextAreaElement
  let filePicker!: HTMLInputElement

  const scope = () => composerScope(selectedSession(), activeWorkspace()?.id)
  const draft = () => composerDraft(scope()).text
  const staged = () => composerDraft(scope()).staged
  const mentions = () => composerDraft(scope()).mentions
  const setDraft = (text: string) => patchComposerDraft(scope(), { text })
  const setStaged = (value: StagedFile[] | ((current: StagedFile[]) => StagedFile[])) => {
    const key = scope()
    const current = composerDraft(key).staged
    patchComposerDraft(key, { staged: typeof value === "function" ? value(current) : value })
  }
  const setMentions = (mentions: string[]) => patchComposerDraft(scope(), { mentions })

  async function addFiles(files: Iterable<File>) {
    const key = scope()
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

  createEffect(() => {
    scope()
    setDismissed(false)
    setMentionQuery(null)
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

  const slash = () => (dismissed() ? null : parseSlash(draft()))
  const matches = createMemo<SlashItem[]>(() => {
    const parsed = slash()
    return parsed ? slashItems(engine, parsed.query) : []
  })

  const [mentionQuery, setMentionQuery] = createSignal<string | null>(null)
  const [fileHits, setFileHits] = createSignal<string[]>([])
  const [fileCursor, setFileCursor] = createSignal(0)
  let mentionToken = 0

  function updateMention() {
    const caret = area.selectionEnd ?? draft().length
    const match = draft().slice(0, caret).match(/(^|\s)@([\w./\\-]*)$/)
    setMentionQuery(match ? match[2] : null)
  }

  createEffect(() => {
    const query = mentionQuery()
    if (query === null || !ready()) {
      setFileHits([])
      return
    }
    const token = ++mentionToken
    void engine.actions.findFiles(query).then((hits) => {
      if (token !== mentionToken) return
      setFileHits(hits.map((hit) => hit.replaceAll("\\", "/")).slice(0, 8))
      setFileCursor(0)
    })
  })

  function pickMention(path: string) {
    const caret = area.selectionEnd ?? draft().length
    const before = draft().slice(0, caret)
    const match = before.match(/(^|\s)@([\w./\\-]*)$/)
    if (!match) return
    const start = caret - match[2].length - 1
    setDraft(draft().slice(0, start) + "@" + path + " " + draft().slice(caret))
    setMentions([...new Set([...mentions(), path])])
    setMentionQuery(null)
    queueMicrotask(() => {
      resize()
      area.focus()
      const position = start + path.length + 2
      area.setSelectionRange(position, position)
    })
  }

  function handleMentionKey(event: KeyboardEvent) {
    if (event.key === "ArrowDown") setFileCursor(Math.min(fileCursor() + 1, fileHits().length - 1))
    else if (event.key === "ArrowUp") setFileCursor(Math.max(fileCursor() - 1, 0))
    else if (event.key === "Escape") setMentionQuery(null)
    else if (event.key === "Enter" || event.key === "Tab") pickMention(fileHits()[Math.min(fileCursor(), fileHits().length - 1)])
    else return false
    event.preventDefault()
    return true
  }

  function mentionFiles(text: string, paths: string[], root: string) {
    const directory = root.replaceAll("\\", "/").replace(/\/+$/, "")
    return paths.flatMap((path) => {
      const value = "@" + path
      const start = text.indexOf(value)
      if (start < 0 || !directory) return []
      const absolute = `${directory}/${path}`
      return [
        {
          mime: "text/plain",
          filename: path.split("/").pop(),
          url: "file:///" + encodeURI(absolute),
          source: { type: "file" as const, path: absolute, text: { value, start, end: start + value.length } },
        },
      ]
    })
  }

  async function pickSlash(item: SlashItem) {
    const args = slash()?.args ?? ""
    setDraft("")
    resize()
    await runSlash(engine, item, args)
  }

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

  async function submit() {
    const key = scope()
    const snapshot = composerDraft(key)
    const existing = selectedSession()
    const workspace = activeWorkspace()
    const selectedPrefs = prefsFor(existing)
    const selectedModel = resolveModel(engine.state, selectedPrefs.model)
    const selectedVariants = Object.keys(modelInfo(engine.state, selectedModel)?.variants ?? {})
    const selectedVariant =
      selectedPrefs.variant && selectedVariants.includes(selectedPrefs.variant) ? selectedPrefs.variant : undefined
    const initial = snapshot.text.trim()
    const text = initial
      ? await transformComposerSubmit({ text: initial, sessionId: existing, workspace })
      : ""
    if (text === null || (!text && snapshot.staged.length === 0) || !workspace || !online()) return
    const id = existing ?? (await engine.actions.newSession())?.id
    if (!id) return
    if (!existing) {
      seedPrefs(id)
      emitThreadCreated(id)
    }
    selectSession(id)
    const files = [
      ...mentionFiles(text ?? "", snapshot.mentions, workspace.path),
      ...snapshot.staged.map((file) => ({ filename: file.filename, mime: file.mime, url: file.dataUrl })),
    ]
    clearComposerDraft(key)
    setFileError("")
    resize()
    queueMicrotask(() => area.focus())
    await engine.actions.send(id, text ?? "", {
      model: selectedModel,
      agent: selectedPrefs.agent,
      variant: selectedVariant,
      files,
    })
  }

  function onKey(event: KeyboardEvent) {
    if (mentionQuery() !== null && fileHits().length > 0 && handleMentionKey(event)) return
    if (matches().length > 0 && handleSlashKey(event)) return
    if (event.key === "Tab") {
      event.preventDefault()
      cycleAgent(event.shiftKey ? -1 : 1)
      return
    }
    if (event.key !== "Enter" || event.shiftKey) return
    event.preventDefault()
    void submit()
  }

  function cycleAgent(step: number) {
    const items = agentItems()
    if (items.length < 2) return
    const index = items.findIndex((item) => item.id === prefs().agent)
    updatePrefs(selectedSession(), { agent: items[(index + step + items.length) % items.length].id })
  }

  function handleSlashKey(event: KeyboardEvent) {
    if (event.key === "ArrowDown") setCursor(Math.min(cursor() + 1, matches().length - 1))
    else if (event.key === "ArrowUp") setCursor(Math.max(cursor() - 1, 0))
    else if (event.key === "Escape") setDismissed(true)
    else if (event.key === "Enter" && !event.shiftKey) void pickSlash(matches()[Math.min(cursor(), matches().length - 1)])
    else if (event.key === "Tab") void pickSlash(matches()[Math.min(cursor(), matches().length - 1)])
    else return false
    event.preventDefault()
    return true
  }

  function resize() {
    area.style.height = "auto"
    area.style.height = `${Math.min(area.scrollHeight, 200)}px`
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

  function openPermissionSession(permission: Permission) {
    const dir = (permission.metadata?.directory as string | undefined) ?? engine.state.sessions[permission.sessionID]?.directory
    const workspace = dir && workspaces().find((w) => normalizeDir(w.path) === normalizeDir(dir))
    if (workspace && workspace.id !== activeWorkspace()?.id) selectWorkspace(workspace.id)
    selectSession(permission.sessionID)
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
                onClick={() => openPermissionSession(permission())}
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
                      onClick={() => selectSession(request().sessionID)}
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
        <Show when={mentionQuery() !== null && fileHits().length > 0}>
          <div class="pop-in absolute bottom-full left-3 z-20 mb-2 w-96 overflow-hidden rounded-lg border border-edge bg-overlay py-1 shadow-xl shadow-black/30">
            <For each={fileHits()}>
              {(path, index) => (
                <button
                  class="flex w-full items-center px-3 py-1.5 text-left font-mono text-xs transition-colors"
                  classList={{
                    "bg-raised text-ink": index() === Math.min(fileCursor(), fileHits().length - 1),
                    "text-ink-muted": index() !== Math.min(fileCursor(), fileHits().length - 1),
                  }}
                  onMouseEnter={() => setFileCursor(index())}
                  onClick={() => pickMention(path)}
                >
                  <span class="truncate">{path}</span>
                </button>
              )}
            </For>
          </div>
        </Show>
        <Show when={matches().length > 0}>
          <div class="pop-in absolute bottom-full left-3 z-20 mb-2 w-80 overflow-hidden rounded-lg border border-edge bg-overlay py-1 shadow-xl shadow-black/30">
            <For each={matches()}>
              {(item, index) => (
                <button
                  class="flex w-full items-baseline gap-2.5 px-3 py-1.5 text-left text-sm transition-colors"
                  classList={{
                    "bg-raised": index() === Math.min(cursor(), matches().length - 1),
                  }}
                  onMouseEnter={() => setCursor(index())}
                  onClick={() => void pickSlash(item)}
                >
                  <span class="shrink-0 font-mono text-xs text-accent">/{item.name}</span>
                  <span class="min-w-0 truncate text-xs text-ink-faint">{item.description}</span>
                </button>
              )}
            </For>
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
            setDismissed(false)
            setCursor(0)
            updateMention()
            resize()
          }}
          onClick={() => updateMention()}
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
            disabled={(!draft().trim() && staged().length === 0) || !ready()}
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
