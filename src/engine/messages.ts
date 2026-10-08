import { unwrapErrorMessage } from "./error";

import type { components } from "./native/types";

export type Message = components["schemas"]["Message"];
export type MessageProblem = { text: string; interrupted: boolean };

/** The visible failure or interruption, derived without changing the stored message. */
export function messageProblem(message: Message): MessageProblem | undefined {
    if (message.role !== "assistant") return undefined;

    if (message.status === "aborted") return { text: "Interrupted", interrupted: true };
    if (message.status === "paused") return { text: message.error ?? "Paused", interrupted: true };
    if (message.status === "error") {
        const text = unwrapErrorMessage(message.error ?? "The turn failed") || "UnknownError";

        return { text, interrupted: false };
    }

    if (message.status !== "done") return undefined;
    if (message.ending === "length") {
        const text = unwrapErrorMessage(message.error ?? "The reply stopped at the output limit.");

        return { text: text || "MessageOutputLengthError", interrupted: false };
    }
    if (message.ending === "refused") {
        const text = unwrapErrorMessage(message.error ?? "The provider's safety filter ended the reply.");

        return { text: text || "UnknownError", interrupted: false };
    }
}

export function messageModel(message: Message) {
    return { providerID: message.model?.provider ?? "", modelID: message.model?.model ?? "" };
}
