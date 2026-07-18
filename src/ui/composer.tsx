import { createMemo, createSignal, For, Show } from "solid-js"
import { useEngine } from "../engine"
import { resolveModel, sessionBusy } from "../engine/store"
import { agentPref, modelPref, setAgentPref, setModelPref } from "../state/prefs"
import { selectedSession, selectSession } from "../state/selection"
import { activeWorkspace } from "../state/workspaces"
import { Picker, type PickerItem } from "./picker"
import { parseSlash, runSlash, slashItems, type SlashItem } from "./slash"

export function Composer() {
  const engine = useEngine()
  const [draft, setDraft] = createSignal("")
  const [dismissed, setDismissed] = createSignal(false)
  const [cursor, setCursor] = createSignal(0)
  let area!: HTMLTextAreaElement

  const slash = () => (dismissed() ? null : parseSlash(draft()))
  const matches = createMemo<SlashItem[]>(() => {
    const parsed = slash()
    return parsed ? slashItems(engine, parsed.query) : []
  })

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

  const modelItems = createMemo<PickerItem[]>(() =>
    engine.state.providers
      .filter((provider) => engine.state.connected.includes(provider.id) || engine.state.connected.length === 0)
      .flatMap((provider) =>
        Object.values(provider.models)
          .filter((model) => model.capabilities.toolcall)
          .map((model) => ({ id: `${provider.id}/${model.id}`, label: model.name, hint: provider.name })),
      ),
  )

  const agentItems = createMemo<PickerItem[]>(() =>
    engine.state.agents
      .filter((agent) => agent.mode !== "subagent")
      .map((agent) => ({ id: agent.name, label: agent.name, hint: agent.description })),
  )

  const model = () => resolveModel(engine.state, modelPref())
  const modelId = () => {
    const ref = model()
    return ref ? `${ref.providerID}/${ref.modelID}` : undefined
  }

  async function submit() {
    const text = draft().trim()
    if (!text || busy() || !ready()) return
    const id = selectedSession() ?? (await engine.actions.newSession())?.id
    if (!id) return
    selectSession(id)
    setDraft("")
    resize()
    await engine.actions.send(id, text, { model: model(), agent: agentPref() })
  }

  function onKey(event: KeyboardEvent) {
    if (matches().length > 0 && handleSlashKey(event)) return
    if (event.key !== "Enter" || event.shiftKey) return
    event.preventDefault()
    void submit()
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

  return (
    <div class="px-4 pb-4">
      <div class="relative mx-auto max-w-3xl rounded-xl border border-edge bg-surface transition-colors focus-within:border-edge-strong">
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
            resize()
          }}
          onKeyDown={onKey}
        />
        <div class="flex items-center gap-1 px-2.5 pb-2">
          <Picker
            label="agent"
            items={agentItems()}
            selected={agentPref()}
            onPick={(id) => setAgentPref(id)}
          />
          <Picker
            label="model"
            items={modelItems()}
            selected={modelId()}
            onPick={(id) => {
              const [providerID, ...rest] = id.split("/")
              setModelPref({ providerID, modelID: rest.join("/") })
            }}
          />
          <div class="flex-1" />
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
              disabled={!draft().trim() || !ready()}
              onClick={() => void submit()}
            >
              Send
            </button>
          </Show>
        </div>
      </div>
    </div>
  )
}
