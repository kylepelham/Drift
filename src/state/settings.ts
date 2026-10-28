import { pushRemoteOverlay } from "./navigation";
import { createSignal } from "solid-js";

export const sections = [
    "General",
    "Appearance",
    "Code",
    "Notifications",
    "Voice",
    "Shortcuts",
    "Tools",
    "Browser",
    "Providers",
    "Usage",
    "Skills",
    "MCP",
    "Plugins",
    "Prompts",
    "Permissions",
    "Storage",
    "Remote Access",
    "About",
] as const;

export type Section = (typeof sections)[number];

const [settingsOpen, setSettingsOpen] = createSignal(false);
const [settingsSection, setSettingsSection] = createSignal<Section>("General");
const [settingsDetail, setSettingsDetail] = createSignal(false);

export { settingsOpen, setSettingsOpen, settingsSection, setSettingsSection, settingsDetail, setSettingsDetail };

/** Opens Settings, on `section` when given; the dialog's code loads only then (`src/app.tsx`). */
export function openSettings(section?: Section) {
    setSettingsSection(section && sections.includes(section) ? section : "General");
    setSettingsDetail(!!section);
    if (!settingsOpen()) pushRemoteOverlay("settings");

    setSettingsOpen(true);
}
