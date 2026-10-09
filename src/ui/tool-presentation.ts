import { toolInput, toolMetadata } from "../engine/parts";

import type { ToolPart } from "../engine/parts";

/** Status, result and timing for a tool row, not a replacement for its native record. */
export function toolDisplay(part: ToolPart) {
    const input = toolInput(part);
    const metadata = toolMetadata(part);
    const start = part.status === "pending" ? undefined : (part.startedAt ?? undefined);
    const active = part.status === "pending" || part.status === "running";
    const end = active ? undefined : (part.finishedAt ?? start);
    let status: "pending" | "running" | "completed" | "error" = "error";
    if (part.status === "pending" || part.status === "running") status = part.status;
    if (part.status === "done") status = "completed";

    return {
        status,
        input,
        metadata,
        title: part.title ?? part.name,
        output: part.output ?? "",
        error: part.output ?? "Failed",
        time: { start, end },
    };
}
