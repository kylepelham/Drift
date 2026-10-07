import { createMemo, createSignal, For, onMount, Show, type JSX } from "solid-js"
import { createStore } from "solid-js/store"
import { useEngine } from "../engine"
import type { components } from "../engine/native/types"
import type { AgentInfo } from "../engine/store"
import { agentModelCapability, agentModelOptions } from "../state/agent-models"
import { t } from "../state/i18n"
import {
  agentBehaviorIssue,
  agentOverrideValue,
  applicableOverride,
  loadPromptSnapshot,
  resetPromptOverride,
  savePromptOverride,
  type PromptOverride,
  type PromptSnapshot,
} from "../state/prompts"
import { Picker } from "./picker"

type BasePrompts = components["schemas"]["BasePrompts"]

/** An agent's editable fields as the form holds them: text, so a half-typed value is never lost. */
export type AgentDraft = { prompt: string; model: string; variant: string; steps: string; advanced: string }

const familyLabels: Record<string, string> = {
  all: "drift.settings.prompts.family.all",
  codex: "drift.settings.prompts.family.codex",
  claude: "drift.settings.prompts.family.claude",
  gemini: "drift.settings.prompts.family.gemini",
  default: "drift.settings.prompts.family.default",
}

const inputClass = "w-full rounded-md border border-edge bg-bg/50 px-2.5 py-1.5 text-xs text-ink outline-none transition-colors placeholder:text-ink-faint focus:border-accent"
const editorClass = "w-full resize-y rounded-lg border border-edge bg-bg/50 p-3 font-mono text-xs leading-relaxed outline-none transition-colors focus:border-accent"

/**
 * Every prompt Drift sends, in one place: the base prompt each model family starts from, and each
 * agent's own prompt and settings. Edits are kept per item until saved, so moving between items
 * never loses or blocks one.
 */
export function PromptsSection() {
  const engine = useEngine()
  const [base, setBase] = createSignal<BasePrompts | null>(null)
  const [snapshot, setSnapshot] = createSignal<PromptSnapshot | null>(null)
  const [selected, setSelected] = createSignal("base:all")
  const [baseDrafts, setBaseDrafts] = createStore<Record<string, string>>({})
  const [agentDrafts, setAgentDrafts] = createStore<Record<string, AgentDraft>>({})
  const [error, setError] = createSignal("")
  const [saved, setSaved] = createSignal(false)
  const [saving, setSaving] = createSignal(false)

  const loadSnapshot = async () => setSnapshot(await loadPromptSnapshot())
  onMount(() => void run(async () => {
    setBase(await engine.actions.basePrompts())
    await loadSnapshot()
  }, false))

  const override = (name: string) => snapshot()?.overrides.find((item) => item.key === `agent:${name}`)
  const agent = (name: string) => engine.state.agents.find((item) => item.name === name)
  const basePrompt = (id: string) => base()?.prompts.find((prompt) => prompt.id === id)
  const baseBaseline = (id: string) => basePrompt(id)?.custom ?? basePrompt(id)?.default ?? ""
  const agentBaseline = (name: string) => draftOf(agentConfig(agent(name), override(name)))
  const baseDirty = (id: string) => baseDrafts[id] !== undefined && baseDrafts[id] !== baseBaseline(id)
  const agentDirty = (name: string) => !!agentDrafts[name] && !sameDraft(agentDrafts[name]!, agentBaseline(name))
  const baseDraft = (id: string) => baseDrafts[id] ?? baseBaseline(id)
  const agentDraft = (name: string) => agentDrafts[name] ?? agentBaseline(name)

  async function run(action: () => Promise<void>, announce = true) {
    setSaving(true)
    setError("")
    setSaved(false)
    try {
      await action()
      setSaved(announce)
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause))
    } finally {
      setSaving(false)
    }
  }

  function select(key: string) {
    setError("")
    setSaved(false)
    setSelected(key)
  }

  function saveBase(id: string) {
    void run(async () => {
      setBase(await engine.actions.saveBasePrompt(id, baseDraft(id)))
      setBaseDrafts(id, undefined!)
    })
  }

  function resetBase(id: string) {
    if (basePrompt(id)?.custom === undefined) return setBaseDrafts(id, undefined!)
    void run(async () => {
      setBase(await engine.actions.resetBasePrompt(id))
      setBaseDrafts(id, undefined!)
    })
  }

  function saveAgent(name: string) {
    const stored = override(name)
    const baseline = agentConfig(agent(name), stored)
    const built = configOf(agentDraft(name), baseline)
    if (typeof built === "string") return setError(built)
    const existing = stored?.value && typeof stored.value === "object" ? applicableOverride(stored.value as Record<string, unknown>) : {}
    const value = agentOverrideValue(built, baseline, existing)
    // A baseline stored before the engine narrowed agent overrides still names retired fields, which the shell now refuses.
    const recorded = stored?.original
    const original = recorded && typeof recorded === "object" ? applicableOverride(recorded as Record<string, unknown>) : agentConfig(agent(name))
    const write = Object.keys(value).length ? () => savePromptOverride(`agent:${name}`, value, original) : () => resetPromptOverride(`agent:${name}`)
    void run(async () => {
      await write()
      await engine.actions.refreshAgents()
      await loadSnapshot()
      setAgentDrafts(name, undefined!)
    })
  }

  function resetAgent(name: string) {
    if (!override(name)) return setAgentDrafts(name, undefined!)
    void run(async () => {
      await resetPromptOverride(`agent:${name}`)
      await engine.actions.refreshAgents()
      await loadSnapshot()
      setAgentDrafts(name, undefined!)
    })
  }

  const groups = createMemo(() => agentGroups(engine.state.agents))
  const selectedBase = () => (selected().startsWith("base:") ? selected().slice(5) : undefined)
  const selectedAgent = () => (selected().startsWith("agent:") ? agent(selected().slice(6)) : undefined)

  return (
    <div class="flex h-full min-h-[26rem] flex-col gap-4 sm:flex-row">
      <nav class="flex max-h-48 shrink-0 flex-col overflow-y-auto sm:max-h-none sm:w-36" aria-label={t("drift.settings.prompts")}>
        <ListGroup title={t("drift.settings.prompts.group.base")} first>
          <For each={base()?.prompts ?? []}>
            {(prompt) => (
              <ListItem
                label={t(familyLabels[prompt.id] ?? prompt.id)}
                active={selected() === `base:${prompt.id}`}
                customized={prompt.custom !== undefined}
                unsaved={baseDirty(prompt.id)}
                onSelect={() => select(`base:${prompt.id}`)}
              />
            )}
          </For>
        </ListGroup>
        <For each={groups()}>
          {(group) => (
            <ListGroup title={t(group.title)}>
              <For each={group.agents}>
                {(item) => (
                  <ListItem
                    label={item.name}
                    active={selected() === `agent:${item.name}`}
                    customized={!!override(item.name)}
                    unsaved={agentDirty(item.name)}
                    problem={!!item.problem}
                    onSelect={() => select(`agent:${item.name}`)}
                  />
                )}
              </For>
            </ListGroup>
          )}
        </For>
      </nav>
      <div class="flex min-h-0 min-w-0 flex-1 flex-col">
        <Show when={selectedBase()}>
          {(id) => (
            <BaseEditor
              id={id()}
              draft={baseDraft(id())}
              customized={basePrompt(id())?.custom !== undefined}
              dirty={baseDirty(id())}
              loaded={!!base()}
              status={<Status error={error()} saved={saved()} />}
              actions={<Actions saving={saving()} dirty={baseDirty(id()) && !!baseDraft(id()).trim()} resettable={basePrompt(id())?.custom !== undefined || baseDirty(id())} onSave={() => saveBase(id())} onReset={() => resetBase(id())} />}
              onInput={(value) => setBaseDrafts(id(), value)}
            />
          )}
        </Show>
        <Show when={selectedAgent()}>
          {(item) => (
            <AgentEditor
              agent={item()}
              draft={agentDraft(item().name)}
              baseline={agentBaseline(item().name)}
              customized={!!override(item().name)}
              status={<Status error={error()} saved={saved()} />}
              actions={<Actions saving={saving()} dirty={agentDirty(item().name)} resettable={!!override(item().name) || agentDirty(item().name)} onSave={() => saveAgent(item().name)} onReset={() => resetAgent(item().name)} />}
              onChange={(field, value) => setAgentDrafts(item().name, { ...agentDraft(item().name), [field]: value })}
            />
          )}
        </Show>
      </div>
    </div>
  )
}

/** A section of the list, ruled off from the one above so each reads as its own. */
function ListGroup(props: { title: string; first?: boolean; children: JSX.Element }) {
  return (
    <div classList={{ "mt-3 border-t border-edge pt-3": !props.first }}>
      <div class="mb-1.5 px-2 text-[0.68rem] font-semibold tracking-wider text-ink-muted uppercase">{props.title}</div>
      <div class="space-y-0.5">{props.children}</div>
    </div>
  )
}

function ListItem(props: { label: string; active: boolean; customized: boolean; unsaved: boolean; problem?: boolean; onSelect: () => void }) {
  return (
    <button
      type="button"
      class="flex w-full items-center gap-2 rounded-md px-2 py-1 text-left text-[0.8rem] outline-none transition-colors focus-visible:bg-raised/60"
      classList={{ "bg-raised text-ink": props.active, "text-ink-muted hover:bg-raised/60 hover:text-ink": !props.active }}
      aria-current={props.active ? "true" : undefined}
      onClick={props.onSelect}
    >
      <span class="min-w-0 flex-1 truncate">{props.label}</span>
      <Show when={props.problem}>
        <span class="size-1.5 shrink-0 rounded-full bg-danger" />
      </Show>
      <Show when={props.unsaved}>
        <span class="shrink-0 text-[0.65rem] text-warn" title={t("drift.settings.prompts.unsaved")}>{t("drift.settings.prompts.unsavedShort")}</span>
      </Show>
      <Show when={props.customized && !props.unsaved}>
        <span class="size-1.5 shrink-0 rounded-full bg-accent" title={t("drift.settings.prompts.customized")} />
      </Show>
    </button>
  )
}

/** The item's name and what it is on the left, Save and Reset on the right, so the editor below gets the height. */
function EditorHeader(props: { title: string; description?: string; customized: boolean; actions: JSX.Element }) {
  return (
    <div class="mb-2.5 flex items-start gap-3">
      <div class="min-w-0 flex-1">
        <div class="flex items-center gap-2">
          <span class="truncate text-sm font-semibold text-ink">{props.title}</span>
          <Show when={props.customized}>
            <span class="shrink-0 rounded bg-accent/15 px-1.5 py-0.5 text-[0.65rem] text-accent">{t("drift.settings.prompts.customized")}</span>
          </Show>
        </div>
        <Show when={props.description}>
          <div class="mt-0.5 line-clamp-2 text-xs leading-relaxed text-ink-faint">{props.description}</div>
        </Show>
      </div>
      {props.actions}
    </div>
  )
}

function Status(props: { error: string; saved: boolean }) {
  return (
    <>
      <Show when={props.error}>
        <div role="alert" class="mt-2 text-xs text-danger">{props.error}</div>
      </Show>
      <Show when={props.saved}>
        <div role="status" class="mt-2 text-xs text-ok">{t("drift.settings.prompts.saved")}</div>
      </Show>
    </>
  )
}

function BaseEditor(props: {
  id: string
  draft: string
  customized: boolean
  dirty: boolean
  loaded: boolean
  status: JSX.Element
  actions: JSX.Element
  onInput: (value: string) => void
}) {
  return (
    <div class="flex min-h-0 flex-1 flex-col">
      <EditorHeader
        title={t(familyLabels[props.id] ?? props.id)}
        description={t(props.id === "all" ? "drift.settings.prompts.allDescription" : "drift.settings.prompts.familyDescription")}
        customized={props.customized}
        actions={props.actions}
      />
      <textarea
        aria-label={t("drift.settings.prompts.systemPrompt")}
        class={`${editorClass} min-h-48 flex-1 resize-none`}
        classList={{ "text-ink": props.customized || props.dirty, "text-ink-faint": !props.customized && !props.dirty }}
        spellcheck={false}
        placeholder={props.id === "all" ? t("drift.settings.prompts.allPlaceholder") : undefined}
        value={props.draft}
        disabled={!props.loaded}
        onInput={(event) => props.onInput(event.currentTarget.value)}
      />
      {props.status}
    </div>
  )
}

function AgentEditor(props: {
  agent: AgentInfo
  draft: AgentDraft
  baseline: AgentDraft
  customized: boolean
  status: JSX.Element
  actions: JSX.Element
  onChange: (field: keyof AgentDraft, value: string) => void
}) {
  const engine = useEngine()
  const capability = () => agentModelCapability(props.agent)
  const inherited = () =>
    props.agent.name === "title"
      ? t("drift.settings.agents.automaticSmallModel")
      : props.agent.name === "compaction"
        ? t("drift.settings.agents.currentSessionModel")
        : t("drift.settings.agents.currentModel")
  const models = createMemo(() => [{ id: "", label: inherited() }, ...agentModelOptions(engine.state, capability() ?? "tools")])
  const changed = (field: keyof AgentDraft) => props.draft[field] !== props.baseline[field]
  // Background jobs only answer in text, so they have no reasoning level, steps, tools or permissions.
  const runsTools = () => capability() !== "text"
  return (
    <div class="flex min-h-0 flex-1 flex-col">
      <EditorHeader title={props.agent.name} description={props.agent.description} customized={props.customized} actions={props.actions} />
      <Show when={props.agent.problem}>
        <div role="alert" class="mb-2.5 rounded-md border border-danger/40 bg-danger/10 px-2.5 py-1.5 text-xs text-danger">{props.agent.problem}</div>
      </Show>
      <div class="mb-2.5 flex flex-wrap gap-3">
        <Show when={capability()}>
          <Field label={t("command.category.model")} class="min-w-48 flex-[2]">
            <Picker
              label={t("command.category.model")}
              items={models()}
              selected={props.draft.model}
              fallbackLabel={props.draft.model || inherited()}
              floating bordered chevronAtEnd placement="below" width="100%"
              onPick={(value) => props.onChange("model", value)}
            />
          </Field>
        </Show>
        <Show when={runsTools()}>
          <Field label={t("drift.settings.prompts.variant")} class="min-w-28 flex-1">
            <input class={inputClass} value={props.draft.variant} placeholder={t("drift.settings.prompts.variantPlaceholder")} onInput={(event) => props.onChange("variant", event.currentTarget.value)} />
          </Field>
          <Field label={t("drift.settings.prompts.steps")} class="min-w-28 flex-1">
            <input class={inputClass} inputMode="numeric" value={props.draft.steps} placeholder={t("drift.settings.prompts.stepsPlaceholder")} onInput={(event) => props.onChange("steps", event.currentTarget.value)} />
          </Field>
        </Show>
      </div>
      <Field label={t("drift.settings.prompts.agentPrompt")} class="flex min-h-0 flex-1 flex-col">
        <textarea
          class={`${editorClass} min-h-40 flex-1 resize-none`}
          classList={{ "text-ink": props.customized || changed("prompt"), "text-ink-faint": !props.customized && !changed("prompt") }}
          spellcheck={false}
          placeholder={t("drift.settings.prompts.inheritsFamily")}
          value={props.draft.prompt}
          onInput={(event) => props.onChange("prompt", event.currentTarget.value)}
        />
      </Field>
      <Show when={runsTools()}>
        <Field label={t("drift.settings.prompts.advanced")} class="mt-2.5" hint={t("drift.settings.prompts.advancedFields")}>
          <textarea
            class={`${editorClass} h-20 resize-y text-ink`}
            spellcheck={false}
            value={props.draft.advanced}
            onInput={(event) => props.onChange("advanced", event.currentTarget.value)}
          />
        </Field>
      </Show>
      {props.status}
    </div>
  )
}

function Field(props: { label: string; hint?: string; class?: string; children: JSX.Element }) {
  return (
    <label class={`block text-xs text-ink-faint ${props.class ?? ""}`}>
      <span class="mb-1 block" title={props.hint}>{props.label}</span>
      {props.children}
    </label>
  )
}

function Actions(props: { saving: boolean; dirty: boolean; resettable: boolean; onSave: () => void; onReset: () => void }) {
  return (
    <div class="flex shrink-0 gap-2">
      <Show when={props.resettable}>
        <button
          class="rounded-md border border-edge px-3 py-1.5 text-xs text-ink-muted transition-colors hover:border-edge-strong hover:text-ink disabled:opacity-40"
          disabled={props.saving}
          onClick={props.onReset}
        >
          {t("common.reset")}
        </button>
      </Show>
      <button
        class="rounded-md bg-accent px-3 py-1.5 text-xs font-medium text-accent-ink disabled:opacity-40"
        disabled={props.saving || !props.dirty}
        onClick={props.onSave}
      >
        {t("common.save")}
      </button>
    </div>
  )
}

/** Agents in the order they are met: picked in the composer, delegated to, then run by Drift itself. */
export function agentGroups(agents: AgentInfo[]) {
  const named = [...agents].sort((a, b) => a.name.localeCompare(b.name))
  return [
    { title: "settings.agents.title", agents: named.filter((agent) => !agent.hidden && agent.mode !== "subagent") },
    { title: "drift.settings.prompts.group.subagents", agents: named.filter((agent) => !agent.hidden && agent.mode === "subagent") },
    { title: "drift.settings.prompts.group.background", agents: named.filter((agent) => agent.hidden) },
  ].filter((group) => group.agents.length)
}

/** The agent as the engine runs it, in the fields Settings can change: nothing shown here goes unapplied. */
export function agentConfig(agent: AgentInfo | undefined, stored?: PromptOverride): Record<string, unknown> {
  const restored = stored?.value && typeof stored.value === "object" ? applicableOverride(stored.value as Record<string, unknown>) : undefined
  if (!agent) return restored ? { ...restored } : {}
  return {
    prompt: agent.prompt,
    model: agent.model ? `${agent.model.providerID}/${agent.model.modelID}` : undefined,
    steps: agent.steps,
    permissions: agent.permissions?.length ? agent.permissions : undefined,
    variant: agent.variant,
    // An empty list is every tool; showing none keeps the editor from offering an override that would mean the same.
    tools: agent.tools.length ? agent.tools : undefined,
    ...restored,
  }
}

/** A config as the form shows it. Tools and permissions, the only structured fields, stay JSON. */
export function draftOf(config: Record<string, unknown>): AgentDraft {
  const text = (value: unknown) => (typeof value === "string" ? value : "")
  const advanced = Object.fromEntries((["tools", "permissions"] as const).filter((key) => config[key] !== undefined).map((key) => [key, config[key]]))
  return {
    prompt: text(config.prompt),
    model: text(config.model),
    variant: text(config.variant),
    steps: typeof config.steps === "number" ? String(config.steps) : "",
    advanced: JSON.stringify(advanced, null, 2),
  }
}

/**
 * The config a draft stands for, or why it cannot be saved. An emptied model or reasoning level that
 * was set is kept as "": for a model that inherits the conversation's, for a level it clears the default.
 */
export function configOf(draft: AgentDraft, baseline: Record<string, unknown>): Record<string, unknown> | string {
  let advanced: unknown
  try {
    advanced = JSON.parse(draft.advanced.trim() || "{}")
  } catch {
    return t("drift.settings.prompts.invalidJson")
  }
  if (!advanced || typeof advanced !== "object" || Array.isArray(advanced)) return t("drift.settings.prompts.invalidJson")
  const config: Record<string, unknown> = { prompt: draft.prompt, ...advanced }
  if (draft.model || baseline.model !== undefined) config.model = draft.model
  if (draft.variant.trim() || baseline.variant !== undefined) config.variant = draft.variant.trim()
  if (draft.steps.trim()) config.steps = Number(draft.steps.trim())
  const { prompt: _prompt, ...behavior } = config
  const issue = agentBehaviorIssue(behavior)
  return issue ? t("drift.settings.prompts.behaviorRefused", { field: issue }) : config
}

function sameDraft(a: AgentDraft, b: AgentDraft) {
  return a.prompt === b.prompt && a.model === b.model && a.variant === b.variant && a.steps === b.steps && a.advanced.trim() === b.advanced.trim()
}
