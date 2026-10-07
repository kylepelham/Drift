import { createMemo, createSignal, For, onMount, Show, type JSX } from "solid-js"
import { createStore } from "solid-js/store"
import { useEngine } from "../engine"
import type { PermissionRule } from "../engine/native/client"
import type { components } from "../engine/native/types"
import { modelInfo, type AgentInfo, type EngineState } from "../engine/store"
import { agentModelCapability, agentModelOptions } from "../state/agent-models"
import { reasoningLevelLabel, t } from "../state/i18n"
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
import { activeWorkspace } from "../state/workspaces"
import { Picker, type PickerItem } from "./picker"
import { SettingsGroup, SettingsRow } from "./settings-controls"
import { AddRule, newRule, RuleList } from "./settings-permissions"

type BasePrompts = components["schemas"]["BasePrompts"]
type ToolName = components["schemas"]["ToolName"]

export type ToolMode = "all" | "only" | "except"

/** An agent's editable fields as the form holds them, so a half-made choice is never lost. */
export type AgentDraft = {
  prompt: string
  model: string
  variant: string
  steps: string
  toolMode: ToolMode
  tools: string[]
  permissions: PermissionRule[]
}

const familyLabels: Record<string, string> = {
  all: "drift.settings.prompts.family.all",
  codex: "drift.settings.prompts.family.codex",
  claude: "drift.settings.prompts.family.claude",
  gemini: "drift.settings.prompts.family.gemini",
  default: "drift.settings.prompts.family.default",
}

const levelOrder = ["none", "minimal", "low", "medium", "high", "xhigh", "max"]
const stepPresets = ["10", "25", "50", "100", "200", "500"]
const pickerWidth = "13rem"
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
  const [toolNames, setToolNames] = createSignal<ToolName[]>([])
  const [selected, setSelected] = createSignal("base:all")
  const [baseDrafts, setBaseDrafts] = createStore<Record<string, string>>({})
  const [agentDrafts, setAgentDrafts] = createStore<Record<string, AgentDraft>>({})
  const [error, setError] = createSignal("")
  const [saved, setSaved] = createSignal(false)
  const [saving, setSaving] = createSignal(false)

  const loadSnapshot = async () => setSnapshot(await loadPromptSnapshot())
  onMount(() => {
    void run(async () => {
      setBase(await engine.actions.basePrompts())
      await loadSnapshot()
    }, false)
    void engine.actions.toolNames(activeWorkspace()?.path).then(setToolNames, () => undefined)
  })

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
  const status = (unsaved: boolean, customized: boolean, problem?: string) =>
    problem ?? (unsaved ? t("drift.settings.prompts.unsaved") : customized ? t("drift.settings.prompts.customized") : undefined)
  const items = createMemo<PickerItem[]>(() => [
    ...(base()?.prompts ?? []).map((prompt) => ({
      id: `base:${prompt.id}`,
      label: t(familyLabels[prompt.id] ?? prompt.id),
      group: t("drift.settings.prompts.group.base"),
      detail: status(baseDirty(prompt.id), prompt.custom !== undefined),
    })),
    ...groups().flatMap((group) =>
      group.agents.map((item) => ({
        id: `agent:${item.name}`,
        label: item.name,
        group: t(group.title),
        detail: status(agentDirty(item.name), !!override(item.name), item.problem),
      })),
    ),
  ])
  const picker = () => <Picker label={t("drift.settings.prompts")} items={items()} selected={selected()} floating bordered chevronAtEnd placement="below" width={pickerWidth} onPick={select} />

  return (
    <div>
      <Show when={selectedBase()}>
        {(id) => (
          <BaseEditor
            id={id()}
            draft={baseDraft(id())}
            customized={basePrompt(id())?.custom !== undefined}
            dirty={baseDirty(id())}
            loaded={!!base()}
            picker={picker()}
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
            toolNames={toolNames()}
            picker={picker()}
            status={<Status error={error()} saved={saved()} />}
            actions={<Actions saving={saving()} dirty={agentDirty(item().name)} resettable={!!override(item().name) || agentDirty(item().name)} onSave={() => saveAgent(item().name)} onReset={() => resetAgent(item().name)} />}
            onChange={(change) => setAgentDrafts(item().name, { ...agentDraft(item().name), ...change })}
          />
        )}
      </Show>
    </div>
  )
}

/** The picker choosing what is edited and what it is on the left, Save and Reset on the right. */
function EditorHeader(props: { picker: JSX.Element; description?: string; customized: boolean; actions: JSX.Element }) {
  return (
    <div class="mb-6 flex items-start gap-4">
      <div class="min-w-0 flex-1">
        <div class="flex items-center gap-2.5">
          {props.picker}
          <Show when={props.customized}>
            <span class="shrink-0 rounded bg-accent/15 px-1.5 py-0.5 text-[0.65rem] text-accent">{t("drift.settings.prompts.customized")}</span>
          </Show>
        </div>
        <Show when={props.description}>
          <div class="mt-2 text-[0.78rem] leading-relaxed text-ink-faint">{props.description}</div>
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
        <div role="alert" class="mt-4 text-xs text-danger">{props.error}</div>
      </Show>
      <Show when={props.saved}>
        <div role="status" class="mt-4 text-xs text-ok">{t("drift.settings.prompts.saved")}</div>
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
  picker: JSX.Element
  status: JSX.Element
  actions: JSX.Element
  onInput: (value: string) => void
}) {
  return (
    <div>
      <EditorHeader
        picker={props.picker}
        description={t(props.id === "all" ? "drift.settings.prompts.allDescription" : "drift.settings.prompts.familyDescription")}
        customized={props.customized}
        actions={props.actions}
      />
      <SettingsGroup title={t("drift.settings.prompts.systemPrompt")}>
        <div class="py-3">
          <textarea
            aria-label={t("drift.settings.prompts.systemPrompt")}
            class={`${editorClass} h-80`}
            classList={{ "text-ink": props.customized || props.dirty, "text-ink-faint": !props.customized && !props.dirty }}
            spellcheck={false}
            placeholder={props.id === "all" ? t("drift.settings.prompts.allPlaceholder") : undefined}
            value={props.draft}
            disabled={!props.loaded}
            onInput={(event) => props.onInput(event.currentTarget.value)}
          />
        </div>
      </SettingsGroup>
      {props.status}
    </div>
  )
}

function AgentEditor(props: {
  agent: AgentInfo
  draft: AgentDraft
  baseline: AgentDraft
  customized: boolean
  toolNames: ToolName[]
  picker: JSX.Element
  status: JSX.Element
  actions: JSX.Element
  onChange: (change: Partial<AgentDraft>) => void
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
  const levels = createMemo(() => [
    { id: "", label: t("drift.settings.prompts.variantPlaceholder") },
    ...reasoningLevels(engine.state, props.draft.model, props.draft.variant).map((level) => ({ id: level, label: reasoningLevelLabel(level) })),
  ])
  const steps = createMemo(() => [
    { id: "", label: t("drift.settings.prompts.stepsPlaceholder") },
    ...[...new Set([...stepPresets, props.draft.steps].filter(Boolean))].sort((a, b) => Number(a) - Number(b)).map((step) => ({ id: step, label: step })),
  ])
  const changed = () => props.draft.prompt !== props.baseline.prompt
  // Background jobs only answer in text, so they have no reasoning level, steps, tools or permissions.
  const runsTools = () => capability() !== "text"
  return (
    <div class="space-y-6">
      <div>
        <EditorHeader picker={props.picker} description={props.agent.description} customized={props.customized} actions={props.actions} />
        <Show when={props.agent.problem}>
          <div role="alert" class="-mt-3 rounded-md border border-danger/40 bg-danger/10 px-3 py-2 text-xs text-danger">{props.agent.problem}</div>
        </Show>
      </div>
      <SettingsGroup title={t("drift.settings.prompts.behaviorGroup")}>
        <Show when={capability()}>
          <SettingsRow title={t("command.category.model")} description={t("drift.settings.prompts.modelDescription")}>
            <Picker
              label={t("command.category.model")}
              items={models()}
              selected={props.draft.model}
              fallbackLabel={props.draft.model || inherited()}
              floating bordered chevronAtEnd placement="below" width={pickerWidth}
              onPick={(model) => props.onChange({ model })}
            />
          </SettingsRow>
        </Show>
        <Show when={runsTools()}>
          <SettingsRow title={t("drift.settings.prompts.variant")} description={t("drift.settings.prompts.variantDescription")}>
            <Picker
              label={t("drift.settings.prompts.variant")}
              items={levels()}
              selected={props.draft.variant}
              floating bordered chevronAtEnd placement="below" width={pickerWidth}
              onPick={(variant) => props.onChange({ variant })}
            />
          </SettingsRow>
          <SettingsRow title={t("drift.settings.prompts.steps")} description={t("drift.settings.prompts.stepsDescription")}>
            <Picker
              label={t("drift.settings.prompts.steps")}
              items={steps()}
              selected={props.draft.steps}
              floating bordered chevronAtEnd placement="below" width={pickerWidth}
              onPick={(steps) => props.onChange({ steps })}
            />
          </SettingsRow>
        </Show>
      </SettingsGroup>
      <SettingsGroup title={t("drift.settings.prompts.agentPrompt")}>
        <div class="py-3">
          <textarea
            aria-label={t("drift.settings.prompts.agentPrompt")}
            class={`${editorClass} h-80`}
            classList={{ "text-ink": props.customized || changed(), "text-ink-faint": !props.customized && !changed() }}
            spellcheck={false}
            placeholder={t("drift.settings.prompts.inheritsFamily")}
            value={props.draft.prompt}
            onInput={(event) => props.onChange({ prompt: event.currentTarget.value })}
          />
        </div>
      </SettingsGroup>
      <Show when={runsTools()}>
        <SettingsGroup title={t("drift.settings.prompts.tools")}>
          <SettingsRow title={t("drift.settings.prompts.toolsOffered")} description={t("drift.settings.prompts.toolsDescription")}>
            <Picker
              label={t("drift.settings.prompts.toolsOffered")}
              items={(["all", "only", "except"] as const).map((mode) => ({ id: mode, label: t(`drift.settings.prompts.tools.${mode}`) }))}
              selected={props.draft.toolMode}
              floating bordered chevronAtEnd placement="below" width={pickerWidth}
              onPick={(mode) => props.onChange({ toolMode: mode as ToolMode, tools: mode === "all" ? [] : props.draft.tools })}
            />
          </SettingsRow>
          <Show when={props.draft.toolMode !== "all"}>
            <ToolChips
              names={props.toolNames}
              chosen={props.draft.tools}
              excluding={props.draft.toolMode === "except"}
              onChange={(tools) => props.onChange({ tools })}
            />
          </Show>
        </SettingsGroup>
        <SettingsGroup title={t("drift.settings.permissions")}>
          <div class="space-y-3 py-3">
            <div class="text-[0.72rem] leading-relaxed text-ink-faint">{t("drift.settings.prompts.permissionsDescription")}</div>
            <RuleList rules={props.draft.permissions} onChange={(permissions) => props.onChange({ permissions })} />
            <AddRule onAdd={() => props.onChange({ permissions: [...props.draft.permissions, newRule()] })} />
          </div>
        </SettingsGroup>
      </Show>
      {props.status}
    </div>
  )
}

/**
 * Built-in tools one chip each; an MCP server one chip for all its tools, since a server can bring
 * dozens (a single MCP tool is narrowed with a permission rule instead). Names the list holds but
 * the engine does not offer here stay as chips of their own.
 */
function ToolChips(props: { names: ToolName[]; chosen: string[]; excluding: boolean; onChange: (tools: string[]) => void }) {
  const builtIn = createMemo(() => {
    const known = new Set(props.names.map((tool) => tool.name))
    return [...props.names.filter((tool) => !tool.server).map((tool) => tool.name), ...props.chosen.filter((name) => !known.has(name))]
  })
  const servers = createMemo(() => {
    const names = [...new Set(props.names.flatMap((tool) => (tool.server ? [tool.server] : [])))].sort()
    return names.map((server) => ({ server, tools: props.names.filter((tool) => tool.server === server).map((tool) => tool.name) }))
  })
  const toggle = (name: string) => props.onChange(props.chosen.includes(name) ? props.chosen.filter((item) => item !== name) : [...props.chosen, name])
  const toggleServer = (tools: string[]) => {
    const all = tools.every((tool) => props.chosen.includes(tool))
    props.onChange(all ? props.chosen.filter((name) => !tools.includes(name)) : [...new Set([...props.chosen, ...tools])])
  }
  return (
    <div class="space-y-3 py-3">
      <div>
        <div class="mb-1.5 text-[0.7rem] text-ink-faint">{t("drift.settings.prompts.builtinTools")}</div>
        <div class="flex flex-wrap gap-1.5">
          <For each={builtIn()}>{(name) => <Chip label={name} on={props.chosen.includes(name)} excluding={props.excluding} onToggle={() => toggle(name)} />}</For>
        </div>
      </div>
      <Show when={servers().length}>
        <div>
          <div class="mb-1.5 text-[0.7rem] text-ink-faint">{t("drift.settings.prompts.mcpTools")}</div>
          <div class="flex flex-wrap gap-1.5">
            <For each={servers()}>
              {(entry) => {
                const picked = () => entry.tools.filter((tool) => props.chosen.includes(tool)).length
                const count = () => (picked() && picked() < entry.tools.length ? `${picked()}/${entry.tools.length}` : String(entry.tools.length))
                return <Chip label={entry.server} count={count()} on={picked() === entry.tools.length} partial={picked() > 0 && picked() < entry.tools.length} excluding={props.excluding} onToggle={() => toggleServer(entry.tools)} />
              }}
            </For>
          </div>
        </div>
      </Show>
      <Show when={!props.chosen.length}>
        <div class="text-[0.72rem] text-warn">{t("drift.settings.prompts.tools.none")}</div>
      </Show>
    </div>
  )
}

function Chip(props: { label: string; count?: string; on: boolean; partial?: boolean; excluding: boolean; onToggle: () => void }) {
  return (
    <button
      type="button"
      class="flex items-center gap-1.5 rounded-md border px-2 py-0.5 font-mono text-[0.7rem] transition-colors"
      classList={{
        "border-accent/50 bg-accent/15 text-accent": props.on && !props.excluding,
        "border-danger/40 bg-danger/10 text-danger line-through": props.on && props.excluding,
        "border-accent/30 border-dashed text-ink-muted": !!props.partial,
        "border-edge text-ink-faint hover:border-edge-strong hover:text-ink-muted": !props.on && !props.partial,
      }}
      aria-pressed={props.on}
      onClick={props.onToggle}
    >
      {props.label}
      <Show when={props.count}>
        <span class="text-[0.65rem] text-ink-faint no-underline">{props.count}</span>
      </Show>
    </button>
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

/**
 * The reasoning levels to offer: the pinned model's, or with none pinned every level a connected
 * model has, since the agent runs on whichever the conversation uses. A saved level is always kept.
 */
export function reasoningLevels(state: Pick<EngineState, "providers" | "connected">, model: string, current: string) {
  const [providerID, ...rest] = model.split("/")
  const pinned = model ? modelInfo(state as EngineState, { providerID: providerID!, modelID: rest.join("/") }) : undefined
  const models = pinned ? [pinned] : state.providers.filter((provider) => state.connected.includes(provider.id)).flatMap((provider) => Object.values(provider.models))
  const found = new Set(models.flatMap((info) => Object.keys(info.variants ?? {})))
  if (current) found.add(current)
  const rank = (level: string) => (levelOrder.includes(level) ? levelOrder.indexOf(level) : levelOrder.length)
  return [...found].sort((a, b) => rank(a) - rank(b) || a.localeCompare(b))
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

/** A config as the form shows it. A tools list of `!name` entries only is every tool except those. */
export function draftOf(config: Record<string, unknown>): AgentDraft {
  const text = (value: unknown) => (typeof value === "string" ? value : "")
  const listed = Array.isArray(config.tools) ? config.tools.filter((tool): tool is string => typeof tool === "string" && tool !== "*") : []
  const excluding = listed.length > 0 && listed.every((tool) => tool.startsWith("!"))
  const toolMode: ToolMode = !listed.length ? "all" : excluding ? "except" : "only"
  return {
    prompt: text(config.prompt),
    model: text(config.model),
    variant: text(config.variant),
    steps: typeof config.steps === "number" ? String(config.steps) : "",
    toolMode,
    tools: excluding ? listed.map((tool) => tool.slice(1)) : listed.filter((tool) => !tool.startsWith("!")),
    permissions: Array.isArray(config.permissions) ? (config.permissions as PermissionRule[]) : [],
  }
}

/**
 * The config a draft stands for, or why it cannot be saved. An emptied model or reasoning level that
 * was set is kept as "": for a model that inherits the conversation's, for a level it clears the
 * default. "All tools" over a narrowed agent is `*`, since an empty list cannot be stored.
 */
export function configOf(draft: AgentDraft, baseline: Record<string, unknown>): Record<string, unknown> | string {
  const config: Record<string, unknown> = { prompt: draft.prompt }
  if (draft.model || baseline.model !== undefined) config.model = draft.model
  if (draft.variant || baseline.variant !== undefined) config.variant = draft.variant
  if (draft.steps) config.steps = Number(draft.steps)
  if (draft.toolMode !== "all" && !draft.tools.length) return t("drift.settings.prompts.tools.none")
  if (draft.toolMode === "only") config.tools = draft.tools
  if (draft.toolMode === "except") config.tools = draft.tools.map((tool) => `!${tool}`)
  if (draft.toolMode === "all" && baseline.tools !== undefined) config.tools = ["*"]
  const rules = draft.permissions.filter((rule) => rule.pattern.trim())
  if (rules.length || baseline.permissions !== undefined) config.permissions = rules
  const { prompt: _prompt, ...behavior } = config
  const issue = agentBehaviorIssue(behavior)
  return issue ? t("drift.settings.prompts.behaviorRefused", { field: issue }) : config
}

function sameDraft(a: AgentDraft, b: AgentDraft) {
  const same = (x: unknown, y: unknown) => JSON.stringify(x) === JSON.stringify(y)
  return a.prompt === b.prompt && a.model === b.model && a.variant === b.variant && a.steps === b.steps && a.toolMode === b.toolMode && same(a.tools, b.tools) && same(a.permissions, b.permissions)
}
