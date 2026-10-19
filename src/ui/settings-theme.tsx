import { createSignal, For, Show } from "solid-js";
import { IconCheck } from "./icons";
import { t } from "../state/i18n";
import {
    accent,
    accents,
    customTheme,
    setAccent,
    setCustomTheme,
    setCustomThemeColor,
    setTheme,
    theme,
    themeAttributes,
    themes,
} from "../state/theme";

import type { CustomTheme, ThemeName } from "../state/theme";

/** Each base's name, for its card and for settings search. */
export const themeLabels: Record<ThemeName, string> = {
    "drift-charcoal": "drift.theme.charcoal",
    "drift-claude": "drift.theme.claude",
    "drift-black": "drift.theme.black",
    "drift-dim": "drift.theme.dim",
    "drift-mocha": "drift.theme.mocha",
    "drift-midnight": "drift.theme.midnight",
    "drift-forest": "drift.theme.forest",
    "drift-plum": "drift.theme.plum",
    "drift-light": "drift.theme.light",
    "drift-paper": "drift.theme.paper",
    "drift-custom": "drift.theme.custom",
};

const hexColor = /^#[\da-f]{6}$/i;

/** Every base as a small window drawn in its own colours and the chosen accent. */
export function ThemeGallery() {
    return (
        <div class="grid grid-cols-[repeat(auto-fill,minmax(9.5rem,1fr))] gap-3 py-3">
            <For each={themes}>{(name) => <ThemeCard name={name} />}</For>
        </div>
    );
}

function ThemeCard(props: { name: ThemeName }) {
    const active = () => theme() === props.name;
    const look = () => themeAttributes(props.name, accent());

    return (
        <button
            class="group text-left"
            aria-pressed={active()}
            aria-label={t(themeLabels[props.name])}
            onClick={() => setTheme(props.name)}
        >
            <div
                data-theme-preview={props.name}
                data-accent={look()["data-accent"]}
                style={look().style}
                class="relative flex h-20 overflow-hidden rounded-xl border bg-bg transition-[box-shadow,border-color]"
                classList={{
                    "border-accent ring-2 ring-accent/35": active(),
                    "border-edge group-hover:border-edge-strong": !active(),
                }}
            >
                <div class="flex w-[28%] flex-col gap-1 border-r border-edge bg-sidebar p-1.5">
                    <span class="h-1 w-3/4 rounded-full bg-ink-faint/60" />
                    <span class="h-1.5 w-full rounded-sm bg-raised" />
                    <span class="h-1 w-2/3 rounded-full bg-ink-faint/40" />
                    <span class="h-1 w-1/2 rounded-full bg-ink-faint/40" />
                </div>
                <div class="flex flex-1 flex-col gap-1 p-2">
                    <span class="ml-auto h-2.5 w-2/5 rounded-md bg-raised" />
                    <span class="h-1 w-5/6 rounded-full bg-ink-muted/70" />
                    <span class="h-1 w-3/5 rounded-full bg-ink-muted/50" />
                    <div class="mt-auto flex items-center gap-1 rounded-md border border-edge bg-surface px-1 py-0.5">
                        <span class="h-1 flex-1 rounded-full bg-ink-faint/40" />
                        <span class="size-2.5 rounded-sm bg-accent" />
                    </div>
                </div>
            </div>
            <div class="mt-1.5 flex items-center gap-1.5 px-0.5">
                <span class="truncate text-xs" classList={{ "text-ink": active(), "text-ink-muted": !active() }}>
                    {t(themeLabels[props.name])}
                </span>
                <Show when={active()}>
                    <IconCheck class="size-3.5 shrink-0 text-accent" />
                </Show>
            </div>
        </button>
    );
}

/** The theme's own accent, the named ones, and a colour of the user's own. */
export function AccentPicker() {
    const own = () => (hexColor.test(accent()) ? accent() : "");

    return (
        <div class="flex flex-wrap items-center gap-2" role="radiogroup" aria-label={t("drift.accent.title")}>
            <AutoAccent />
            <span class="mx-1 h-5 w-px bg-edge" aria-hidden="true" />
            <For each={accents}>{(name) => <AccentSwatch id={name} label={t(`drift.accent.${name}`)} />}</For>
            <label
                class="relative flex size-7 cursor-pointer items-center justify-center rounded-full transition-transform hover:scale-110"
                title={t("drift.accent.custom")}
            >
                <span
                    class="accent-dot size-5 rounded-full"
                    classList={{ "accent-dot-chosen": !!own() }}
                    style={{
                        background:
                            own() ||
                            "conic-gradient(#f2797c, #f3a25f, #e8c170, #7ccf8e, #5ec7c2, #7aa2f7, #a78bfa, #ec8fc6, #f2797c)",
                    }}
                />
                <input
                    type="color"
                    class="absolute inset-0 cursor-pointer opacity-0"
                    aria-label={t("drift.accent.custom")}
                    value={own() || "#a78bfa"}
                    onInput={(event) => setAccent(event.currentTarget.value)}
                />
            </label>
        </div>
    );
}

/** The base's own accent, as a labelled choice apart from the colours. */
function AutoAccent() {
    const active = () => accent() === "";

    return (
        <button
            role="radio"
            aria-checked={active()}
            title={t("drift.accent.description")}
            class="flex h-7 items-center gap-1.5 rounded-full border px-2.5 text-xs font-medium transition-colors"
            classList={{
                "border-ink text-ink": active(),
                "border-edge text-ink-muted hover:border-edge-strong hover:text-ink": !active(),
            }}
            onClick={() => setAccent("")}
        >
            <span data-theme-preview={theme()} class="size-3 rounded-full bg-accent" />
            {t("drift.accent.auto")}
        </button>
    );
}

function AccentSwatch(props: { id: string; label: string }) {
    const active = () => accent() === props.id;
    const look = () => themeAttributes(theme(), props.id);

    return (
        <button
            role="radio"
            aria-checked={active()}
            aria-label={props.label}
            title={props.label}
            class="flex size-7 items-center justify-center rounded-full transition-transform hover:scale-110"
            onClick={() => setAccent(props.id)}
        >
            <span
                data-theme-preview={theme()}
                data-accent={look()["data-accent"]}
                class="accent-dot size-5 rounded-full bg-accent"
                classList={{ "accent-dot-chosen": active() }}
            />
        </button>
    );
}

const customColors: { id: Exclude<keyof CustomTheme, "accent">; label: string }[] = [
    { id: "background", label: "drift.color.background" },
    { id: "surface", label: "drift.color.surface" },
    { id: "text", label: "drift.color.text" },
];

/** The custom base's three colours, and a base to copy them from. */
export function CustomColors() {
    const [copied, setCopied] = createSignal(false);

    /** Reads a built-in base's colours off a hidden element drawn in it. */
    const startFrom = (base: ThemeName) => {
        const probe = document.createElement("div");
        probe.dataset.themePreview = base;
        document.body.append(probe);
        const style = getComputedStyle(probe);
        const read = (property: string) => toHex(style.getPropertyValue(property).trim());
        setCustomTheme({
            ...customTheme(),
            background: read("--bg"),
            surface: read("--surface"),
            text: read("--ink"),
        });
        probe.remove();
        setCopied(true);
    };

    return (
        <div class="space-y-3 py-3">
            <div class="grid grid-cols-3 gap-2">
                <For each={customColors}>
                    {(color) => (
                        <label class="flex items-center gap-2 rounded-lg border border-edge bg-bg/40 p-2">
                            <input
                                type="color"
                                class="size-8 shrink-0 cursor-pointer rounded-md border border-edge bg-transparent p-0.5"
                                aria-label={t(color.label)}
                                value={customTheme()[color.id]}
                                onInput={(event) => setCustomThemeColor(color.id, event.currentTarget.value)}
                            />
                            <span class="min-w-0">
                                <span class="block text-xs text-ink">{t(color.label)}</span>
                                <span class="block font-mono text-[0.68rem] text-ink-faint uppercase">
                                    {customTheme()[color.id]}
                                </span>
                            </span>
                        </label>
                    )}
                </For>
            </div>
            <div class="flex flex-wrap items-center gap-1.5 text-xs text-ink-muted">
                <span class="mr-1">
                    {copied() ? t("drift.settings.customPalette.copied") : t("drift.settings.customPalette.startFrom")}
                </span>
                <For each={themes.filter((name) => name !== "drift-custom")}>
                    {(base) => (
                        <button
                            class="rounded-md border border-edge px-2 py-0.5 transition-colors hover:border-edge-strong hover:text-ink"
                            onClick={() => startFrom(base)}
                        >
                            {t(themeLabels[base])}
                        </button>
                    )}
                </For>
            </div>
        </div>
    );
}

/** A computed colour (`rgb(…)` or `#…`) as `#rrggbb`. */
function toHex(color: string) {
    if (hexColor.test(color)) return color.toLowerCase();

    const channels = color
        .match(/\d+(\.\d+)?/g)
        ?.slice(0, 3)
        .map(Number) ?? [0, 0, 0];
    return `#${channels.map((channel) => Math.round(channel).toString(16).padStart(2, "0")).join("")}`;
}
