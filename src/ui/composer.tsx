import { createEffect, createMemo, createSignal, For, onMount, Show, untrack } from "solid-js"
import { useEngine } from "../engine"
import { modelInfo, resolveModel, sessionBusy } from "../engine/store"
import { emitThreadCreated, transformComposerSubmit } from "../plugins"
import { autoAcceptSessions, hiddenModelIds, prefsFor, seedPrefs, toggleAutoAccept, updatePrefs } from "../state/prefs"
import { onKeybind } from "../state/keybinds"
import type { Permission } from "@opencode-ai/sdk/client"
import { restoredDraft, setRestoredDraft } from "../state/composer"
import { selectedSession, selectSession } from "../state/selection"
import { activeWorkspace, selectWorkspace, workspaces } from "../state/workspaces"
import { normalizeDir } from "../engine/store"
import { localAsks, resolveAsk } from "../state/asks"
import { PermissionCard, QuestionCard } from "./attention"
import { IconPaperclip, IconX } from "./icons"
import { openLightbox } from "./lightbox"
import { Picker, type PickerItem } from "./picker"
import { ModelManager } from "./model-manager"
import { ProviderIcon } from "./provider-icon"
import { parseSlash, runSlash, slashItems, type SlashItem } from "./slash"

const capitalize = (value: string) => value.charAt(0).toUpperCase() + value.slice(1)

type StagedFile = { id: string; filename: string; mime: string; dataUrl: string; size: number }

const maxFileBytes = 10 * 1024 * 1024

function readDataUrl(file: File) {
  return new Promise<string>((resolve, reject) => {
    const reader = new FileReader()
    reader.onload = () => resolve(reader.result as string)
    reader.onerror = () => reject(reader.error)
    reader.readAsDataURL(file)
  })
}

export function Composer() {
  const engine = useEngine()
  const [draft, setDraft] = createSignal("")
  const [dismissed, setDismissed] = createSignal(false)
  const [cursor, setCursor] = createSignal(0)
  const [manageModels, setManageModels] = createSignal(false)
  const [staged, setStaged] = createSignal<StagedFile[]>([])
  const [fileError, setFileError] = createSignal("")
  let area!: HTMLTextAreaElement
  let filePicker!: HTMLInputElement

  async function addFiles(files: Iterable<File>) {
    setFileError("")
    for (const file of files) {
      if (file.size > maxFileBytes) {
        setFileError(`${file.name} is over 10 MB`)
        continue
      }
      const dataUrl = await readDataUrl(file).catch(() => null)
      if (!dataUrl) {
        setFileError(`Could not read ${file.name}`)
        continue
      }
      setStaged((current) => [
        ...current,
        {
          id: crypto.randomUUID(),
          filename: file.name,
          mime: file.type || "application/octet-stream",
          dataUrl,
          size: file.size,
        },
      ])
    }
  }

  createEffect(() => {
    const restored = restoredDraft()
    if (restored === null) return
    setDraft(restored)
    setRestoredDraft(null)
    queueMicrotask(() => {
      resize()
      area.focus()
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
  const [mentions, setMentions] = createSignal<string[]>([])
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

  function mentionFiles(text: string) {
    const directory = engine.state.directory.replaceAll("\\", "/").replace(/\/+$/, "")
    return mentions().flatMap((path) => {
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
    if (!activeWorkspace()) return "Select a workspace to start"
    return online() ? "Message Drift..." : "Connecting to engine..."
  }

  const availableModelItems = createMemo<PickerItem[]>(() =>
    engine.state.providers
      .filter((provider) => engine.state.connected.includes(provider.id) || engine.state.connected.length === 0)
      .flatMap((provider) =>
        Object.values(provider.models)
          .filter((model) => model.capabilities.toolcall)
          .map((model) => ({ id: `${provider.id}/${model.id}`, label: model.name, group: provider.name })),
      ),
  )
  const modelItems = createMemo(() => availableModelItems().filter((item) => !hiddenModelIds().includes(item.id)))

  const agentItems = createMemo<PickerItem[]>(() =>
    engine.state.agents
      .filter((agent) => agent.mode !== "subagent" && !(agent as { hidden?: boolean }).hidden)
      .map((agent) => ({ id: agent.name, label: capitalize(agent.name), hint: agent.description })),
  )

  const prefs = () => prefsFor(selectedSession())
  const model = () => resolveModel(engine.state, prefs().model)
  const modelId = () => {
    const ref = model()
    return ref ? `${ref.providerID}/${ref.modelID}` : undefined
  }

  const variants = createMemo(() => Object.keys(modelInfo(engine.state, model())?.variants ?? {}))
  const variantItems = createMemo<PickerItem[]>(() => [
    { id: "default", label: "Default" },
    ...variants().map((name) => ({ id: name, label: capitalize(name) })),
  ])
  const variant = () => {
    const pref = prefs().variant
    return pref && variants().includes(pref) ? pref : undefined
  }

  async function submit() {
    const initial = draft().trim()
    const text = initial
      ? await transformComposerSubmit({ text: initial, sessionId: selectedSession(), workspace: activeWorkspace() })
      : ""
    if (text === null || (!text && staged().length === 0) || busy() || !ready()) return
    const existing = selectedSession()
    const id = existing ?? (await engine.actions.newSession())?.id
    if (!id) return
    if (!existing) {
      seedPrefs(id)
      emitThreadCreated(id)
    }
    selectSession(id)
    const files = [
      ...mentionFiles(text ?? ""),
      ...staged().map((file) => ({ filename: file.filename, mime: file.mime, url: file.dataUrl })),
    ]
    setDraft("")
    setStaged([])
    setMentions([])
    setFileError("")
    resize()
    await engine.actions.send(id, text ?? "", { model: model(), agent: prefs().agent, variant: variant(), files })
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
    return !!id && autoAcceptSessions().includes(id)
  }

  function autoAccepted(sessionId: string) {
    const set = autoAcceptSessions()
    if (set.includes(sessionId)) return true
    const parent = engine.state.sessions[sessionId]?.parentID ?? engine.state.links[sessionId]
    return !!parent && set.includes(parent)
  }

  onMount(() =>
    onKeybind("autoAccept", () => {
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

  const pendingPermission = () => Object.values(engine.state.permissions).flat()[0]
  const pendingQuestion = () => Object.values(engine.state.questions).flat()[0]
  const pendingAsk = () => localAsks()[0]
  const takeover = () => !!pendingPermission() || !!pendingQuestion() || !!pendingAsk()

  function openPermissionSession(permission: Permission) {
    const dir = (permission.metadata?.directory as string | undefined) ?? engine.state.sessions[permission.sessionID]?.directory
    const workspace = dir && workspaces().find((w) => normalizeDir(w.path) === normalizeDir(dir))
    if (workspace && workspace.id !== activeWorkspace()?.id) selectWorkspace(workspace.id)
    selectSession(permission.sessionID)
  }

  return (
    <div class="px-4 pb-4">
      <Show when={pendingPermission()}>
        {(permission) => (
          <div class="mx-auto max-w-3xl">
            <Show when={permission().sessionID !== selectedSession()}>
              <button
                class="mb-1 text-xs text-ink-faint transition-colors hover:text-ink"
                title="Open that thread"
                onClick={() => openPermissionSession(permission())}
              >
                in {engine.state.sessions[permission().sessionID]?.title || "another thread"}
              </button>
            </Show>
            <PermissionCard permission={permission()} />
          </div>
        )}
      </Show>
      <Show when={pendingPermission() ? undefined : pendingQuestion()}>
        {(question) => (
          <div class="mx-auto max-w-3xl">
            <Show when={question().sessionID !== selectedSession()}>
              <button
                class="mb-1 text-xs text-ink-faint transition-colors hover:text-ink"
                title="Open that thread"
                onClick={() => selectSession(question().sessionID)}
              >
                in {engine.state.sessions[question().sessionID]?.title || "another thread"}
              </button>
            </Show>
            <QuestionCard
              questions={[...question().questions]}
              onAnswer={(answers) => void engine.actions.answerQuestion(question().sessionID, question().id, answers)}
            />
          </div>
        )}
      </Show>
      <Show when={pendingPermission() || pendingQuestion() ? undefined : pendingAsk()}>
        {(ask) => (
          <div class="mx-auto max-w-3xl">
            <QuestionCard questions={ask().questions} onAnswer={(answers) => resolveAsk(ask().id, answers)} />
          </div>
        )}
      </Show>
      <div
        class="relative mx-auto max-w-3xl rounded-xl border border-edge bg-surface transition-colors focus-within:border-edge-strong"
        classList={{ hidden: takeover() }}
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
          <div class="absolute bottom-full left-3 z-20 mb-2 w-96 overflow-hidden rounded-lg border border-edge bg-overlay py-1 shadow-xl shadow-black/30">
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
          <div class="absolute bottom-full left-3 z-20 mb-2 w-80 overflow-hidden rounded-lg border border-edge bg-overlay py-1 shadow-xl shadow-black/30">
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
                          title="Remove attachment"
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
                        title="Remove attachment"
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
            label="Agent"
            items={agentItems()}
            selected={prefs().agent}
            fallbackLabel={capitalize(prefs().agent)}
            onPick={(id) => updatePrefs(selectedSession(), { agent: id })}
          />
          <Picker
            label="Model"
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
              label="Thinking level"
              items={variantItems()}
              selected={variant() ?? "default"}
              onPick={(id) => updatePrefs(selectedSession(), { variant: id === "default" ? null : id })}
            />
          </Show>
          <div class="flex-1" />
          <Show when={autoAcceptOn()}>
            <button
              class="rounded-full border border-warn/50 bg-warn/10 px-2 py-0.5 text-[0.65rem] text-warn transition-colors select-none hover:bg-warn/20"
              title="Auto-accepting permissions for this thread (Ctrl+Shift+A to toggle)"
              onClick={() => toggleAutoAccept(selectedSession()!)}
            >
              auto-accept
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
            title="Attach files"
            class="flex size-7 items-center justify-center rounded-md text-ink-faint transition-colors hover:bg-raised hover:text-ink"
            disabled={!ready()}
            onClick={() => filePicker.click()}
          >
            <IconPaperclip class="size-4" />
          </button>
          <Show
            when={!busy()}
            fallback={
              <button
                class="rounded-md border border-edge px-3 py-1 text-xs text-ink-muted transition-colors hover:border-danger hover:text-danger"
                onClick={() => void engine.actions.abort(selectedSession()!)}
              >
                Stop
              </button>
            }
          >
            <button
              class="rounded-md bg-accent px-3 py-1 text-xs font-medium text-accent-ink transition-opacity disabled:opacity-40"
              disabled={(!draft().trim() && staged().length === 0) || !ready()}
              onClick={() => void submit()}
            >
              Send
            </button>
          </Show>
        </div>
      </div>
      <Show when={manageModels()}>
        <ModelManager items={availableModelItems()} onClose={() => setManageModels(false)} />
      </Show>
    </div>
  )
}
