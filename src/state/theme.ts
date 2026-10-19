import { createEffect, createSignal, onCleanup } from "solid-js";
import { publishMirrorTheme, type MirrorTheme } from "./mirror";
import { persisted } from "./persist";

/** The bases a theme is built on: surfaces and text; the accent is chosen apart. */
export const themes = [
    "drift-charcoal",
    "drift-black",
    "drift-dim",
    "drift-mocha",
    "drift-midnight",
    "drift-forest",
    "drift-plum",
    "drift-light",
    "drift-paper",
    "drift-custom",
] as const;
export type ThemeName = (typeof themes)[number];

/** Named accents; a `#rrggbb` colour is the user's own, and an empty one the base's. */
export const accents = ["blue", "violet", "pink", "red", "orange", "amber", "green", "teal", "clay", "silver"] as const;

export type CornerStyle = "sharp" | "" | "round";

export type CustomTheme = { background: string; surface: string; text: string; accent: string };

/** Names saved before 2.4.0 and the base and accent each became. */
const renamed: Record<string, [ThemeName, string]> = {
    "drift-dark": ["drift-charcoal", ""],
    "drift-graphite": ["drift-charcoal", "silver"],
    "drift-slate": ["drift-midnight", "teal"],
    "drift-aubergine": ["drift-plum", ""],
};

const hexColorPattern = /^#([\da-f]{2})([\da-f]{2})([\da-f]{2})$/i;

/** A saved theme name and accent as they are now: an earlier name becomes its base, and its accent unless one was chosen. */
export function migratedTheme(name: string, accent: string): [ThemeName, string] {
    const known = themes.find((base) => base === name);
    if (known) return [known, accent];

    const [base, implied] = renamed[name] ?? ["drift-charcoal", ""];
    return [base, accent || implied];
}

const [theme, setThemeValue] = persisted<ThemeName>("drift.theme", "drift-charcoal");
const [accent, setAccentValue] = persisted<string>("drift.theme.accent", "");
const [corners, setCornersValue] = persisted<CornerStyle>("drift.theme.corners", "");
const [uiFont, setUiFontValue] = persisted("drift.theme.uiFont", "");
const [codeFont, setCodeFontValue] = persisted("drift.theme.codeFont", "");
const [customTheme, setCustomThemeValue] = persisted<CustomTheme>("drift.theme.custom", {
    background: "#161616",
    surface: "#1e1e1e",
    text: "#ececec",
    accent: "#a78bfa",
});
export { theme, accent, corners, uiFont, codeFont, customTheme };

// A theme saved by an earlier version is renamed once, before anything reads it.
{
    const [base, chosen] = migratedTheme(theme(), accent());
    if (base !== theme()) {
        setThemeValue(base);
        setAccentValue(chosen);
    }
}

const customCssKey = "drift.theme.customCss";
const maxCustomCssChars = 20_000;
const truncateChars = (value: string, max: number) => [...value].slice(0, max).join("");
// Debouncing avoids storage writes and style replacement on every CSS keystroke.
const cssPersistDebounceMs = 200;
const cssApplyDebounceMs = 75;

let savedCustomCss = "";
try {
    const raw = localStorage.getItem(customCssKey);
    const parsed = raw ? JSON.parse(raw) : "";
    if (typeof parsed === "string") savedCustomCss = truncateChars(parsed, maxCustomCssChars);
} catch {}
const [customCss, setCustomCssValue] = createSignal(savedCustomCss);
export { customCss };
let cssPersistTimer: ReturnType<typeof setTimeout> | undefined;

export function setCustomCss(value: string) {
    const next = truncateChars(value, maxCustomCssChars);
    setCustomCssValue(next);
    publishTheme();
    clearTimeout(cssPersistTimer);
    cssPersistTimer = setTimeout(() => {
        try {
            localStorage.setItem(customCssKey, JSON.stringify(next));
        } catch {}
    }, cssPersistDebounceMs);
}

export function setCustomThemeColor(color: keyof CustomTheme, value: string) {
    setCustomTheme({ ...customTheme(), [color]: value });
}

export function setTheme(value: ThemeName) {
    setThemeValue(value);
    publishTheme();
}

/** A named accent, a `#rrggbb` colour, or empty for the base's own. */
export function setAccent(value: string) {
    setAccentValue(value);
    publishTheme();
}

export function setCorners(value: CornerStyle) {
    setCornersValue(value);
    publishTheme();
}

export function setUiFont(value: string) {
    setUiFontValue(truncateChars(value, 256));
    publishTheme();
}

export function setCodeFont(value: string) {
    setCodeFontValue(truncateChars(value, 256));
    publishTheme();
}

export function setCustomTheme(value: CustomTheme) {
    setCustomThemeValue(value);
    publishTheme();
}

/**
 * Applies a theme another window or device chose.
 * @public loaded by main.tsx through a dynamic import, which knip cannot follow
 */
export function applyMirroredTheme(value: MirrorTheme) {
    const [base, chosen] = migratedTheme(value.name, value.accent);
    setThemeValue(base);
    setAccentValue(chosen);
    setCornersValue(value.corners);
    setCustomThemeValue(value.custom);
    setUiFontValue(value.uiFont);
    setCodeFontValue(value.codeFont);
    setCustomCssValue(value.customCss);
}

function publishTheme() {
    publishMirrorTheme({
        name: theme(),
        accent: accent(),
        corners: corners(),
        custom: customTheme(),
        uiFont: uiFont(),
        codeFont: codeFont(),
        customCss: customCss(),
    });
}

// Theme brightness uses ITU-R BT.601 luma weights normalized to the range 0..1.
const lumaRed = 299;
const lumaGreen = 587;
const lumaBlue = 114;
const lumaScale = 255_000;
const lightBackgroundThreshold = 0.6;

/** How light a `#rrggbb` colour reads, 0 to 1; a malformed colour reads as dark. */
function brightness(color: string) {
    const match = color.match(hexColorPattern);
    if (!match) return 0;

    const [red, green, blue] = [match[1], match[2], match[3]].map((part) => Number.parseInt(part, 16));
    return (red * lumaRed + green * lumaGreen + blue * lumaBlue) / lumaScale;
}

export function lightTheme() {
    if (theme() === "drift-light" || theme() === "drift-paper") return true;
    if (theme() !== "drift-custom") return false;

    return brightness(customTheme().background) > lightBackgroundThreshold;
}

/** The attributes and properties an element needs to look like `base` with the chosen accent. */
export function themeAttributes(base: ThemeName, chosen: string) {
    const named = accents.find((name) => name === chosen);
    const own = hexColorPattern.test(chosen) ? chosen : undefined;
    const ink = own && brightness(own) > 0.55 ? "#111111" : "#ffffff";
    const style: Record<string, string> = own
        ? { "--accent-d": own, "--accent-l": own, "--accent-ink-d": ink, "--accent-ink-l": ink }
        : {};

    return { "data-theme-preview": base, "data-accent": named, style };
}

export function bindTheme() {
    const root = document.documentElement;

    createEffect(() => {
        root.dataset.theme = theme();
        root.dataset.tone = lightTheme() ? "light" : "dark";
    });
    createEffect(() => {
        const { "data-accent": named, style } = themeAttributes(theme(), accent());
        if (named) root.dataset.accent = named;
        else delete root.dataset.accent;
        for (const property of ["--accent-d", "--accent-l", "--accent-ink-d", "--accent-ink-l"])
            root.style.removeProperty(property);
        for (const [property, value] of Object.entries(style)) root.style.setProperty(property, value);
    });
    createEffect(() => {
        if (corners()) root.dataset.corners = corners();
        else delete root.dataset.corners;
    });
    createEffect(() => {
        root.style.setProperty(
            "--ui-font",
            uiFont().trim() || '"Segoe UI Variable Text", "Segoe UI", system-ui, sans-serif',
        );
        root.style.setProperty(
            "--code-font",
            codeFont().trim() || '"Cascadia Code", Consolas, ui-monospace, monospace',
        );
    });
    createEffect(() => {
        const colors = customTheme();
        root.style.setProperty("--custom-bg", colors.background);
        root.style.setProperty("--custom-surface", colors.surface);
        root.style.setProperty("--custom-ink", colors.text);
        root.style.setProperty("--custom-accent", colors.accent);
    });
    createEffect(() => {
        const value = customCss();
        const timer = setTimeout(() => {
            const style =
                document.querySelector<HTMLStyleElement>("#drift-custom-css") ?? document.createElement("style");
            style.id = "drift-custom-css";
            style.textContent = value;
            if (!style.isConnected) document.head.append(style);
        }, cssApplyDebounceMs);
        onCleanup(() => clearTimeout(timer));
    });
}
