import { agentBehaviorIssue, applicableOverride } from "../state/prompts";
import { modelInfo } from "../engine/store";
import { t } from "../state/i18n";

import type { PermissionRule } from "../engine/native/client";
import type { AgentInfo, EngineState } from "../engine/store";
import type { PromptOverride } from "../state/prompts";

type ToolMode = "all" | "only" | "except";

/** Editable agent fields, including incomplete choices that have not been saved. */
export type AgentDraft = {
    prompt: string;
    model: string;
    variant: string;
    steps: string;
    toolMode: ToolMode;
    tools: string[];
    permissions: PermissionRule[];
};

const levelOrder = ["none", "minimal", "low", "medium", "high", "xhigh", "max"];

/** Returns the pinned model's reasoning levels, or all connected levels, retaining the saved choice. */
export function reasoningLevels(state: Pick<EngineState, "providers" | "connected">, model: string, current: string) {
    const [providerID, ...rest] = model.split("/");
    const pinned = model
        ? modelInfo(state as EngineState, { providerID: providerID!, modelID: rest.join("/") })
        : undefined;
    const models = pinned
        ? [pinned]
        : state.providers
              .filter((provider) => state.connected.includes(provider.id))
              .flatMap((provider) => Object.values(provider.models));

    const found = new Set(models.flatMap((info) => Object.keys(info.variants ?? {})));
    if (current) found.add(current);

    const rank = (level: string) => (levelOrder.includes(level) ? levelOrder.indexOf(level) : levelOrder.length);

    return [...found].sort((left, right) => rank(left) - rank(right) || left.localeCompare(right));
}

/** Groups agents as composer choices, delegated agents, then background agents. */
export function agentGroups(agents: AgentInfo[]) {
    const named = [...agents].sort((left, right) => left.name.localeCompare(right.name));

    return [
        { title: "settings.agents.title", agents: named.filter((agent) => !agent.hidden && agent.mode !== "subagent") },
        {
            title: "drift.settings.prompts.group.subagents",
            agents: named.filter((agent) => !agent.hidden && agent.mode === "subagent"),
        },
        { title: "drift.settings.prompts.group.background", agents: named.filter((agent) => agent.hidden) },
    ].filter((group) => group.agents.length);
}

/** Returns the effective agent fields that Settings can edit. */
export function agentConfig(agent: AgentInfo | undefined, stored?: PromptOverride): Record<string, unknown> {
    const restored =
        stored?.value && typeof stored.value === "object"
            ? applicableOverride(stored.value as Record<string, unknown>)
            : undefined;
    if (!agent) return restored ? { ...restored } : {};

    return {
        prompt: agent.prompt,
        model: agent.model ? `${agent.model.providerID}/${agent.model.modelID}` : undefined,
        steps: agent.steps,
        permissions: agent.permissions?.length ? agent.permissions : undefined,
        variant: agent.variant,
        // An empty engine list means every tool, not an explicit selection of none.
        tools: agent.tools.length ? agent.tools : undefined,
        ...restored,
    };
}

/** Converts config to form fields; a list containing only `!name` entries excludes those tools. */
export function draftOf(config: Record<string, unknown>): AgentDraft {
    const text = (value: unknown) => (typeof value === "string" ? value : "");
    const listed = Array.isArray(config.tools)
        ? config.tools.filter((tool): tool is string => typeof tool === "string" && tool !== "*")
        : [];
    const excluding = listed.length > 0 && listed.every((tool) => tool.startsWith("!"));
    const toolMode = draftToolMode(listed, excluding);

    return {
        prompt: text(config.prompt),
        model: text(config.model),
        variant: text(config.variant),
        steps: typeof config.steps === "number" ? String(config.steps) : "",
        toolMode,
        tools: excluding ? listed.map((tool) => tool.slice(1)) : listed.filter((tool) => !tool.startsWith("!")),
        permissions: Array.isArray(config.permissions) ? (config.permissions as PermissionRule[]) : [],
    };
}

/** Builds config or returns a validation error; cleared choices and all-tools selections remain explicit. */
export function configOf(draft: AgentDraft, baseline: Record<string, unknown>): Record<string, unknown> | string {
    const config: Record<string, unknown> = { prompt: draft.prompt };
    if (draft.model || baseline.model !== undefined) config.model = draft.model;
    if (draft.variant || baseline.variant !== undefined) config.variant = draft.variant;
    if (draft.steps) config.steps = Number(draft.steps);

    if (draft.toolMode !== "all" && !draft.tools.length) return t("drift.settings.prompts.tools.none");
    if (draft.toolMode === "only") config.tools = draft.tools;
    if (draft.toolMode === "except") config.tools = draft.tools.map((tool) => `!${tool}`);
    if (draft.toolMode === "all" && baseline.tools !== undefined) config.tools = ["*"];

    const rules = draft.permissions.filter((rule) => rule.pattern.trim());
    if (rules.length || baseline.permissions !== undefined) config.permissions = rules;

    const { prompt: _prompt, ...behavior } = config;
    const issue = agentBehaviorIssue(behavior);

    return issue ? t("drift.settings.prompts.behaviorRefused", { field: issue }) : config;
}

export function sameDraft(left: AgentDraft, right: AgentDraft) {
    const same = (first: unknown, second: unknown) => JSON.stringify(first) === JSON.stringify(second);

    return (
        left.prompt === right.prompt &&
        left.model === right.model &&
        left.variant === right.variant &&
        left.steps === right.steps &&
        left.toolMode === right.toolMode &&
        same(left.tools, right.tools) &&
        same(left.permissions, right.permissions)
    );
}

function draftToolMode(listed: string[], excluding: boolean): ToolMode {
    if (!listed.length) return "all";

    return excluding ? "except" : "only";
}
