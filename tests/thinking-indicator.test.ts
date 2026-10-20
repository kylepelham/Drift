import { expect, test } from "bun:test";

if (!("localStorage" in globalThis))
    Object.defineProperty(globalThis, "localStorage", {
        value: { getItem: () => null, setItem: () => undefined },
    });

test("stored thinking preferences keep what is known and fall back for the rest", async () => {
    const { thinkingPrefsFrom } = await import("../src/state/prefs");
    const defaults = { style: "orbs", tint: "neutral", speed: "normal", label: false, shimmer: true, text: "" };
    expect(thinkingPrefsFrom(null)).toEqual(defaults);
    const chosen = { style: "dots", tint: "spectrum", speed: "fast", label: true, shimmer: false, text: "Pondering" };
    expect(thinkingPrefsFrom(chosen)).toEqual(chosen);
    expect(thinkingPrefsFrom({ style: "plugin:aurora", shimmer: true }).style).toBe("plugin:aurora");
    expect(thinkingPrefsFrom({ style: "plugin:", tint: "red", speed: 3, label: "yes", shimmer: 1, text: 4 })).toEqual(
        defaults,
    );

    // Shimmer and Still were styles; prefs from then had no shimmer setting and showed the word by default.
    expect(thinkingPrefsFrom({ style: "shimmer", tint: "accent", speed: "slow", label: true })).toEqual({
        ...defaults,
        tint: "accent",
        speed: "slow",
    });
    expect(thinkingPrefsFrom({ style: "still", label: true }).style).toBe("orbs");
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

    const again = registerThinkingIndicator("aurora", first);
    const offAgain = registerThinkingIndicator("aurora", first);
    again();
    expect(thinkingIndicator("aurora")).toBe(first);
    offAgain();
    expect(thinkingIndicator("aurora")).toBeUndefined();

    expect(() => registerThinkingIndicator("", first)).toThrow();
    expect(() => registerThinkingIndicator("broken", { name: "x" } as never)).toThrow();
});

test("a missing plugin indicator or a retired style draws as Orbs, and the text is the user's or Thinking", async () => {
    const { shownThinkingStyle, thinkingText } = await import("../src/ui/thinking-indicator");
    expect(shownThinkingStyle("plugin:aurora", () => false)).toBe("orbs");
    expect(shownThinkingStyle("plugin:aurora", (id) => id === "aurora")).toBe("plugin:aurora");
    expect(shownThinkingStyle("pulse", () => false)).toBe("pulse");
    expect(shownThinkingStyle("shimmer", () => false)).toBe("orbs");

    expect(thinkingText({ text: "" })).toBe("Thinking");
    expect(thinkingText({ text: "   " })).toBe("Thinking");
    expect(thinkingText({ text: " Mulling it over " })).toBe("Mulling it over");

    const css = await Bun.file("src/styles/app.css").text();
    expect(css).toMatch(
        /\.thinking-piece, \.thinking-label, \[data-component="text-shimmer"\]\) \{\s*animation: none !important;/,
    );
    expect(css).toContain('.thinking-figure[data-look="orbs"] .thinking-piece');
    expect(css).not.toContain("data-style=");
});

test("Spectrum's hue turn is a number, since a relative colour's `h` plus an angle is invalid and paints nothing", async () => {
    const css = await Bun.file("src/styles/app.css").text();
    expect(css).toContain("calc(h + var(--thinking-hue))");
    expect(css).toMatch(/@property --thinking-hue \{\s*syntax: "<number>";/);
    expect(css).toMatch(/--thinking-hue: 360;/);
});

test("Settings offers every built-in style, then each plugin's, and plugins can register one", async () => {
    const { thinkingStyleItems } = await import("../src/ui/settings-thinking");
    const items = thinkingStyleItems([{ id: "aurora", name: "Aurora" }]);
    expect(items.map((item) => item.id)).toEqual(["orbs", "float", "dots", "pulse", "plugin:aurora"]);
    expect(items.at(-1)?.label).toBe("Aurora (plugin)");

    const plugins = await Bun.file("src/plugins.tsx").text();
    expect(plugins).toContainCode(
        "registerThinkingIndicator: (id, indicator) => track(registerThinkingIndicator(id, indicator))",
    );
});
