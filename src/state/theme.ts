import { createEffect, createSignal, onCleanup } from "solid-js";
import { publishMirrorTheme, type MirrorTheme } from "./mirror";
import { persisted } from "./persist";

export const themes = [
    "drift-dark",
    "drift-graphite",
    "drift-midnight",
    "drift-slate",
    "drift-forest",
    "drift-aubergine",
    "drift-light",
    "drift-paper",
    "drift-custom",
] as const;
export type ThemeName = (typeof themes)[number];
export type CustomTheme = { background: string; surface: string; text: string; accent: string };

const [theme, setThemeValue] = persisted<ThemeName>("drift.theme", "drift-dark");
const [uiFont, setUiFontValue] = persisted("drift.theme.uiFont", "");
const [codeFont, setCodeFontValue] = persisted("drift.theme.codeFont", "");
const [customTheme, setCustomThemeValue] = persisted<CustomTheme>("drift.theme.custom", {
    background: "#111318",
    surface: "#1b1e25",
    text: "#e8eaf0",
    accent: "#a78bfa",
});
export { theme, uiFont, codeFont, customTheme };

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
    setThemeValue(value.name);
    setCustomThemeValue(value.custom);
    setUiFontValue(value.uiFont);
    setCodeFontValue(value.codeFont);
    setCustomCssValue(value.customCss);
}

function publishTheme() {
    publishMirrorTheme({
        name: theme(),
        custom: customTheme(),
        uiFont: uiFont(),
        codeFont: codeFont(),
        customCss: customCss(),
    });
}

// Custom theme brightness uses ITU-R BT.601 luma weights normalized to the range 0..1.
const lumaRed = 299;
const lumaGreen = 587;
const lumaBlue = 114;
const lumaScale = 255_000;
const lightBackgroundThreshold = 0.6;
const hexColorPattern = /^#([\da-f]{2})([\da-f]{2})([\da-f]{2})$/i;

export function lightTheme() {
    if (theme() === "drift-light" || theme() === "drift-paper") return true;
    if (theme() !== "drift-custom") return false;
    const match = customTheme().background.match(hexColorPattern);
    if (!match) return false;
    const red = Number.parseInt(match[1], 16);
    const green = Number.parseInt(match[2], 16);
    const blue = Number.parseInt(match[3], 16);
    const brightness = (red * lumaRed + green * lumaGreen + blue * lumaBlue) / lumaScale;
    return brightness > lightBackgroundThreshold;
}

export function bindTheme() {
    createEffect(() => {
        document.documentElement.dataset.theme = theme();
    });
    createEffect(() => {
        document.documentElement.style.setProperty(
            "--ui-font",
            uiFont().trim() || '"Segoe UI Variable Text", "Segoe UI", system-ui, sans-serif',
        );
        document.documentElement.style.setProperty(
            "--code-font",
            codeFont().trim() || '"Cascadia Code", Consolas, ui-monospace, monospace',
        );
    });
    createEffect(() => {
        const colors = customTheme();
        document.documentElement.style.setProperty("--custom-bg", colors.background);
        document.documentElement.style.setProperty("--custom-surface", colors.surface);
        document.documentElement.style.setProperty("--custom-ink", colors.text);
        document.documentElement.style.setProperty("--custom-accent", colors.accent);
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
