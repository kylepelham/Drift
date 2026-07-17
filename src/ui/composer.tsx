import { createMemo, createSignal, Show } from "solid-js"
import { useEngine } from "../engine"
import { resolveModel, sessionBusy } from "../engine/store"
import { agentPref, modelPref, setAgentPref, setModelPref } from "../state/prefs"
import { selectedSession, selectSession } from "../state/selection"
import { Picker, type PickerItem } from "./picker"

export function Composer() {
  const engine = useEngine()
  const [draft, setDraft] = createSignal("")
  let area!: HTMLTextAreaElement

  const busy = () => {
    const id = selectedSession()
    return !!id && sessionBusy(engine.state, id)
  }
  const online = () => engine.state.connection === "online"

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
    if (!text || busy() || !online()) return
    const id = selectedSession() ?? (await engine.actions.newSession())?.id
    if (!id) return
    selectSession(id)
    setDraft("")
    resize()
    await engine.actions.send(id, text, { model: model(), agent: agentPref() })
  }

  function onKey(event: KeyboardEvent) {
    if (event.key !== "Enter" || event.shiftKey) return
    event.preventDefault()
    void submit()
  }

  function resize() {
    area.style.height = "auto"
    area.style.height = `${Math.min(area.scrollHeight, 200)}px`
  }

  return (
    <div class="px-4 pb-4">
      <div class="mx-auto max-w-3xl rounded-xl border border-edge bg-surface transition-colors focus-within:border-edge-strong">
        <textarea
          ref={area}
          rows={1}
          class="max-h-50 w-full resize-none bg-transparent px-4 pt-3 pb-1 text-[0.925rem] outline-none placeholder:text-ink-faint"
          placeholder={online() ? "Message Drift..." : "Connecting to engine..."}
          disabled={!online()}
          value={draft()}
          onInput={(event) => {
            setDraft(event.currentTarget.value)
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
              disabled={!draft().trim() || !online()}
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
