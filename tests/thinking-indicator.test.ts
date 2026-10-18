import { expect, test } from "bun:test";

if (!("localStorage" in globalThis))
    Object.defineProperty(globalThis, "localStorage", {
        value: { getItem: () => null, setItem: () => undefined },
    });

test("stored thinking preferences keep what is known and fall back for the rest", async () => {
    const { thinkingPrefsFrom } = await import("../src/state/prefs");
    expect(thinkingPrefsFrom(null)).toEqual({ style: "shimmer", tint: "neutral", speed: "normal", label: true });
    expect(thinkingPrefsFrom({ style: "orbs", tint: "spectrum", speed: "fast", label: false })).toEqual({
        style: "orbs",
        tint: "spectrum",
        speed: "fast",
        label: false,
    });
    expect(thinkingPrefsFrom({ style: "plugin:aurora" }).style).toBe("plugin:aurora");
    expect(thinkingPrefsFrom({ style: "plugin:", tint: "red", speed: 3, label: "yes" })).toEqual({
        style: "shimmer",
        tint: "neutral",
        speed: "normal",
        label: true,
    });
});

test("a plugin's indicator is listed while registered, and only its own unregister removes it", async () => {
    const { registerThinkingIndicator, thinkingIndicator, thinkingIndicators } =
        await import("../src/thinking-indicators");
    const first = { name: "Aurora", render: () => undefined };
    const off = registerThinkingIndicator("aurora", first);
    expect(thinkingIndicators()).toContainEqual({ id: "aurora", name: "Aurora" });

    const second = { name: "Aurora 2", render: () => undefined };
    const offSecond = registerThinkingIndicator("aurora", second);
    off();
    expect(thinkingIndicator("aurora")).toBe(second);
    offSecond();
    expect(thinkingIndicator("aurora")).toBeUndefined();

    expect(() => registerThinkingIndicator("", first)).toThrow();
    expect(() => registerThinkingIndicator("broken", { name: "x" } as never)).toThrow();
});

test("a missing plugin indicator draws as Shimmer, and only figures can drop the word", async () => {
    const { shownThinkingStyle, thinkingWordShown } = await import("../src/ui/thinking-indicator");
    expect(shownThinkingStyle("plugin:aurora", () => false)).toBe("shimmer");
    expect(shownThinkingStyle("plugin:aurora", (id) => id === "aurora")).toBe("plugin:aurora");
    expect(shownThinkingStyle("orbs", () => false)).toBe("orbs");

    expect(thinkingWordShown("shimmer", false)).toBeTrue();
    expect(thinkingWordShown("still", false)).toBeTrue();
    expect(thinkingWordShown("orbs", false)).toBeFalse();
    expect(thinkingWordShown("orbs", true)).toBeTrue();
    expect(thinkingWordShown("plugin:aurora", true)).toBeFalse();
});

test("Settings offers every built-in style, then each plugin's, and plugins can register one", async () => {
    const { thinkingStyleItems } = await import("../src/ui/settings-thinking");
    const items = thinkingStyleItems([{ id: "aurora", name: "Aurora" }]);
    expect(items.map((item) => item.id)).toEqual([
        "shimmer",
        "orbs",
        "float",
        "dots",
        "pulse",
        "still",
        "plugin:aurora",
    ]);
    expect(items.at(-1)?.label).toBe("Aurora (plugin)");

    const plugins = await Bun.file("src/plugins.tsx").text();
    expect(plugins).toContainCode(
        "registerThinkingIndicator: (id, indicator) => track(registerThinkingIndicator(id, indicator))",
    );
});
