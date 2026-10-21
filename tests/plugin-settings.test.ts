import { configFrom, draftFrom } from "../src/state/plugin-settings";
import { expect, test } from "bun:test";

import type { PluginSetting } from "../src/state/plugin-settings";

const settings: PluginSetting[] = [
    {
        key: "url",
        label: "URL",
        kind: { type: "text", default: "http://x", placeholder: "", multiline: false, secret: false },
    },
    { key: "limit", label: "Limit", kind: { type: "number", default: 5, integer: true } },
    { key: "on", label: "On", kind: { type: "toggle", default: true } },
    { key: "words", label: "Words", kind: { type: "items", default: ["a"], placeholder: "", secret: false } },
    {
        key: "swap",
        label: "Swap",
        kind: {
            type: "pairs",
            default: [],
            keyLabel: "Secret",
            valueLabel: "Placeholder",
            secretKeys: true,
            secretValues: false,
        },
    },
];

test("a draft shows stored values and fills the rest with defaults", () => {
    const draft = draftFrom(settings, { limit: 9, swap: { hunter2: "PASSWORD" } });

    expect(draft).toEqual({
        url: "http://x",
        limit: "9",
        on: true,
        words: ["a"],
        swap: [{ key: "hunter2", value: "PASSWORD" }],
    });
});

test("saving drops empty rows, removes a cleared number and keeps undeclared keys", () => {
    const draft = {
        url: "http://y",
        limit: " ",
        on: false,
        words: ["b", " ", "c"],
        swap: [
            { key: "hunter2", value: "PASSWORD" },
            { key: "", value: "orphan" },
        ],
    };

    expect(configFrom(settings, draft, { limit: 5, extra: 1 })).toEqual({
        url: "http://y",
        on: false,
        words: ["b", "c"],
        swap: { hunter2: "PASSWORD" },
        extra: 1,
    });
});

test("a number that does not parse goes to the engine as typed, so it can name the setting", () => {
    const config = configFrom(settings, { ...draftFrom(settings, {}), limit: "lots" }, {});

    expect(config.limit).toBe("lots");
});
