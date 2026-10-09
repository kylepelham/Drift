import { keybindLabels } from "./settings-shortcuts";
import { themeMeta } from "./settings-appearance";
import { t } from "../state/i18n";

export const sections = [
    "General",
    "Appearance",
    "Code",
    "Notifications",
    "Voice",
    "Shortcuts",
    "Tools",
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
type SettingsSearchDefinition = { title: string; description?: string };
export type SettingsSearchItem = { section: Section; sectionLabel: string; title: string; description: string };

export const sectionLabels: Record<Section, string> = {
    General: "settings.tab.general",
    Appearance: "settings.general.section.appearance",
    Code: "drift.settings.code",
    Notifications: "drift.settings.notifications",
    Voice: "drift.voice",
    Shortcuts: "settings.tab.shortcuts",
    Tools: "drift.settings.execution",
    Providers: "settings.providers.title",
    Usage: "drift.usage.title",
    Skills: "drift.settings.skills",
    MCP: "dialog.mcp.title",
    Plugins: "drift.settings.plugins",
    Prompts: "drift.settings.prompts",
    Permissions: "drift.settings.permissions",
    Storage: "drift.storage",
    "Remote Access": "drift.remote.title",
    About: "drift.settings.about",
};
export const sectionGroups: { label: string; items: Section[] }[] = [
    {
        label: "settings.section.desktop",
        items: ["General", "Appearance", "Code", "Notifications", "Voice", "Shortcuts"],
    },
    {
        label: "settings.section.server",
        items: ["Tools", "Providers", "Usage", "Skills", "MCP", "Plugins", "Prompts", "Permissions"],
    },
    { label: "drift.settings.section", items: ["Storage", "Remote Access", "About"] },
];

const settingsSearchDefinitions = {
    General: [
        { title: "settings.general.row.language.title", description: "settings.general.row.language.description" },
        {
            title: "drift.settings.responseAnimation.title",
            description: "drift.settings.responseAnimation.description",
        },
        { title: "drift.settings.dayDividers.title", description: "drift.settings.dayDividers.description" },
        {
            title: "drift.settings.responseAnimation.speed.title",
            description: "drift.settings.responseAnimation.speed.description",
        },
        { title: "drift.preview.settings.title", description: "drift.preview.settings.description" },
        { title: "command.permissions.autoaccept.enable", description: "toast.permissions.autoaccept.on.description" },
        {
            title: "settings.general.row.reasoningSummaries.title",
            description: "settings.general.row.reasoningSummaries.description",
        },
        { title: "drift.settings.toolErrors.title", description: "drift.settings.toolErrors.description" },
        { title: "drift.settings.autoCompact.title", description: "drift.settings.autoCompact.description" },
        {
            title: "drift.settings.summaries.collapsible.title",
            description: "drift.settings.summaries.collapsible.description",
        },
        {
            title: "drift.settings.summaries.collapsed.title",
            description: "drift.settings.summaries.collapsed.description",
        },
        { title: "settings.updates.row.startup.title", description: "settings.updates.row.startup.description" },
    ],
    Appearance: [
        { title: "settings.general.row.theme.title" },
        ...Object.values(themeMeta).map((item) => ({ title: item.label })),
        { title: "drift.settings.customPalette" },
        { title: "settings.general.row.uiFont.title", description: "settings.general.row.uiFont.description" },
        { title: "startup.settings.show.title", description: "startup.settings.show.description" },
        { title: "startup.settings.mascot.title", description: "startup.settings.mascot.description" },
        { title: "startup.settings.exit.title", description: "startup.settings.exit.description" },
        { title: "startup.settings.duration.title", description: "startup.settings.duration.description" },
        { title: "startup.settings.font.title", description: "startup.settings.font.description" },
        { title: "drift.settings.customCss", description: "drift.settings.customCss.description" },
    ],
    Code: [
        { title: "drift.code.syntaxTheme.title", description: "drift.code.syntaxTheme.description" },
        { title: "settings.general.row.font.title", description: "settings.general.row.font.description" },
        { title: "drift.code.fontSize.title", description: "drift.code.fontSize.description" },
        { title: "drift.code.tabWidth.title", description: "drift.code.tabWidth.description" },
        { title: "drift.code.wordWrap.title", description: "drift.code.wordWrap.description" },
        { title: "drift.code.diffWordWrap.title", description: "drift.code.diffWordWrap.description" },
        { title: "drift.code.lineNumbers.title", description: "drift.code.lineNumbers.description" },
        { title: "drift.code.diffIndicator.title", description: "drift.code.diffIndicator.description" },
    ],
    Notifications: [
        ...["agent", "permissions", "errors"].flatMap((kind) => [
            {
                title: `settings.general.notifications.${kind}.title`,
                description: `settings.general.notifications.${kind}.description`,
            },
            {
                title: `settings.general.sounds.${kind}.title`,
                description: `settings.general.sounds.${kind}.description`,
            },
        ]),
        { title: "drift.settings.sound.chooseCustom" },
    ],
    Voice: [
        { title: "drift.voice.dictation.enabled.title", description: "drift.voice.dictation.enabled.description" },
        { title: "drift.voice.input.title", description: "drift.voice.input.description" },
        { title: "drift.voice.model.title", description: "drift.voice.model.description" },
        { title: "drift.voice.model.storage.title", description: "drift.voice.model.storage.ready" },
        { title: "drift.voice.acceleration.title", description: "drift.voice.acceleration.gpu" },
        { title: "drift.voice.dictation.language.title", description: "drift.voice.dictation.language.description" },
        { title: "drift.voice.dictation.keyterms.title", description: "drift.voice.dictation.keyterms.description" },
    ],
    Shortcuts: Object.values(keybindLabels).map((title) => ({ title })),
    Tools: [
        { title: "drift.settings.shellTimeout.title", description: "drift.settings.shellTimeout.description" },
        { title: "drift.settings.backgroundLimit.title", description: "drift.settings.backgroundLimit.description" },
        {
            title: "drift.settings.shellTimeout.customMinutes",
            description: "drift.settings.shellTimeout.customDescription",
        },
    ],
    Providers: [
        { title: "dialog.provider.search.placeholder" },
        { title: "settings.providers.section.connected" },
        { title: "provider.connect.method.apiKey" },
        { title: "provider.connect.oauth.code.placeholder" },
        { title: "drift.lmStudio.apiToken", description: "drift.lmStudio.description" },
        { title: "drift.lmStudio.refresh" },
    ],
    Usage: [
        { title: "drift.usage.title", description: "drift.usage.settingsDescription" },
        { title: "drift.usage.session" },
        { title: "drift.usage.weekly" },
    ],
    MCP: [
        { title: "drift.mcp.servers" },
        { title: "drift.mcp.registry" },
        { title: "drift.mcp.add" },
        { title: "drift.mcp.name" },
        { title: "drift.mcp.form.command" },
        { title: "drift.mcp.form.environment" },
        { title: "drift.mcp.form.url" },
        { title: "drift.mcp.form.headers" },
        { title: "drift.registry.sources", description: "drift.registry.sources.mcpDescription" },
    ],
    Prompts: [
        { title: "drift.settings.prompts.group.base", description: "drift.settings.prompts.familyDescription" },
        { title: "drift.settings.prompts.systemPrompt", description: "drift.settings.prompts.allDescription" },
        { title: "settings.agents.title", description: "drift.settings.prompts.agentsDescription" },
        { title: "drift.settings.prompts.group.subagents" },
        { title: "command.category.model" },
        { title: "drift.settings.prompts.agentPrompt", description: "drift.settings.prompts.inheritsFamily" },
        { title: "drift.settings.prompts.variant" },
        { title: "drift.settings.prompts.steps" },
        { title: "drift.settings.prompts.tools", description: "drift.settings.prompts.toolsDescription" },
        { title: "drift.settings.permissions", description: "drift.settings.prompts.permissionsDescription" },
    ],
    Skills: [
        { title: "drift.settings.skills", description: "drift.skills.empty" },
        { title: "drift.plugins.tab.registry", description: "drift.skills.registrySource" },
        { title: "drift.registry.sources", description: "drift.registry.sources.pluginsDescription" },
    ],
    Plugins: [
        { title: "drift.settings.plugins", description: "drift.plugins.empty" },
        { title: "drift.plugins.tab.registry", description: "drift.plugins.registrySource" },
        { title: "drift.registry.sources", description: "drift.registry.sources.pluginsDescription" },
        { title: "drift.plugins.reload" },
    ],
    Permissions: [
        { title: "drift.permissions.rules", description: "drift.permissions.rulesDescription" },
        { title: "drift.permissions.grants" },
    ],
    Storage: [
        { title: "drift.storage.sessions.total", description: "drift.storage.sessions.total.description" },
        { title: "drift.storage.sessions.subagent", description: "drift.storage.sessions.subagent.description" },
        { title: "drift.storage.sessions.archived", description: "drift.storage.sessions.archived.description" },
        { title: "drift.storage.auto", description: "drift.storage.auto.description" },
        { title: "drift.storage.rule.superseded", description: "drift.storage.rule.superseded.description" },
        { title: "drift.storage.rule.subagent", description: "drift.storage.rule.subagent.description" },
        { title: "drift.storage.rule.archived", description: "drift.storage.rule.archived.description" },
        { title: "drift.storage.rule.orphan", description: "drift.storage.rule.orphan.description" },
        { title: "drift.storage.analyze", description: "drift.storage.analyze.description" },
        { title: "drift.storage.prune", description: "drift.storage.prune.description" },
        { title: "drift.storage.compact", description: "drift.storage.compact.description" },
    ],
    "Remote Access": [
        { title: "drift.remote.enable", description: "drift.remote.enableDescription" },
        { title: "drift.remote.connect.title", description: "drift.remote.connect.open" },
        { title: "drift.remote.devices.title", description: "drift.remote.devices.revokeAll" },
        { title: "drift.remote.password.title", description: "drift.remote.password.description" },
        { title: "drift.remote.certificate.title", description: "drift.remote.certificate.description" },
    ],
    About: [
        { title: "drift.about.row.app.title", description: "drift.about.row.app.description" },
        { title: "drift.about.row.native.title", description: "drift.about.row.native.description" },
        { title: "drift.about.row.updates.title", description: "drift.about.row.updates.installed" },
        { title: "drift.about.row.website.title", description: "drift.about.row.website.description" },
    ],
} satisfies Record<Section, SettingsSearchDefinition[]>;

const normalizeSettingsSearch = (value: string) => value.trim().toLocaleLowerCase();

export function settingsSearchResults(query: string): SettingsSearchItem[] {
    const value = normalizeSettingsSearch(query);
    if (!value) return [];

    const terms = value.split(/\s+/);
    const items = sections.flatMap((section) => {
        const sectionLabel = t(sectionLabels[section]);
        const definitions: readonly SettingsSearchDefinition[] = settingsSearchDefinitions[section];

        return definitions.map((definition) => ({
            section,
            sectionLabel,
            title: t(definition.title),
            description: definition.description ? t(definition.description) : "",
        }));
    });

    return items
        .filter((item) => {
            const text = normalizeSettingsSearch(
                `${item.section} ${item.sectionLabel} ${item.title} ${item.description}`,
            );

            return terms.every((term) => text.includes(term));
        })
        .sort((left, right) => {
            const leftTitle = normalizeSettingsSearch(left.title);
            const rightTitle = normalizeSettingsSearch(right.title);

            return settingsTitleRank(leftTitle, value) - settingsTitleRank(rightTitle, value);
        })
        .slice(0, 40);
}

function settingsTitleRank(title: string, query: string) {
    if (title === query) return 0;
    if (title.startsWith(query)) return 1;
    if (title.includes(query)) return 2;

    return 3;
}
