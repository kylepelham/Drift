import { t } from "../state/i18n";

import type { ReviewRefs } from "../engine/native/client";
import type { ReviewSource } from "../state/panels";

/** The branch "All changes" compares with: the repository's main line when it has one, else the first other branch. */
export function defaultBase(refs: ReviewRefs | undefined) {
    if (!refs?.repo) return undefined;
    const preferred = ["main", "master", "origin/main", "origin/master", "develop", "origin/develop"];

    return preferred.find((name) => refs.branches.includes(name)) ?? refs.branches[0];
}

/** What the picker's button says: a branch comparison reads base to current, a commit by its id and subject. */
export function sourceTitle(source: ReviewSource, refs: ReviewRefs | undefined) {
    if (source.kind === "turn") return { text: t("drift.review.source.turn") };
    if (source.kind === "uncommitted") return { text: t("drift.review.source.uncommitted") };
    if (source.kind === "files") return { text: t("drift.review.source.files") };
    if (source.kind === "branch") return { text: source.target, to: refs?.branch ?? "HEAD" };

    const commit = refs?.commits.find((item) => item.id === source.target);
    return { text: commit ? `${commit.short} ${commit.subject}` : source.target.slice(0, 7) };
}

/** Whether two sources show the same changes; a turn is the same turn by its prompt. */
export function sameSource(left: ReviewSource, right: ReviewSource) {
    if (left.kind !== right.kind) return false;
    if (left.kind === "turn" && right.kind === "turn") return left.messageId === right.messageId;
    if ("target" in left && "target" in right) return left.target === right.target;

    return true;
}

/** Items whose text holds every word of `query`, any case. */
export function matching<T>(items: T[], query: string, text: (item: T) => string) {
    const words = query.toLowerCase().split(/\s+/).filter(Boolean);
    return items.filter((item) => words.every((word) => text(item).toLowerCase().includes(word)));
}

/** A review path, relative to its workspace, names `focus` when it is the same file or its tail. */
export function samePath(path: string, focus: string) {
    const normalize = (value: string) => value.replaceAll("\\", "/").toLowerCase();
    const [left, right] = [normalize(path), normalize(focus)];

    return left === right || right.endsWith(`/${left}`);
}
