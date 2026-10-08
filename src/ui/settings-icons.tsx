import {
    IconArchive,
    IconBell,
    IconChip,
    IconGauge,
    IconCode,
    IconInfo,
    IconKeyboard,
    IconMic,
    IconPalette,
    IconPlug,
    IconSparkles,
    IconShieldCheck,
    IconSliders,
} from "./icons";

import type { Section } from "./settings-search";

const icons = {
    General: IconSliders,
    Appearance: IconPalette,
    Code: IconCode,
    Notifications: IconBell,
    Voice: IconMic,
    Shortcuts: IconKeyboard,
    Tools: IconSliders,
    Providers: IconChip,
    Usage: IconGauge,
    MCP: IconShieldCheck,
    Skills: IconSparkles,
    Plugins: IconPlug,
    Prompts: IconCode,
    Permissions: IconShieldCheck,
    Storage: IconArchive,
    "Remote Access": IconShieldCheck,
    About: IconInfo,
};

export function SectionIcon(props: { section: Section }) {
    const icon = () => {
        const Component = icons[props.section] ?? IconInfo;

        return <Component />;
    };

    return <span class="flex size-5 shrink-0 items-center justify-center text-ink-faint">{icon()}</span>;
}
