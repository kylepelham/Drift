import { createEffect } from "solid-js";
import { persisted } from "./persist";

/** The smallest and largest an area's text can be scaled, against its default. */
export const minTextScale = 0.8;
export const maxTextScale = 1.5;

const clamped = (value: unknown) =>
    typeof value === "number" && Number.isFinite(value) ? Math.min(maxTextScale, Math.max(minTextScale, value)) : 1;

/** The largest icons can be scaled: past it they press on the buttons around them. */
export const maxIconScale = 1.4;

const iconClamped = (value: unknown) =>
    typeof value === "number" && Number.isFinite(value) ? Math.min(maxIconScale, Math.max(minTextScale, value)) : 1;

/** How large each area's text is against its default; `interface` is everything not set apart in an area of its own. */
export const [interfaceTextScale, setInterfaceTextScale] = persisted<number>("drift.text.interface", 1, clamped);
export const [conversationScale, setConversationScale] = persisted<number>("drift.text.conversation", 1, clamped);
export const [sidebarScale, setSidebarScale] = persisted<number>("drift.text.sidebar", 1, clamped);
export const [composerScale, setComposerScale] = persisted<number>("drift.text.composer", 1, clamped);
export const [settingsScale, setSettingsScale] = persisted<number>("drift.text.settings", 1, clamped);
/** How large icons are against their default, everywhere. */
export const [iconScale, setIconScale] = persisted<number>("drift.icons.scale", 1, iconClamped);

/** Keeps each area's scale on the document, where the CSS for that area reads it. */
export function bindTextSizes() {
    const root = document.documentElement;
    const bind = (property: string, scale: () => number) =>
        createEffect(() => {
            if (scale() === 1) root.style.removeProperty(property);
            else root.style.setProperty(property, String(scale()));
        });

    bind("--conversation-scale", conversationScale);
    bind("--sidebar-scale", sidebarScale);
    bind("--composer-scale", composerScale);
    bind("--ui-text-scale", interfaceTextScale);
    bind("--settings-scale", settingsScale);
    bind("--icon-scale", iconScale);
}
