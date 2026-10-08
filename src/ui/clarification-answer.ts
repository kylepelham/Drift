import type { MessageEntry } from "../engine/store";

type ClarificationItem = { header: string; question: string; answers: string[] };
export type ClarificationAnswer = { items: ClarificationItem[]; text: string; preview: string; spawned?: boolean };

export function clarificationAnswer(entry: MessageEntry): ClarificationAnswer | undefined {
    // Held worker results can ride along with an answer; they are not the user's words.
    const visible = entry.parts.filter((part) => part.type !== "task_result" && part.type !== "unknown");
    if (entry.info.role !== "user" || visible.length !== 1) return;

    const part = visible[0];
    if (part.type === "clarification") {
        if (!part.items.length) return;

        const items = part.items.map((item) => ({ ...item, answers: [...item.answers] }));

        return {
            items,
            text: items.map((item) => `${item.question}\n${item.answers.join(", ")}`).join("\n\n"),
            preview: items.flatMap((item) => item.answers).join(", "),
        };
    }

    if (part.type !== "text") return;

    // Earlier builds persisted only this protocol text. Preserve its body without guessing Q&A boundaries.
    const legacy = /^Answer to clarification que_[a-zA-Z0-9]+:\r?\n([\s\S]+)$/.exec(part.text);
    if (legacy) return { items: [], text: legacy[1], preview: "" };
}
