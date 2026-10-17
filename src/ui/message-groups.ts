import { createStore, reconcile, unwrap } from "solid-js/store";
import { toolGroups, type ToolGroupKind } from "../state/prefs";
import { contextTools, toolGroupKind } from "./tool-labels";
import { messageProblem } from "../engine/messages";
import { createSignal } from "solid-js";
import { partVisible } from "./parts";

import type { Part, ToolPart } from "../engine/parts";
import type { MessageEntry } from "../engine/store";

/** `explored`: consecutive reads. `run`: consecutive calls of the kinds the user groups, two or more. */
export type PartGroup =
    | { id: string; key: string; explored: ToolPart[] }
    | { id: string; key: string; run: ToolPart[] }
    | { id: string; key: string; part: Part };
export type PartGroupSlot = {
    id: string;
    value: PartGroup;
    revision?: () => number;
    update: (value: PartGroup) => void;
};
type GroupKinds = Record<ToolGroupKind, boolean>;

/** The calls a collapsible group holds; none for a single part. */
export function groupMembers(group: PartGroup) {
    if ("explored" in group) return group.explored;
    if ("run" in group) return group.run;
}

function memberKind(group: PartGroup) {
    if ("explored" in group) return "explored" as const;
    if ("run" in group) return "run" as const;
}

function runnable(part: Part, kinds: GroupKinds): part is ToolPart {
    if (part.type !== "tool_call") return false;
    const kind = toolGroupKind(part.name);
    return !!kind && kinds[kind];
}

/** Groups before a run of one goes back to being a plain row. */
function collectParts(parts: Part[], kinds: GroupKinds): PartGroup[] {
    const groups: PartGroup[] = [];

    for (const part of parts) {
        if (!partVisible(part)) continue;

        const last = groups.at(-1);
        if (part.type === "tool_call" && contextTools.has(part.name)) {
            if (last && "explored" in last) last.explored.push(part);
            else groups.push({ id: `explored:${part.id}`, key: `explored:${part.id}`, explored: [part] });
            continue;
        }
        if (runnable(part, kinds)) {
            if (last && "run" in last) last.run.push(part);
            else groups.push({ id: `run:${part.id}`, key: `run:${part.id}`, run: [part] });
            continue;
        }

        groups.push({ id: part.id, key: part.id, part });
    }

    return groups;
}

function settleRuns(groups: PartGroup[]): PartGroup[] {
    return groups.map((group) =>
        "run" in group && group.run.length === 1
            ? { id: group.run[0].id, key: group.run[0].id, part: group.run[0] }
            : group,
    );
}

export function groupParts(parts: Part[], kinds: GroupKinds = toolGroups()): PartGroup[] {
    return settleRuns(collectParts(parts, kinds));
}

function assistantBoundary(entry: MessageEntry) {
    if (entry.info.role !== "assistant") return true;

    const info = entry.info;

    return !!info.summary || !!messageProblem(info) || entry.parts.some((part) => part.type === "compaction");
}

export function assistantFlowContinues(previous: MessageEntry, next: MessageEntry) {
    return (
        previous.info.role === "assistant" &&
        next.info.role === "assistant" &&
        !assistantBoundary(previous) &&
        !assistantBoundary(next)
    );
}

export function groupAssistantEntries(entries: MessageEntry[], kinds: GroupKinds = toolGroups()) {
    const result = new Map<string, PartGroup[]>();
    let previous: MessageEntry | undefined;
    let trailing: PartGroup | undefined;

    for (const entry of entries) {
        if (entry.info.role !== "assistant") {
            previous = entry;
            trailing = undefined;
            continue;
        }

        if (!previous || !assistantFlowContinues(previous, entry)) trailing = undefined;
        const groups: PartGroup[] = [];
        for (const group of collectParts(entry.parts, kinds)) {
            const kind = memberKind(group);
            if (kind && trailing && memberKind(trailing) === kind) {
                groupMembers(trailing)!.push(...groupMembers(group)!);
                continue;
            }

            groups.push(group);
            trailing = kind ? group : undefined;
        }

        result.set(entry.info.id, groups);
        previous = entry;
    }

    // A run is only complete once every step that continues it has been read.
    for (const [id, groups] of result) result.set(id, settleRuns(groups));

    return result;
}

function createPartGroupSlot(group: PartGroup): PartGroupSlot {
    const [value, setValue] = createStore(group);
    const [revision, setRevision] = createSignal(0);

    return {
        id: group.id,
        value,
        revision,
        update: (updated) => {
            setValue(reconcile(unwrap(updated)));

            // Nested proxy replacement needs an explicit invalidation without remounting the row.
            setRevision((value) => value + 1);
        },
    };
}

export function updatePartGroupSlots(
    groups: PartGroup[],
    slots: Map<string, PartGroupSlot>,
    createSlot = createPartGroupSlot,
) {
    const previous = [...slots.values()];
    const memberOf = new Map<string, { index: number; slot: PartGroupSlot }>();
    previous.forEach((slot, index) => {
        groupMembers(slot.value)?.forEach((part) => memberOf.set(part.id, { index, slot }));
    });

    // When a group splits, only its anchor-containing fragment may reuse the mounted slot.
    const reserved = new Map<string, number>();
    previous.forEach((slot) => {
        const kind = memberKind(slot.value);
        if (!kind) return;

        const owner = groups.findIndex(
            (group) =>
                memberKind(group) === kind && groupMembers(group)!.some((part) => `${kind}:${part.id}` === slot.id),
        );
        if (owner !== -1) reserved.set(slot.id, owner);
    });

    const claimed = new Set<string>();
    const active = new Set<string>();
    const next = groups.map((input, index) => {
        const members = groupMembers(input);
        const existing = members
            ? members.reduce<{ index: number; slot: PartGroupSlot } | undefined>((result, part) => {
                  const candidate = memberOf.get(part.id);
                  if (!candidate || claimed.has(candidate.slot.id)) return result;
                  if (memberKind(candidate.slot.value) !== memberKind(input)) return result;

                  const owner = reserved.get(candidate.slot.id);
                  if (owner !== undefined && owner !== index) return result;

                  return !result || candidate.index < result.index ? candidate : result;
              }, undefined)?.slot
            : slots.get(input.id);
        const group = existing && input.id !== existing.id ? { ...input, id: existing.id, key: existing.id } : input;
        if (existing && members) claimed.add(existing.id);
        active.add(group.id);

        if (existing) {
            existing.update(group);
            return existing;
        }

        const slot = createSlot(group);
        slots.set(group.id, slot);

        return slot;
    });

    for (const id of slots.keys()) if (!active.has(id)) slots.delete(id);
    for (const slot of next) {
        slots.delete(slot.id);
        slots.set(slot.id, slot);
    }

    return next;
}
