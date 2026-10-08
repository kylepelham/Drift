import { sections, sectionLabels, sectionGroups, settingsSearchResults } from "./settings-search";
import { createMemo, createSignal, For, Match, onCleanup, onMount, Show, Switch } from "solid-js";
import { parseNavigationHash, pushRemoteOverlay } from "../state/navigation";
import { AboutSection, preloadUpdateSupport } from "./settings-about";
import { activateModal, closeOnBackdropPointerDown } from "./modal";
import { NotificationsSection } from "./settings-notifications";
import { RemoteAccessSection } from "./settings-remote-access";
import { PermissionsSection } from "./settings-permissions";
import { AppearanceSection } from "./settings-appearance";
import { ToolExecutionSection } from "./settings-tools";
import { ProvidersSection } from "./settings-providers";
import { KeybindsSection } from "./settings-shortcuts";
import { UsageLimitsSection } from "./settings-usage";
import { StorageSection } from "./settings-storage";
import { PluginsSection } from "./settings-plugins";
import { PromptsSection } from "./settings-prompts";
import { GeneralSection } from "./settings-general";
import { SkillsSection } from "./settings-skills";
import { VoiceSection } from "./settings-voice";
import { preloadJellyfish } from "./jellyfish";
import { SectionIcon } from "./settings-icons";
import { CodeSection } from "./settings-code";
import { isRemoteRuntime } from "../runtime";
import { IconSearch, IconX } from "./icons";
import { Portal } from "solid-js/web";
import { McpManagement } from "./mcp";
import { t } from "../state/i18n";

import type { Section, SettingsSearchItem } from "./settings-search";
import type { ProviderAuthMethod } from "../engine/provider-auth";

export type { ProviderAuthMethod };

const [settingsOpen, setSettingsOpen] = createSignal(false);
const [settingsSection, setSettingsSection] = createSignal<Section>("General");

export function openSettings(section?: Section) {
    setSettingsSection(section && sections.includes(section) ? section : "General");
    if (!settingsOpen()) pushRemoteOverlay("settings");

    setSettingsOpen(true);
}

export function SettingsHost() {
    onMount(preloadUpdateSupport);
    onMount(() => {
        const sync = () => {
            if (isRemoteRuntime() && parseNavigationHash(window.location.hash).overlay !== "settings")
                setSettingsOpen(false);
        };

        window.addEventListener("popstate", sync);
        onCleanup(() => window.removeEventListener("popstate", sync));
    });

    const close = () => {
        setSettingsOpen(false);
        if (isRemoteRuntime() && parseNavigationHash(window.location.hash).overlay === "settings") history.back();
    };

    return (
        <Show when={settingsOpen()}>
            <Portal>
                <SettingsModal onClose={close} />
            </Portal>
        </Show>
    );
}

function SettingsModal(props: { onClose: () => void }) {
    let dialog!: HTMLDivElement;
    let searchInput!: HTMLInputElement;
    const section = settingsSection;
    const [contentScrolled, setContentScrolled] = createSignal(false);
    const [query, setQuery] = createSignal("");
    const results = createMemo(() => settingsSearchResults(query()));

    const selectSection = (next: Section) => {
        setSettingsSection(next);
        setQuery("");
    };
    onMount(() => onCleanup(activateModal(dialog, props.onClose)));

    return (
        <div
            data-modal-layer
            class="fixed inset-0 z-30 flex items-center justify-center bg-black/50"
            onPointerDown={(event) => closeOnBackdropPointerDown(event, props.onClose, dialog)}
        >
            <div
                ref={dialog}
                role="dialog"
                aria-modal="true"
                aria-label={t("sidebar.settings")}
                tabIndex={-1}
                class="fade-up flex h-[calc((100vh-1rem)/var(--zoom-scale,1))] w-[calc((100vw-1rem)/var(--zoom-scale,1))] overflow-hidden rounded-xl border border-edge bg-overlay shadow-2xl shadow-black/40 sm:h-[min(48rem,calc((100vh-3rem)/var(--zoom-scale,1)))] sm:w-[min(64rem,calc((100vw-3rem)/var(--zoom-scale,1)))]"
                onClick={(event) => event.stopPropagation()}
            >
                <nav class="flex w-13 shrink-0 flex-col overflow-y-auto border-r border-edge px-1.5 py-3 sm:w-44 sm:px-3">
                    <For each={sectionGroups}>
                        {(group) => (
                            <div class="mb-3 last:mb-0">
                                <div class="hidden px-2 pb-1.5 text-[0.68rem] font-medium text-ink-faint sm:block">
                                    {t(group.label)}
                                </div>
                                <div class="space-y-0.5">
                                    <For each={group.items}>
                                        {(name) => (
                                            <button
                                                aria-label={t(sectionLabels[name])}
                                                class="flex w-full items-center justify-center gap-2.5 rounded-md px-2 py-1.5 text-left text-sm outline-none transition-colors focus-visible:bg-raised/60 sm:justify-start"
                                                classList={{
                                                    "bg-raised text-ink": section() === name,
                                                    "text-ink-muted hover:bg-raised/60 hover:text-ink":
                                                        section() !== name,
                                                }}
                                                onClick={() => selectSection(name)}
                                                onPointerEnter={() =>
                                                    name === "About" && void preloadJellyfish()?.catch(() => undefined)
                                                }
                                                onFocus={() =>
                                                    name === "About" && void preloadJellyfish()?.catch(() => undefined)
                                                }
                                            >
                                                <SectionIcon section={name} />
                                                <span
                                                    class="hidden min-w-0 truncate sm:inline"
                                                    title={t(sectionLabels[name])}
                                                >
                                                    {t(sectionLabels[name])}
                                                </span>
                                            </button>
                                        )}
                                    </For>
                                </div>
                            </div>
                        )}
                    </For>
                </nav>
                <div class="flex min-w-0 flex-1 flex-col overflow-hidden">
                    <div
                        class="settings-header z-10 flex items-center justify-between px-5 py-3.5"
                        classList={{ "settings-header-scrolled": contentScrolled() }}
                    >
                        <span class="hidden min-w-0 flex-1 truncate text-sm font-semibold text-ink sm:block">
                            {t(sectionLabels[section()])}
                        </span>
                        <div class="mr-2 flex min-w-0 flex-1 items-center gap-1.5 rounded-md border border-edge bg-raised/45 px-2 transition-colors focus-within:border-accent sm:max-w-56">
                            <IconSearch class="size-3.5 shrink-0 text-ink-faint" />
                            <input
                                ref={searchInput}
                                type="text"
                                inputMode="search"
                                autocomplete="off"
                                autofocus
                                class="min-w-0 flex-1 bg-transparent py-1.5 text-sm text-ink outline-none placeholder:text-ink-faint"
                                aria-label={t("drift.settings.search.placeholder")}
                                placeholder={t("drift.settings.search.placeholder")}
                                value={query()}
                                onInput={(event) => setQuery(event.currentTarget.value)}
                                onKeyDown={(event) => {
                                    if (event.key === "Enter" && results()[0]) {
                                        event.preventDefault();
                                        selectSection(results()[0]!.section);
                                    }
                                }}
                            />
                            <Show when={query()}>
                                <button
                                    class="flex size-5 shrink-0 items-center justify-center rounded text-ink-faint transition-colors hover:text-ink"
                                    title={t("drift.search.clear")}
                                    onClick={() => {
                                        setQuery("");
                                        searchInput.focus();
                                    }}
                                >
                                    <IconX class="size-3" />
                                </button>
                            </Show>
                        </div>
                        <button
                            title={t("common.close")}
                            class="flex size-7 items-center justify-center rounded-md text-ink-faint transition-colors hover:bg-raised hover:text-ink"
                            onClick={() => props.onClose()}
                        >
                            <IconX />
                        </button>
                    </div>
                    <div
                        class="min-h-0 flex-1 overflow-y-auto px-3 py-4 sm:px-4"
                        onScroll={(event) => setContentScrolled(event.currentTarget.scrollTop > 1)}
                    >
                        <Show
                            when={query().trim()}
                            fallback={
                                <Switch>
                                    <Match when={section() === "General"}>
                                        <GeneralSection />
                                    </Match>
                                    <Match when={section() === "Appearance"}>
                                        <AppearanceSection />
                                    </Match>
                                    <Match when={section() === "Code"}>
                                        <CodeSection />
                                    </Match>
                                    <Match when={section() === "Notifications"}>
                                        <NotificationsSection />
                                    </Match>
                                    <Match when={section() === "Voice"}>
                                        <VoiceSection />
                                    </Match>
                                    <Match when={section() === "Tools"}>
                                        <ToolExecutionSection />
                                    </Match>
                                    <Match when={section() === "Providers"}>
                                        <ProvidersSection />
                                    </Match>
                                    <Match when={section() === "Usage"}>
                                        <UsageLimitsSection />
                                    </Match>
                                    <Match when={section() === "MCP"}>
                                        <McpManagement embedded />
                                    </Match>
                                    <Match when={section() === "Skills"}>
                                        <SkillsSection />
                                    </Match>
                                    <Match when={section() === "Plugins"}>
                                        <PluginsSection />
                                    </Match>
                                    <Match when={section() === "Shortcuts"}>
                                        <KeybindsSection />
                                    </Match>
                                    <Match when={section() === "Prompts"}>
                                        <PromptsSection />
                                    </Match>
                                    <Match when={section() === "Permissions"}>
                                        <PermissionsSection />
                                    </Match>
                                    <Match when={section() === "Storage"}>
                                        <StorageSection />
                                    </Match>
                                    <Match when={section() === "Remote Access"}>
                                        <RemoteAccessSection />
                                    </Match>
                                    <Match when={section() === "About"}>
                                        <AboutSection />
                                    </Match>
                                </Switch>
                            }
                        >
                            <SettingsSearchResults items={results()} onSelect={selectSection} />
                        </Show>
                    </div>
                </div>
            </div>
        </div>
    );
}

function SettingsSearchResults(props: { items: SettingsSearchItem[]; onSelect: (section: Section) => void }) {
    return (
        <Show
            when={props.items.length}
            fallback={
                <div class="px-2 py-8 text-center text-sm text-ink-faint">{t("drift.settings.search.empty")}</div>
            }
        >
            <div class="space-y-1">
                <For each={props.items}>
                    {(item) => (
                        <button
                            type="button"
                            class="flex w-full items-center gap-3 rounded-lg px-3 py-2.5 text-left transition-colors hover:bg-raised/60 focus-visible:bg-raised/60 focus-visible:outline-none"
                            onClick={() => props.onSelect(item.section)}
                        >
                            <SectionIcon section={item.section} />
                            <span class="min-w-0 flex-1">
                                <span class="block text-sm font-medium text-ink">{item.title}</span>
                                <Show when={item.description}>
                                    <span class="mt-0.5 block text-[0.72rem] leading-relaxed text-ink-faint">
                                        {item.description}
                                    </span>
                                </Show>
                            </span>
                            <span class="shrink-0 text-[0.68rem] text-ink-faint">{item.sectionLabel}</span>
                        </button>
                    )}
                </For>
            </div>
        </Show>
    );
}
