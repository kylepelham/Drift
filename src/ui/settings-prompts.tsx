import { agentConfig, agentGroups, configOf, draftOf, reasoningLevels, sameDraft } from "./settings-agent-drafts";
import { createMemo, createSignal, For, onMount, Show, type JSX } from "solid-js";
import { agentModelCapability, agentModelOptions } from "../state/agent-models";
import { AddRule, newRule, RuleList } from "./settings-permission-rules";
import { SettingsGroup, SettingsRow } from "./settings-controls";
import { ServerRows, ToolRows } from "./settings-prompt-tools";
import { ListGroup, ListItem } from "./settings-prompt-list";
import { reasoningLevelLabel, t } from "../state/i18n";
import { activeWorkspace } from "../state/workspaces";
import { createStore } from "solid-js/store";
import { useEngine } from "../engine";
import { Picker } from "./picker";
import {
    agentOverrideValue,
    applicableOverride,
    loadPromptSnapshot,
    resetPromptOverride,
    savePromptOverride,
    type PromptSnapshot,
} from "../state/prompts";

import type { AgentDraft } from "./settings-agent-drafts";
import type { components } from "../engine/native/types";
import type { AgentInfo } from "../engine/store";

type BasePrompts = components["schemas"]["BasePrompts"];
type ToolName = components["schemas"]["ToolName"];

const familyLabels: Record<string, string> = {
    all: "drift.settings.prompts.family.all",
    codex: "drift.settings.prompts.family.codex",
    claude: "drift.settings.prompts.family.claude",
    gemini: "drift.settings.prompts.family.gemini",
    default: "drift.settings.prompts.family.default",
};

const stepPresets = ["10", "25", "50", "100", "200", "500"];
const pickerWidth = "13rem";
const editorClass =
    "w-full resize-y rounded-lg border border-edge bg-bg/50 p-3 font-mono text-xs leading-relaxed outline-none transition-colors focus:border-accent";

/** Edits base prompts and agents, retaining each item's changes until they are saved. */
export function PromptsSection() {
    const engine = useEngine();
    const [base, setBase] = createSignal<BasePrompts | null>(null);
    const [snapshot, setSnapshot] = createSignal<PromptSnapshot | null>(null);
    const [toolNames, setToolNames] = createSignal<ToolName[]>([]);
    const [selected, setSelected] = createSignal("base:all");
    const [baseDrafts, setBaseDrafts] = createStore<Record<string, string>>({});
    const [agentDrafts, setAgentDrafts] = createStore<Record<string, AgentDraft>>({});
    const [error, setError] = createSignal("");
    const [saved, setSaved] = createSignal(false);
    const [saving, setSaving] = createSignal(false);

    const loadSnapshot = async () => setSnapshot(await loadPromptSnapshot());
    onMount(() => {
        void run(async () => {
            setBase(await engine.actions.basePrompts());
            await loadSnapshot();
        }, false);
        void engine.actions.toolNames(activeWorkspace()?.path).then(setToolNames, () => undefined);
    });

    const override = (name: string) => snapshot()?.overrides.find((item) => item.key === `agent:${name}`);
    const agent = (name: string) => engine.state.agents.find((item) => item.name === name);
    const basePrompt = (id: string) => base()?.prompts.find((prompt) => prompt.id === id);
    const baseBaseline = (id: string) => basePrompt(id)?.custom ?? basePrompt(id)?.default ?? "";
    const agentBaseline = (name: string) => draftOf(agentConfig(agent(name), override(name)));
    const baseDirty = (id: string) => baseDrafts[id] !== undefined && baseDrafts[id] !== baseBaseline(id);
    const agentDirty = (name: string) => !!agentDrafts[name] && !sameDraft(agentDrafts[name]!, agentBaseline(name));
    const baseDraft = (id: string) => baseDrafts[id] ?? baseBaseline(id);
    const agentDraft = (name: string) => agentDrafts[name] ?? agentBaseline(name);

    async function run(action: () => Promise<void>, announce = true) {
        setSaving(true);
        setError("");
        setSaved(false);

        try {
            await action();
            setSaved(announce);
        } catch (cause) {
            setError(cause instanceof Error ? cause.message : String(cause));
        } finally {
            setSaving(false);
        }
    }

    function select(key: string) {
        setError("");
        setSaved(false);
        setSelected(key);
    }

    function saveBase(id: string) {
        const content = baseDraft(id);

        void run(async () => {
            setBase(await engine.actions.saveBasePrompt(id, content));
            setBaseDrafts(id, undefined!);
        });
    }

    function resetBase(id: string) {
        if (basePrompt(id)?.custom === undefined) return setBaseDrafts(id, undefined!);

        void run(async () => {
            setBase(await engine.actions.resetBasePrompt(id));
            setBaseDrafts(id, undefined!);
        });
    }

    function saveAgent(name: string) {
        const stored = override(name);
        const baseline = agentConfig(agent(name), stored);
        const built = configOf(agentDraft(name), baseline);
        if (typeof built === "string") return setError(built);

        const existing =
            stored?.value && typeof stored.value === "object"
                ? applicableOverride(stored.value as Record<string, unknown>)
                : {};
        const value = agentOverrideValue(built, baseline, existing);
        // Older baselines can contain retired fields that the shell now rejects.
        const recorded = stored?.original;
        const original =
            recorded && typeof recorded === "object"
                ? applicableOverride(recorded as Record<string, unknown>)
                : agentConfig(agent(name));
        const write = Object.keys(value).length
            ? () => savePromptOverride(`agent:${name}`, value, original)
            : () => resetPromptOverride(`agent:${name}`);

        void run(async () => {
            await write();
            await engine.actions.refreshAgents();
            await loadSnapshot();
            setAgentDrafts(name, undefined!);
        });
    }

    function resetAgent(name: string) {
        if (!override(name)) return setAgentDrafts(name, undefined!);

        void run(async () => {
            await resetPromptOverride(`agent:${name}`);
            await engine.actions.refreshAgents();
            await loadSnapshot();
            setAgentDrafts(name, undefined!);
        });
    }

    const groups = createMemo(() => agentGroups(engine.state.agents));
    const selectedBase = () => (selected().startsWith("base:") ? selected().slice(5) : undefined);
    const selectedAgent = () => (selected().startsWith("agent:") ? agent(selected().slice(6)) : undefined);

    return (
        <div class="flex flex-col gap-6 sm:flex-row">
            <nav class="flex shrink-0 flex-col sm:w-40" aria-label={t("drift.settings.prompts")}>
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
            <div class="min-w-0 flex-1">
                <Show when={selectedBase()}>
                    {(id) => (
                        <BaseEditor
                            id={id()}
                            draft={baseDraft(id())}
                            customized={basePrompt(id())?.custom !== undefined}
                            dirty={baseDirty(id())}
                            loaded={!!base()}
                            status={<Status error={error()} saved={saved()} />}
                            actions={
                                <Actions
                                    saving={saving()}
                                    dirty={baseDirty(id()) && !!baseDraft(id()).trim()}
                                    resettable={basePrompt(id())?.custom !== undefined || baseDirty(id())}
                                    onSave={() => saveBase(id())}
                                    onReset={() => resetBase(id())}
                                />
                            }
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
                            status={<Status error={error()} saved={saved()} />}
                            actions={
                                <Actions
                                    saving={saving()}
                                    dirty={agentDirty(item().name)}
                                    resettable={!!override(item().name) || agentDirty(item().name)}
                                    onSave={() => saveAgent(item().name)}
                                    onReset={() => resetAgent(item().name)}
                                />
                            }
                            onChange={(change) =>
                                setAgentDrafts(item().name, { ...agentDraft(item().name), ...change })
                            }
                        />
                    )}
                </Show>
            </div>
        </div>
    );
}

/** Shows the selected item's title, description, and save/reset actions. */
function EditorHeader(props: { title: string; description?: string; actions: JSX.Element }) {
    return (
        <div class="mb-6 flex items-start gap-4">
            <div class="min-w-0 flex-1">
                <div class="truncate text-base font-semibold text-ink">{props.title}</div>
                <Show when={props.description}>
                    <div class="mt-1 text-[0.78rem] leading-relaxed text-ink-faint">{props.description}</div>
                </Show>
            </div>
            {props.actions}
        </div>
    );
}

function Status(props: { error: string; saved: boolean }) {
    return (
        <>
            <Show when={props.error}>
                <div role="alert" class="mt-4 text-xs text-danger">
                    {props.error}
                </div>
            </Show>
            <Show when={props.saved}>
                <div role="status" class="mt-4 text-xs text-ok">
                    {t("drift.settings.prompts.saved")}
                </div>
            </Show>
        </>
    );
}

function BaseEditor(props: {
    id: string;
    draft: string;
    customized: boolean;
    dirty: boolean;
    loaded: boolean;
    status: JSX.Element;
    actions: JSX.Element;
    onInput: (value: string) => void;
}) {
    return (
        <div>
            <EditorHeader
                title={t(familyLabels[props.id] ?? props.id)}
                description={t(
                    props.id === "all"
                        ? "drift.settings.prompts.allDescription"
                        : "drift.settings.prompts.familyDescription",
                )}
                actions={props.actions}
            />
            <SettingsGroup title={t("drift.settings.prompts.systemPrompt")}>
                <div class="py-3">
                    <textarea
                        aria-label={t("drift.settings.prompts.systemPrompt")}
                        class={`${editorClass} h-80`}
                        classList={{
                            "text-ink": props.customized || props.dirty,
                            "text-ink-faint": !props.customized && !props.dirty,
                        }}
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
    );
}

function AgentEditor(props: {
    agent: AgentInfo;
    draft: AgentDraft;
    baseline: AgentDraft;
    customized: boolean;
    toolNames: ToolName[];
    status: JSX.Element;
    actions: JSX.Element;
    onChange: (change: Partial<AgentDraft>) => void;
}) {
    const engine = useEngine();
    const capability = () => agentModelCapability(props.agent);
    const inherited = () => t(inheritedModelLabel(props.agent.name));
    const models = createMemo(() => [
        { id: "", label: inherited() },
        ...agentModelOptions(engine.state, capability() ?? "tools"),
    ]);
    const levels = createMemo(() => [
        { id: "", label: t("drift.settings.prompts.variantPlaceholder") },
        ...reasoningLevels(engine.state, props.draft.model, props.draft.variant).map((level) => ({
            id: level,
            label: reasoningLevelLabel(level),
        })),
    ]);
    const steps = createMemo(() => [
        { id: "", label: t("drift.settings.prompts.stepsPlaceholder") },
        ...[...new Set([...stepPresets, props.draft.steps].filter(Boolean))]
            .sort((a, b) => Number(a) - Number(b))
            .map((step) => ({ id: step, label: step })),
    ]);
    const changed = () => props.draft.prompt !== props.baseline.prompt;
    // Background jobs only answer in text, so they have no reasoning level, steps, tools or permissions.
    const runsTools = () => capability() !== "text";

    return (
        <div class="space-y-6">
            <div>
                <EditorHeader title={props.agent.name} description={props.agent.description} actions={props.actions} />
                <Show when={props.agent.problem}>
                    <div
                        role="alert"
                        class="-mt-3 rounded-md border border-danger/40 bg-danger/10 px-3 py-2 text-xs text-danger"
                    >
                        {props.agent.problem}
                    </div>
                </Show>
            </div>
            <SettingsGroup title={t("drift.settings.prompts.behaviorGroup")}>
                <Show when={capability()}>
                    <SettingsRow
                        title={t("command.category.model")}
                        description={t("drift.settings.prompts.modelDescription")}
                    >
                        <Picker
                            label={t("command.category.model")}
                            items={models()}
                            selected={props.draft.model}
                            fallbackLabel={props.draft.model || inherited()}
                            floating
                            bordered
                            chevronAtEnd
                            placement="below"
                            width={pickerWidth}
                            onPick={(model) => props.onChange({ model })}
                        />
                    </SettingsRow>
                </Show>
                <Show when={runsTools()}>
                    <SettingsRow
                        title={t("drift.settings.prompts.variant")}
                        description={t("drift.settings.prompts.variantDescription")}
                    >
                        <Picker
                            label={t("drift.settings.prompts.variant")}
                            items={levels()}
                            selected={props.draft.variant}
                            floating
                            bordered
                            chevronAtEnd
                            placement="below"
                            width={pickerWidth}
                            onPick={(variant) => props.onChange({ variant })}
                        />
                    </SettingsRow>
                    <SettingsRow
                        title={t("drift.settings.prompts.steps")}
                        description={t("drift.settings.prompts.stepsDescription")}
                    >
                        <Picker
                            label={t("drift.settings.prompts.steps")}
                            items={steps()}
                            selected={props.draft.steps}
                            floating
                            bordered
                            chevronAtEnd
                            placement="below"
                            width={pickerWidth}
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
                        classList={{
                            "text-ink": props.customized || changed(),
                            "text-ink-faint": !props.customized && !changed(),
                        }}
                        spellcheck={false}
                        placeholder={t("drift.settings.prompts.inheritsFamily")}
                        value={props.draft.prompt}
                        onInput={(event) => props.onChange({ prompt: event.currentTarget.value })}
                    />
                </div>
            </SettingsGroup>
            <Show when={runsTools()}>
                <SettingsGroup title={t("drift.settings.prompts.tools")}>
                    <SettingsRow
                        title={t("drift.settings.prompts.toolsOffered")}
                        description={t("drift.settings.prompts.toolsDescription")}
                    >
                        <Picker
                            label={t("drift.settings.prompts.toolsOffered")}
                            items={(["all", "only", "except"] as const).map((mode) => ({
                                id: mode,
                                label: t(`drift.settings.prompts.tools.${mode}`),
                            }))}
                            selected={props.draft.toolMode}
                            floating
                            bordered
                            chevronAtEnd
                            placement="below"
                            width={pickerWidth}
                            onPick={(mode) =>
                                props.onChange({
                                    toolMode: mode as AgentDraft["toolMode"],
                                    tools: mode === "all" ? [] : props.draft.tools,
                                })
                            }
                        />
                    </SettingsRow>
                    <Show when={props.draft.toolMode !== "all"}>
                        <ToolRows
                            names={props.toolNames}
                            chosen={props.draft.tools}
                            onChange={(tools) => props.onChange({ tools })}
                        />
                        <Show when={!props.draft.tools.length}>
                            <div class="px-1 py-2.5 text-[0.72rem] text-warn">
                                {t("drift.settings.prompts.tools.none")}
                            </div>
                        </Show>
                    </Show>
                </SettingsGroup>
                <Show when={props.draft.toolMode !== "all"}>
                    <ServerRows
                        names={props.toolNames}
                        chosen={props.draft.tools}
                        onChange={(tools) => props.onChange({ tools })}
                    />
                </Show>
                <SettingsGroup title={t("drift.settings.permissions")}>
                    <RuleList
                        rules={props.draft.permissions}
                        onChange={(permissions) => props.onChange({ permissions })}
                    />
                    <div class="px-1 py-2.5">
                        <AddRule
                            onAdd={() => props.onChange({ permissions: [...props.draft.permissions, newRule()] })}
                        />
                    </div>
                </SettingsGroup>
            </Show>
            {props.status}
        </div>
    );
}

function Actions(props: {
    saving: boolean;
    dirty: boolean;
    resettable: boolean;
    onSave: () => void;
    onReset: () => void;
}) {
    return (
        <div class="flex shrink-0 gap-2">
            <Show when={props.resettable}>
                <button
                    class="rounded-md border border-edge px-3 py-1.5 text-xs text-ink-muted transition-colors hover:border-edge-strong hover:text-ink disabled:opacity-40"
                    disabled={props.saving}
                    onClick={() => props.onReset()}
                >
                    {t("common.reset")}
                </button>
            </Show>
            <button
                class="rounded-md bg-accent px-3 py-1.5 text-xs font-medium text-accent-ink disabled:opacity-40"
                disabled={props.saving || !props.dirty}
                onClick={() => props.onSave()}
            >
                {t("common.save")}
            </button>
        </div>
    );
}

function inheritedModelLabel(agent: string) {
    if (agent === "title") return "drift.settings.agents.automaticSmallModel";
    if (agent === "compaction") return "drift.settings.agents.currentSessionModel";

    return "drift.settings.agents.currentModel";
}
