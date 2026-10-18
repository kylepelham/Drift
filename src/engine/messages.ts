import { unwrapErrorMessage } from "./error";
import { t } from "../state/i18n";

import type { components } from "./native/types";

export type Message = components["schemas"]["Message"];
export type MessageProblem = { text: string; interrupted: boolean };

/** The visible failure or interruption, derived without changing the stored message. */
export function messageProblem(message: Message): MessageProblem | undefined {
    if (message.role !== "assistant") return undefined;

    if (message.status === "aborted") return { text: t("drift.message.interrupted"), interrupted: true };
    if (message.status === "paused") return { text: message.error ?? t("drift.message.paused"), interrupted: true };
    if (message.status === "error") return failure(message.error, t("drift.message.turnFailed"));

    if (message.status !== "done") return undefined;
    if (message.ending === "length") return failure(message.error, t("drift.message.outputLimit"));
    if (message.ending === "refused") return failure(message.error, t("drift.message.refused"));
}

function failure(error: string | null | undefined, fallback: string): MessageProblem {
    return { text: unwrapErrorMessage(error ?? fallback) || t("drift.error.unknown"), interrupted: false };
}

export function messageModel(message: Message) {
    return { providerID: message.model?.provider ?? "", modelID: message.model?.model ?? "" };
}
