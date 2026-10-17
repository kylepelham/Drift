import { t } from "../state/i18n";

import type { ReviewPanel, ReviewSource } from "../state/panels";
import type { ReviewRefs } from "../engine/native/client";
import type { PickerItem } from "./picker";

export function sourceId(source: ReviewSource) {
    if (source.kind === "branch" || source.kind === "commit") return `${source.kind}:${source.target}`;

    return source.kind;
}

export function sourceFromId(id: string, turn: ReviewPanel["turn"]): ReviewSource | undefined {
    if (id === "turn") return turn;
    if (id === "uncommitted") return { kind: "uncommitted" };
    const [kind, ...rest] = id.split(":");
    const target = rest.join(":");
    if ((kind === "branch" || kind === "commit") && target) return { kind, target };
}

export function sourceItems(refs: ReviewRefs | undefined, turn: boolean): PickerItem[] {
    const items: PickerItem[] = [];
    if (turn) items.push({ id: "turn", label: t("drift.review.source.turn") });
    if (!refs?.repo) return items;

    items.push({ id: "uncommitted", label: t("drift.review.source.uncommitted") });
    for (const branch of refs.branches)
        items.push({ id: `branch:${branch}`, label: branch, group: t("drift.review.source.branch") });
    for (const commit of refs.commits)
        items.push({
            id: `commit:${commit.id}`,
            label: commit.subject,
            detail: `${commit.short} · ${commit.author}`,
            group: t("drift.review.source.commit"),
        });

    return items;
}

/** A review path, relative to its workspace, names `focus` when it is the same file or its tail. */
export function samePath(path: string, focus: string) {
    const normalize = (value: string) => value.replaceAll("\\", "/").toLowerCase();
    const [left, right] = [normalize(path), normalize(focus)];

    return left === right || right.endsWith(`/${left}`);
}
