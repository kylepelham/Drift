import { switchMessage } from "../src/engine/actions-providers";
import { drift as en } from "../src/i18n/en";
import { expect, test } from "bun:test";

test("each reason a turn moves to another account has its own notice", () => {
    const reasons = ["limit", "reset", "model", "refusal"] as const;
    const keys = reasons.map(switchMessage);

    expect(new Set(keys).size).toBe(reasons.length);
    for (const key of keys) expect(en[key as keyof typeof en]).toContain("{{account}}");
    expect(en[switchMessage("refusal") as keyof typeof en]).toContain("safety filter");
});
