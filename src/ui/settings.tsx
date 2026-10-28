import { createEffect, createMemo, createSignal, For, Match, onCleanup, onMount, Show, Switch } from "solid-js";
import { afterOverlayBack, bindOverlayBack, overlayBackPending, popOverlayBack } from "../state/overlay-back";
import { sectionLabels, sectionGroups, settingsSearchResults } from "./settings-search";
import { createMediaQuery, smallScreenQuery, touchInput } from "../state/screen";
import { AboutSection, preloadUpdateSupport } from "./settings-about";
import { activateModal, closeOnBackdropPointerDown } from "./modal";
import { NotificationsSection } from "./settings-notifications";
import { RemoteAccessSection } from "./settings-remote-access";
import { IconChevronLeft, IconSearch, IconX } from "./icons";
import { PermissionsSection } from "./settings-permissions";
import { parseNavigationHash } from "../state/navigation";
import { AppearanceSection } from "./settings-appearance";
import { ToolExecutionSection } from "./settings-tools";
import { ProvidersSection } from "./settings-providers";
import { KeybindsSection } from "./settings-shortcuts";
import { UsageLimitsSection } from "./settings-usage";
import { BrowserSection } from "./settings-browser";
import { StorageSection } from "./settings-storage";
import { PluginsSection } from "./settings-plugins";
import { PromptsSection } from "./settings-prompts";
import { GeneralSection } from "./settings-general";
import { focusBundle } from "../state/bundle-focus";
import { SkillsSection } from "./settings-skills";
import { VoiceSection } from "./settings-voice";
import { preloadJellyfish } from "./jellyfish";
import { SectionIcon } from "./settings-icons";
import { CodeSection } from "./settings-code";
import { McpManagement } from "./mcp/manager";
import { isRemoteRuntime } from "../runtime";
import { Portal } from "solid-js/web";
import { t } from "../state/i18n";
import {
    setSettingsDetail,
    setSettingsOpen,
    setSettingsSection,
    settingsDetail,
    settingsSection,
} from "../state/settings";

import type { ProviderAuthMethod } from "../engine/provider-auth";
import type { SettingsSearchItem } from "./settings-search";
import type { Section } from "../state/settings";

export type { ProviderAuthMethod };

/** Another page's link to a bundle: Plugins, on that bundle's screen. */
function showBundle(id: string) {
    focusBundle(id);
    setSettingsSection("Plugins");
}

/** The open dialog; `src/app.tsx` loads this module only once Settings first opens. */
export function SettingsDialog() {
    onMount(preloadUpdateSupport);
    onMount(() => {
        const sync = () => {
            if (overlayBackPending()) return;
            if (isRemoteRuntime() && parseNavigationHash(window.location.hash).overlay !== "settings")
                setSettingsOpen(false);
        };

        window.addEventListener("popstate", sync);
        onCleanup(() => window.removeEventListener("popstate", sync));
    });

    const close = () => {
        setSettingsOpen(false);
        afterOverlayBack(() => {
            if (isRemoteRuntime() && parseNavigationHash(window.location.hash).overlay === "settings") popOverlayBack();
        });
    };

    return (
        <Portal>
            <SettingsModal onClose={close} />
        </Portal>
    );
}

function SettingsModal(props: { onClose: () => void }) {
    let dialog!: HTMLDivElement;
    let searchInput!: HTMLInputElement;
    let content!: HTMLDivElement;
    const section = settingsSection;
    const [contentScrolled, setContentScrolled] = createSignal(false);
    const [query, setQuery] = createSignal("");
    const results = createMemo(() => settingsSearchResults(query()));
    const narrow = createMediaQuery(smallScreenQuery);
    const detail = createMemo(() => settingsDetail() || !!query().trim());
    const back = () => {
        setSettingsDetail(false);
        setQuery("");
        searchInput.blur();
    };
    createEffect(() => {
        if (narrow() && detail()) onCleanup(bindOverlayBack(back));
    });

    const selectSection = (next: Section) => {
        setSettingsSection(next);
        setSettingsDetail(true);
        setQuery("");
        if (touchInput()) searchInput.blur();
        content.scrollTop = 0;
    };
    onMount(() =>
        onCleanup(activateModal(dialog, () => (narrow() && detail() ? back() : props.onClose()), { history: false })),
    );

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
                class="settings-dialog settings-scale fade-up flex h-[calc((100vh-1rem)/var(--zoom-scale,1))] w-[calc((100vw-1rem)/var(--zoom-scale,1))] overflow-hidden rounded-xl border border-edge bg-overlay shadow-2xl shadow-black/40 sm:h-[min(56rem,calc((100vh-3rem)/var(--zoom-scale,1)))] sm:w-[min(76rem,calc((100vw-3rem)/var(--zoom-scale,1)))]"
                classList={{ "settings-detail": detail() }}
                onClick={(event) => event.stopPropagation()}
            >
                <nav
                    class="settings-nav flex w-13 shrink-0 flex-col overflow-y-auto settings-nav-panel border-r border-edge px-1.5 py-3 sm:w-48 sm:px-3"
                    aria-label={t("sidebar.settings")}
                >
                    <For each={sectionGroups}>
                        {(group) => (
                            <div class="mb-3 last:mb-0">
                                <div class="settings-nav-group hidden px-2 pb-1.5 text-[length:calc(0.68rem*var(--settings-scale,1))] font-medium text-ink-faint sm:block">
                                    {t(group.label)}
                                </div>
                                <div class="space-y-0.5">
                                    <For each={group.items}>
                                        {(name) => (
                                            <button
                                                aria-label={t(sectionLabels[name])}
                                                aria-current={section() === name ? "page" : undefined}
                                                class="flex w-full items-center justify-center gap-2.5 rounded-md px-2 py-1.5 text-left text-sm outline-none transition-colors focus-visible:bg-ink/5 sm:justify-start"
                                                classList={{
                                                    "bg-ink/8 text-ink": section() === name,
                                                    "text-ink-muted hover:bg-ink/5 hover:text-ink": section() !== name,
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
                                                    class="settings-nav-label hidden min-w-0 truncate sm:inline"
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
                <div class="settings-body flex min-w-0 flex-1 flex-col overflow-hidden">
                    <div
                        class="settings-header z-10 flex items-center justify-between px-5 py-3.5"
                        classList={{ "settings-header-scrolled": contentScrolled() }}
                    >
                        <button
                            class="settings-back hidden shrink-0 items-center justify-center rounded-md text-ink-muted hover:bg-raised"
                            aria-label={t("common.goBack")}
                            onClick={back}
                        >
                            <IconChevronLeft />
                        </button>
                        <span class="settings-heading hidden min-w-0 flex-1 truncate text-sm font-semibold text-ink sm:block">
                            <span class="settings-index-title hidden">{t("sidebar.settings")}</span>
                            <span class="settings-section-title">{t(sectionLabels[section()])}</span>
                        </span>
                        <div class="settings-search mr-2 flex min-w-0 flex-1 items-center gap-1.5 rounded-md border border-edge bg-raised/45 px-2 transition-colors focus-within:border-accent sm:max-w-56">
                            <IconSearch class="size-3.5 shrink-0 text-ink-faint" />
                            <input
                                ref={searchInput}
                                type="text"
                                inputMode="search"
                                autocomplete="off"
                                autofocus={!touchInput()}
                                class="min-w-0 flex-1 bg-transparent py-1.5 text-sm text-ink outline-none placeholder:text-ink-faint"
                                aria-label={t("drift.settings.search.placeholder")}
                                placeholder={t("drift.settings.search.placeholder")}
                                value={query()}
                                onInput={(event) => setQuery(event.currentTarget.value)}
                                onKeyDown={(event) => {
                                    if (event.key === "Enter" && results()[0] && !event.isComposing) {
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
                        ref={content}
                        class="settings-content min-h-0 flex-1 overflow-y-auto px-3 py-4 sm:px-4"
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
                                    <Match when={section() === "Browser"}>
                                        <BrowserSection />
                                    </Match>
                                    <Match when={section() === "Providers"}>
                                        <ProvidersSection />
                                    </Match>
                                    <Match when={section() === "Usage"}>
                                        <UsageLimitsSection />
                                    </Match>
                                    <Match when={section() === "MCP"}>
                                        <McpManagement embedded onBundle={showBundle} />
                                    </Match>
                                    <Match when={section() === "Skills"}>
                                        <SkillsSection onBundle={showBundle} />
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
                                    <span class="mt-0.5 block text-[length:calc(0.72rem*var(--settings-scale,1))] leading-relaxed text-ink-faint">
                                        {item.description}
                                    </span>
                                </Show>
                            </span>
                            <span class="shrink-0 text-[length:calc(0.68rem*var(--settings-scale,1))] text-ink-faint">
                                {item.sectionLabel}
                            </span>
                        </button>
                    )}
                </For>
            </div>
        </Show>
    );
}
