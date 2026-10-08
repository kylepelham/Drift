import { createStore, reconcile, unwrap } from "solid-js/store";
import { messageProblem } from "../engine/messages";
import { contextTools } from "./tool-labels";
import { createSignal } from "solid-js";
import { partVisible } from "./parts";

import type { Part, ToolPart } from "../engine/parts";
import type { MessageEntry } from "../engine/store";

export type PartGroup = { id: string; key: string; explored: ToolPart[] } | { id: string; key: string; part: Part };
export type PartGroupSlot = {
    id: string;
    value: PartGroup;
    revision?: () => number;
    update: (value: PartGroup) => void;
};

export function groupParts(parts: Part[]): PartGroup[] {
    const groups: PartGroup[] = [];

    for (const part of parts) {
        if (!partVisible(part)) continue;

        if (part.type === "tool_call" && contextTools.has(part.name)) {
            const last = groups.at(-1);
            if (last && "explored" in last) last.explored.push(part);
            else {
                const id = `explored:${part.id}`;
                groups.push({ id, key: id, explored: [part] });
            }

            continue;
        }

        groups.push({ id: part.id, key: part.id, part });
    }

    return groups;
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

export function groupAssistantEntries(entries: MessageEntry[]) {
    const result = new Map<string, PartGroup[]>();
    let previous: MessageEntry | undefined;
    let trailing: Extract<PartGroup, { explored: ToolPart[] }> | undefined;

    for (const entry of entries) {
        if (entry.info.role !== "assistant") {
            previous = entry;
            trailing = undefined;
            continue;
        }

        if (!previous || !assistantFlowContinues(previous, entry)) trailing = undefined;
        const groups: PartGroup[] = [];
        for (const group of groupParts(entry.parts)) {
            if ("explored" in group && trailing) {
                trailing.explored.push(...group.explored);
                continue;
            }

            groups.push(group);
            trailing = "explored" in group ? group : undefined;
        }

        result.set(entry.info.id, groups);
        previous = entry;
    }

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
    const exploredByPart = new Map<string, { index: number; slot: PartGroupSlot }>();
    previous.forEach((slot, index) => {
        if (!("explored" in slot.value)) return;

        slot.value.explored.forEach((part) => exploredByPart.set(part.id, { index, slot }));
    });

    // When a group splits, only its anchor-containing fragment may reuse the mounted slot.
    const reserved = new Map<string, number>();
    previous.forEach((slot) => {
        if (!("explored" in slot.value)) return;

        const owner = groups.findIndex(
            (group) => "explored" in group && group.explored.some((part) => `explored:${part.id}` === slot.id),
        );
        if (owner !== -1) reserved.set(slot.id, owner);
    });

    const claimed = new Set<string>();
    const active = new Set<string>();
    const next = groups.map((input, index) => {
        const existing =
            "explored" in input
                ? input.explored.reduce<{ index: number; slot: PartGroupSlot } | undefined>((result, part) => {
                      const candidate = exploredByPart.get(part.id);
                      if (!candidate || claimed.has(candidate.slot.id)) return result;

                      const owner = reserved.get(candidate.slot.id);
                      if (owner !== undefined && owner !== index) return result;

                      return !result || candidate.index < result.index ? candidate : result;
                  }, undefined)?.slot
                : slots.get(input.id);
        const group = existing && input.id !== existing.id ? { ...input, id: existing.id, key: existing.id } : input;
        if (existing && "explored" in input) claimed.add(existing.id);
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
