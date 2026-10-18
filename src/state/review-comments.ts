import { composerDraft, composerScope, patchComposerDraft, type ReviewComment } from "./composer";
import { activeWorkspace } from "./workspaces";
import { selectedSession } from "./selection";

/** The draft the review panel's comments join: the composer of the conversation beside it. */
const reviewScope = () => composerScope(selectedSession(), activeWorkspace()?.id);

export const reviewComments = () => composerDraft(reviewScope()).comments;

/** Adds a note, or replaces the one with the same id. */
export function saveReviewComment(comment: ReviewComment, scope = reviewScope()) {
    const comments = composerDraft(scope).comments;
    const kept = comments.some((item) => item.id === comment.id)
        ? comments.map((item) => (item.id === comment.id ? comment : item))
        : [...comments, comment];
    patchComposerDraft(scope, { comments: kept });
}

export function removeReviewComment(id: string, scope = reviewScope()) {
    patchComposerDraft(scope, { comments: composerDraft(scope).comments.filter((item) => item.id !== id) });
}

/** Whether a note was made on this row of a diff: the same file, side, number and line. */
export function commentOn(comment: ReviewComment, path: string, row: { kind: string; line: number; text: string }) {
    return comment.path === path && comment.side === row.kind && comment.line === row.line && comment.code === row.text;
}

const sideNames: Record<ReviewComment["side"], string> = { add: "added", del: "removed", ctx: "unchanged" };

/** The notes as the model reads them: each file and line, the line itself, then what the user wrote. */
export function reviewCommentsText(comments: ReviewComment[]) {
    const notes = comments.map((comment) =>
        [
            `${comment.path} line ${comment.line} (${sideNames[comment.side]}):`,
            `    ${comment.code}`,
            comment.text,
        ].join("\n"),
    );

    return ["Comments on specific lines of the code, from the review panel:", ...notes].join("\n\n");
}

/** One text attachment carrying every note, so the sent message shows a single chip rather than the notes. */
export function reviewCommentsFile(comments: ReviewComment[]) {
    const bytes = new TextEncoder().encode(reviewCommentsText(comments));
    const binary = Array.from(bytes, (byte) => String.fromCharCode(byte)).join("");

    return { filename: "review-comments.md", mime: "text/plain", url: `data:text/plain;base64,${btoa(binary)}` };
}
