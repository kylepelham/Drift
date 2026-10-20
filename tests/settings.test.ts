import { invariantKeys } from "../scripts/i18n/invariants";
import { expect, test } from "bun:test";
import { code } from "./source";

const settingsStorage = new Map<string, string>();
if (!("localStorage" in globalThis))
    Object.defineProperty(globalThis, "localStorage", {
        value: {
            getItem: (key: string) => settingsStorage.get(key) ?? null,
            setItem: (key: string, value: string) => settingsStorage.set(key, value),
        },
    });

test("no source guards on typeof import.meta.glob, which a Vite build leaves false so the assets never load", async () => {
    const { Glob } = await import("bun");
    for await (const file of new Glob("src/**/*.{ts,tsx}").scan(".")) {
        const text = await Bun.file(file).text();
        expect(code(text).includes("typeof import.meta.glob"), file).toBe(false);
    }
});

test("settings expose the OpenCode language and sound catalogs", async () => {
    const { languages } = await import("../src/state/language");
    const { soundOptions } = await import("../src/ui/sounds");
    expect(languages.map((item) => item.id)).toEqual([
        "en",
        "zh",
        "zht",
        "ko",
        "de",
        "es",
        "fr",
        "da",
        "ja",
        "pl",
        "ru",
        "uk",
        "bs",
        "ar",
        "no",
        "br",
        "th",
        "tr",
    ]);
    expect(soundOptions).toHaveLength(45);
    expect(new Set(soundOptions.map((item) => item.id)).size).toBe(45);
});

test("LM Studio readiness uses the loaded context required by the coding agent", async () => {
    const { formatModelContext, lmStudioMinimumContext, lmStudioModelReady } = await import("../src/state/lm-studio");
    const model = {
        limit: { context: 4096 },
    };
    expect(lmStudioMinimumContext).toBe(32768);
    expect(formatModelContext(4096)).toBe("4K");
    expect(formatModelContext(32768)).toBe("32K");
    expect(lmStudioModelReady(model as never)).toBe(false);
    expect(lmStudioModelReady({ ...model, limit: { context: 32768 } } as never)).toBe(true);
    expect(lmStudioModelReady({ ...model, limit: undefined } as never)).toBe(false);
});

test("selected language dictionaries translate settings without loading every locale", async () => {
    const { loadDictionary, reasoningLevelLabel, t } = await import("../src/state/i18n");
    await loadDictionary("es");
    expect(t("settings.tab.general")).toBe("General");
    expect(t("settings.general.row.language.title")).toBe("Idioma");
    expect(t("common.reset")).toBe("Restablecer");
    expect(t("drift.remote.title")).toBe("Acceso remoto");
    expect(t("drift.settings.prompts")).toBe("Prompts");
    expect(t("drift.slash.spawn.required")).toBe("Indica qué debe hacer el nuevo hilo después de /spawn.");
    expect(t("drift.attachment.kind.pdf")).toBe("PDF");
    expect(reasoningLevelLabel("xhigh")).toBe("Muy alto");
    expect(reasoningLevelLabel("custom")).toBe("Custom");
    await loadDictionary("en");
    expect(t("settings.general.row.language.title")).toBe("Language");
    expect(t("common.reset")).toBe("Reset");
});

test("base prompts and agents are one Server setting, with inherited values styled apart and save only for changes", async () => {
    const source = await Bun.file("src/ui/settings.tsx").text();
    const navigation = await Bun.file("src/ui/settings-search.ts").text();
    const editor = await Bun.file("src/ui/settings-prompts.tsx").text();
    expect(navigation).toContainCode(
        'items: ["Tools", "Browser", "Providers", "Usage", "Skills", "MCP", "Plugins", "Prompts", "Permissions"]',
    );
    expect(source).toContainCode("<PromptsSection />");
    expect(await Bun.file("src/ui/settings-base-prompts.tsx").exists()).toBeFalse();
    expect(editor).toContainCode('"text-ink-faint": !props.customized && !changed()');
    expect(editor).toContainCode("disabled={props.saving || !props.dirty}");
});

test("model-family base prompts are edited and reset in the engine, never through the shell's family overrides", async () => {
    const settings = await Bun.file("src/ui/settings.tsx").text();
    const editor = await Bun.file("src/ui/settings-prompts.tsx").text();
    expect(editor).toContainCode("const content = baseDraft(id)");
    expect(editor).toContainCode("engine.actions.saveBasePrompt(id, content)");
    expect(editor).toContainCode("engine.actions.resetBasePrompt(id)");
    expect(editor).not.toContainCode("readOnly");
    expect(settings).not.toContainCode("`family:");
    expect(settings).not.toContainCode("familyUnavailable");
});

test("settings search covers every category and finds feature descriptions", async () => {
    const { loadDictionary } = await import("../src/state/i18n");
    const { settingsSearchResults } = await import("../src/ui/settings-search");
    await loadDictionary("en");

    const categories = [
        "General",
        "Appearance",
        "Code",
        "Notifications",
        "Voice",
        "Shortcuts",
        "Tools",
        "Providers",
        "Skills",
        "MCP",
        "Plugins",
        "Prompts",
        "Storage",
        "Remote Access",
        "About",
    ] as const;
    for (const category of categories) {
        expect(
            settingsSearchResults(category).some((item) => item.section === category),
            category,
        ).toBeTrue();
    }

    expect(settingsSearchResults("shell commands child processes")[0]?.section).toBe("Tools");
    expect(settingsSearchResults("compact database")[0]?.section).toBe("Storage");
    expect(settingsSearchResults("engine version")[0]?.section).toBe("About");
});

test("Settings offers no Jev tool routing: the native engine has none, so a toggle would configure nothing", async () => {
    const source = await Bun.file("src/ui/settings.tsx").text();
    expect(source).not.toContainCode("ToolRoutingSetting");
    expect(source).not.toContainCode("drift.settings.toolRouting");
    expect(source).not.toContainCode('t("drift.settings.shellTimeout.scope")');
    expect(await Bun.file("src/ui/settings-tool-routing.tsx").exists()).toBeFalse();
    expect(await Bun.file("src/state/tool-routing.ts").exists()).toBeFalse();
});

test("agent overrides retain only values changed from upstream", async () => {
    const { agentOverrideValue } = await import("../src/state/prompts");
    const inherited = { prompt: "Upstream", mode: "primary", tools: { bash: true, read: true } };
    expect(agentOverrideValue({ ...inherited, tools: { read: true, bash: true } }, inherited)).toEqual({});
    expect(agentOverrideValue({ ...inherited, mode: "subagent" }, inherited)).toEqual({ mode: "subagent" });
    expect(agentOverrideValue({ ...inherited, prompt: "Custom" }, inherited)).toEqual({ prompt: "Custom" });
    expect(
        agentOverrideValue(
            { prompt: "Custom", mode: "subagent" },
            { prompt: "Custom", mode: "primary" },
            { prompt: "Custom" },
        ),
    ).toEqual({ prompt: "Custom", mode: "subagent" });
});

test("agent overrides saved or reset reach the engine for desktop and companion callers", async () => {
    const prompts = await Bun.file("src-tauri/src/prompts.rs").text();
    const remote = await Bun.file("src-tauri/src/remote.rs").text();
    for (const command of ["prompt_save", "prompt_reset"]) {
        const body = prompts.slice(prompts.indexOf(`pub(crate) fn ${command}(`)).split("\n}")[0]!;
        expect(body).toContain("app: AppHandle");
        expect(body).toContainCode(
            "crate::native::push_agent_overrides(&app, &store).map_err(|error| error.to_string())",
        );
        expect(remote).toContain(`prompts::${command}(`);
    }
    const ui = await Bun.file("src/ui/settings-prompts.tsx").text();
    expect(ui).toContainCode("await write(); await engine.actions.refreshAgents()");
    expect(ui).toContain('t("drift.settings.prompts.saved")');
    expect(ui).not.toContain("showRestartNotice");
});

type Catalog = { dict: Record<string, string>; drift: Record<string, string> };

test("no locale keeps a key English does not have", async () => {
    const { languages } = await import("../src/state/language");
    const en: Catalog = await import("../src/i18n/en");
    const english = new Set([...Object.keys(en.dict), ...Object.keys(en.drift)]);
    for (const language of languages.filter((language) => language.id !== "en")) {
        const catalog: Catalog = await import(`../src/i18n/${language.id}.ts`);
        const extra = [...Object.keys(catalog.dict), ...Object.keys(catalog.drift)].filter((key) => !english.has(key));
        expect(extra, `${language.id} has keys en.ts dropped`).toEqual([]);
    }
});

test("every locale translates every key except the deliberate invariants", async () => {
    const { languages } = await import("../src/state/language");
    const en: Catalog = await import("../src/i18n/en");
    const english = { ...en.dict, ...en.drift } as Record<string, string>;
    const keys = Object.keys(english)
        .filter((key) => !invariantKeys.has(key))
        .sort();
    const nonEnglish = languages.filter((language) => language.id !== "en");
    const localized = await Promise.all(
        nonEnglish.map(async (language) => {
            const catalog: Catalog = await import(`../src/i18n/${language.id}.ts`);
            return { ...catalog.dict, ...catalog.drift } as Record<string, string>;
        }),
    );

    nonEnglish.forEach((language, index) =>
        expect(Object.keys(localized[index]).sort(), `${language.id} keys`).toEqual(keys),
    );

    const unknownInvariant = [...invariantKeys].filter((key) => !(key in english));
    const copiedEnglish = keys.filter(
        (key) => key.startsWith("drift.") && localized.every((catalog) => catalog[key] === english[key]),
    );
    expect(unknownInvariant, "invariantKeys names keys that do not exist in en.ts").toEqual([]);
    expect(copiedEnglish, "English copies must be explicit invariants").toEqual([]);

    for (const language of nonEnglish) {
        const source = await Bun.file(`src/i18n/${language.id}.ts`).text();
        expect(source).not.toMatch(/^import\s/m);
        expect(source).not.toMatch(/\.\.\.[A-Za-z_$]/);
    }
    expect(await Bun.file("src/state/i18n.ts").text()).not.toContain("engine/upstream");
});

test("General settings expose preview modes and custom-only per-type toggles", async () => {
    const general = await Bun.file("src/ui/settings-general.tsx").text();
    expect(general).toContainCode('title={t("drift.preview.settings.title")}');
    expect(general).toContainCode('(["all", "none", "custom"] as const)');
    expect(general).toContainCode("value={filePreviewPrefs().mode}");
    expect(general).toContainCode('<Show when={filePreviewPrefs().mode === "custom"}>');
    expect(general).toContainCode("<For each={filePreviewTypes}>");
    expect(general).toContainCode("checked={filePreviewPrefs().types[type]}");
    expect(general).toContainCode("setFilePreviewType(type, !filePreviewPrefs().types[type])");
});

test("appearance offers bases with a custom one, light or dark by its background", async () => {
    const { setCustomTheme, setTheme, lightTheme, themes } = await import("../src/state/theme");
    expect(themes[0]).toBe("drift-charcoal");
    expect(themes).toContain("drift-claude");
    expect(themes).toContain("drift-mocha");
    expect(themes.at(-1)).toBe("drift-custom");
    setTheme("drift-paper");
    expect(lightTheme()).toBeTrue();
    setCustomTheme({ background: "#ffffff", surface: "#f5f5f5", text: "#111111", accent: "#3366cc" });
    setTheme("drift-custom");
    expect(lightTheme()).toBeTrue();
    setTheme("drift-charcoal");
    expect(lightTheme()).toBeFalse();
});

test("settings elevation and toggle contrast follow their visual state", async () => {
    const [settings, toggles, styles] = await Promise.all([
        Bun.file("src/ui/settings.tsx").text(),
        Bun.file("src/ui/controls.tsx").text(),
        Bun.file("src/styles/app.css").text(),
    ]);
    expect(settings).toContainCode('"settings-header-scrolled": contentScrolled()');
    expect(settings).toContainCode("setContentScrolled(event.currentTarget.scrollTop > 1)");
    expect(settings).toContainCode('class="settings-body flex min-w-0 flex-1 flex-col overflow-hidden"');
    expect(styles).toContain(".settings-header-scrolled::after");
    expect(styles).not.toContain(".settings-header::after");
    expect(toggles).toContainCode('"bg-ink-muted": !props.checked');
    expect(toggles).toContainCode('"translate-x-3 bg-accent-ink": props.checked');
});

test("appearance exposes persisted startup splash controls", async () => {
    const { splashDuration, splashDurations, splashExitAnimation, splashExitAnimations, splashMascotAnimations } =
        await import("../src/state/startup");
    const settings = await Bun.file("src/ui/settings-appearance.tsx").text();
    expect(splashMascotAnimations).toEqual(["bounce", "float", "pulse", "still"]);
    expect(splashExitAnimations).toEqual(["wave", "fade", "lift"]);
    expect(splashDurations).toEqual([1500, 3200, 5000]);
    expect(splashExitAnimation()).toBe("fade");
    expect(splashDuration()).toBe(3200);
    expect(settings).toContainCode('title={t("startup.settings.title")}');
    expect(settings).toContainCode("setSplashEnabled(!splashEnabled())");
    expect(settings).toContainCode("value={splashFont()}");
});

test("code display defaults preserve source and diff structure", async () => {
    const {
        codeFontSize,
        codeTabWidth,
        codeWordWrap,
        diffIndicator,
        diffLineNumbers,
        diffWordWrap,
        syntaxThemePreset,
        syntaxThemePresets,
    } = await import("../src/state/code");
    expect(syntaxThemePresets).toEqual(["automatic", "github", "vitesse", "one", "dracula", "nord"]);
    expect(syntaxThemePreset()).toBe("automatic");
    expect(codeFontSize()).toBe(13);
    expect(codeTabWidth()).toBe(4);
    expect(codeWordWrap()).toBeFalse();
    expect(diffWordWrap()).toBeFalse();
    expect(diffLineNumbers()).toBeTrue();
    expect(diffIndicator()).toBe("background");
    const { codeSettingOptions } = await import("../src/ui/settings-code");
    expect(codeSettingOptions()).toEqual({
        themes: ["automatic", "github", "vitesse", "one", "dracula", "nord"],
        fontSizes: [11, 12, 13, 14, 15, 16],
        tabWidths: [2, 4, 8],
        indicators: ["symbols", "bars", "background"],
    });
    const { codePreferenceBinding } = await import("../src/state/code");
    expect(codePreferenceBinding(13, 4, false, "automatic")).toEqual({
        size: "13px",
        tabSize: "4",
        wrap: "scroll",
        theme: "automatic",
    });
    expect(codePreferenceBinding(16, 8, true, "dracula").wrap).toBe("wrap");
});

test("notification defaults stay explicit and old webview auto-accept is forgotten only once the engine takes it", async () => {
    const { handOverAutoAccept, notificationDefaults, soundDefaults } = await import("../src/state/prefs");
    expect(notificationDefaults(true)).toEqual({ agent: true, permission: true, error: true });
    expect(soundDefaults()).toEqual({ agent: "none", permission: "none", error: "none" });
    const kept = new Map([
        ["drift.autoAccept.global", "true"],
        ["drift.autoAccept", JSON.stringify(["s1", 7, "s2"])],
    ]);
    const storage = localStorage as Storage;
    const saved = { getItem: storage.getItem, setItem: storage.setItem, removeItem: storage.removeItem };
    Object.assign(storage, {
        getItem: (key: string) => kept.get(key) ?? null,
        setItem: (key: string, value: string) => kept.set(key, value),
        removeItem: (key: string) => kept.delete(key),
    });
    try {
        await expect(handOverAutoAccept(async () => Promise.reject(new Error("engine down")))).rejects.toThrow(
            "engine down",
        );
        expect(kept.size, "a failed hand-over forgets nothing").toBe(2);
        const seen: unknown[] = [];
        await handOverAutoAccept(async (offered) => (seen.push(offered), { all: false, sessions: ["s2"] }));
        expect(seen).toEqual([{ all: true, sessions: ["s1", "s2"] }]);
        expect([...kept.entries()], "only what the engine did not take is offered again").toEqual([
            ["drift.autoAccept", JSON.stringify(["s2"])],
        ]);
        await handOverAutoAccept(async () => ({ all: false, sessions: [] }));
        expect(kept.size).toBe(0);
    } finally {
        Object.assign(storage, saved);
    }
});

test("shell timeout preferences normalize and persist explicit no-timeout", async () => {
    const { normalizeShellTimeout, setShellTimeoutMs, shellTimeoutMs, shellTimeoutPresets } =
        await import("../src/state/prefs");
    expect(shellTimeoutPresets).toEqual([60_000, 300_000, 900_000, 1_800_000]);
    expect(normalizeShellTimeout(null)).toBeNull();
    expect(normalizeShellTimeout(60_000)).toBe(60_000);
    expect(normalizeShellTimeout(59_999)).toBeNull();
    expect(normalizeShellTimeout(86_400_001)).toBeNull();
    expect(normalizeShellTimeout("300000")).toBeNull();

    const setItem = localStorage.setItem;
    const writes = new Map<string, string>();
    localStorage.setItem = (key, value) => writes.set(key, value);
    try {
        setShellTimeoutMs(900_000);
        expect(shellTimeoutMs()).toBe(900_000);
        expect(writes.get("drift.shell.timeout")).toBe("900000");
        setShellTimeoutMs(null);
        expect(shellTimeoutMs()).toBeNull();
        expect(writes.get("drift.shell.timeout")).toBe("null");
    } finally {
        localStorage.setItem = setItem;
    }
});

test("the About mascot always disposes its scene, including when it loads after unmount", async () => {
    const { mountScene } = await import("../src/ui/jellyfish");
    const settle = async () => {
        for (let tick = 0; tick < 4; tick++) await Promise.resolve();
    };

    let disposed = 0;
    let land!: (dispose: () => void) => void;
    const slow = new Promise<() => void>((resolve) => (land = resolve));
    mountScene(
        () => slow,
        () => {},
    )();
    land(() => disposed++);
    await settle();
    expect(disposed).toBe(1);

    let live = 0;
    const cleanup = mountScene(
        () => Promise.resolve(() => live++),
        () => {},
    );
    await settle();
    expect(live).toBe(0);
    cleanup();
    cleanup();
    expect(live).toBe(1);

    let fallbacks = 0;
    mountScene(
        () => Promise.reject(new Error("no webgl")),
        () => fallbacks++,
    );
    mountScene(
        () => Promise.resolve(undefined),
        () => fallbacks++,
    );
    await settle();
    expect(fallbacks).toBe(2);

    let ignored = 0;
    mountScene(
        () => Promise.reject(new Error("no webgl")),
        () => ignored++,
    )();
    await settle();
    expect(ignored).toBe(0);
});

test("the mascot takes the theme accent and the logo mark never flashes as a block", async () => {
    const { applyAccent } = await import("../src/ui/jelly/jellyfish");
    const THREE = await import("three");

    // The bell palette derives from the accent, so a themed mascot never falls back to stock aqua.
    applyAccent("#c9a9e0");
    const bell = (await import("../src/ui/jelly/jellyfish")).createJellyfish();
    const tinted: string[] = [];
    bell.group.traverse((object) => {
        const material = (object as { material?: unknown }).material as
            { uniforms?: Record<string, { value: unknown }> } | undefined;
        for (const name of ["uTop", "uBottom", "uRim", "uColor", "uTip"]) {
            const value = material?.uniforms?.[name]?.value;
            if (value instanceof THREE.Color) tinted.push(value.getHexString());
        }
    });
    expect(tinted.length).toBeGreaterThan(0);
    // Stock aqua (#8fd9fb and friends) must be gone entirely.
    expect(tinted).not.toContain("8fd9fb");
    expect(tinted).not.toContain("d4f2ff");
    expect(tinted).not.toContain("4f93cc");
    // Bell tints follow the accent hue, while fixed face colors keep the eyes and blush recognizable.
    const face = new Set(["ffa9b8", "ffffff", "0f1626"]);
    const bellTints = tinted.filter((hex) => !face.has(hex));
    expect(bellTints.length).toBeGreaterThan(0);
    for (const hex of bellTints) {
        const color = new THREE.Color(`#${hex}`);
        const hsl = { h: 0, s: 0, l: 0 };
        color.getHSL(hsl);
        if (hsl.s > 0.05) expect(Math.abs(hsl.h - 0.763)).toBeLessThan(0.05);
    }

    // The logo mask is inlined, so `background: currentColor` is never painted unmasked.
    const logo = await Bun.file("src/ui/logo.tsx").text();
    expect(logo).toContainCode("logo.svg?raw");
    expect(logo).toContainCode("data:image/svg+xml,${encodeURIComponent(logoSource)}");
    const jelly = await Bun.file("src/ui/jellyfish.tsx").text();
    // Tinted before the first frame and retinted on theme changes, with the observer torn down.
    expect(code(jelly)).toMatch(/applyAccent\(accentColor\(host\)\).*?jelly=createJellyfish\(\)/);
    expect(jelly).toContainCode("themeObserver.observe(document.documentElement");
    expect(jelly).toContainCode("themeObserver.disconnect()");
    expect(jelly).toContainCode("renderer.setClearColor(0x000000, 0)");

    // The canvas mounts hidden and is revealed only from inside render(), after a frame it drew.
    // Revealing at append time let WebView2 composite one opaque white frame first.
    expect(jelly).toContain('canvas.style.opacity = "0"');
    expect(code(jelly)).toMatch(
        /renderer\.render\(scene,camera\)[\s\S]*?if\(!revealed\)\{ ?revealed=true canvas\.style\.opacity="1" ready\(\)/,
    );
    // No reveal may happen next to the append, before any frame exists.
    expect(code(jelly)).not.toContain("host.append(canvas) ready()");
});

test("provider sign-in hides raw URLs, surfaces device codes, and keeps disconnect beside the methods", async () => {
    const { authorizationPrompt } = await import("../src/engine/provider-auth");
    const long = "https://claude.ai/oauth/authorize?code=true&client_id=9d1c&state=" + "x".repeat(400);
    expect(authorizationPrompt(`Paste the authorization code here: ${long}`)).toEqual({
        text: "Paste the authorization code here",
    });
    expect(authorizationPrompt("Enter code: ABCD-1234")).toEqual({ code: "ABCD-1234" });
    expect(authorizationPrompt("Open https://accounts.x.ai/device on any device and enter code: WXYZ-9876")).toEqual({
        code: "WXYZ-9876",
    });
    expect(authorizationPrompt("Sign in with `az login` before continuing.")).toEqual({
        text: "Sign in with `az login` before continuing.",
    });
    expect(authorizationPrompt("")).toEqual({});
    const source = await Bun.file("src/ui/settings-provider-connect.tsx").text();
    expect(source).not.toContainCode("{auth().url}");
    expect(source).not.toContainCode("disconnectDescription");
    const connect = source.slice(
        source.indexOf("function ProviderConnect("),
        source.indexOf("function AuthorizationHint("),
    );
    expect(connect.indexOf("props.methods.length > 1 || props.connected")).toBeLessThan(
        connect.indexOf('t("common.disconnect")'),
    );
    expect(connect.indexOf('t("common.disconnect")')).toBeLessThan(connect.indexOf('method()?.type === "api"'));
});

test("the About mascot stays light: preloaded from the nav, compiled off-thread, paced, and low-poly", async () => {
    const jelly = await Bun.file("src/ui/jellyfish.tsx").text();
    expect(jelly).toContainCode("await renderer.compileAsync(scene, camera)");
    expect(jelly).toContainCode('powerPreference: "low-power"');
    expect(jelly).toContainCode("if (last && now - last < interval - 1) return");
    expect(jelly).toContainCode("renderer.setPixelRatio(1)");
    const settings = await Bun.file("src/ui/settings.tsx").text();
    expect(settings).toContainCode('onPointerEnter={() => name === "About" && void preloadJellyfish()');
    const { createJellyfish } = await import("../src/ui/jelly/jellyfish");
    const seen = new Set<unknown>();
    let vertices = 0;
    createJellyfish().group.traverse((object) => {
        const geometry = (object as { geometry?: { attributes: { position: { count: number } } } }).geometry;
        if (!geometry || seen.has(geometry)) return;
        seen.add(geometry);
        vertices += geometry.attributes.position.count;
    });
    expect(vertices).toBeGreaterThan(5_000);
    expect(vertices).toBeLessThan(12_000);
});

test("permission rules reorder within the list and grants read as what was approved", async () => {
    const { loadDictionary } = await import("../src/state/i18n");
    await loadDictionary("en");
    const { moveRule } = await import("../src/ui/settings-permission-rules");
    const { grantLabel } = await import("../src/ui/settings-permissions");
    const rule = (pattern: string) => ({ kind: "bash", pattern, decision: "ask" as const });
    const rules = [rule("a"), rule("b"), rule("c")];
    expect(moveRule(rules, 2, -1).map((r) => r.pattern)).toEqual(["a", "c", "b"]);
    expect(moveRule(rules, 0, -1).map((r) => r.pattern)).toEqual(["a", "b", "c"], "the first stays first");
    expect(moveRule(rules, 2, 1).map((r) => r.pattern)).toEqual(["a", "b", "c"]);
    expect(grantLabel({ grant: "exact", kind: "edit", target: "src/[id].tsx" })).toBe("edit: src/[id].tsx");
    expect(grantLabel({ grant: "subcommand", prefix: "cargo test" })).toBe("bash: cargo test with any arguments");
    expect(grantLabel({ grant: "pattern", kind: "read", pattern: "docs/**", decision: "allow" })).toBe("read: docs/**");
});
test("always-allowed grants are grouped by what they let through and shown without the kind their group names", async () => {
    const { grantGroup, grantText } = await import("../src/ui/settings-permissions");
    const exact = { grant: "exact" as const, kind: "bash", target: "cargo test" };
    const sub = { grant: "subcommand" as const, prefix: "git push" };
    const folder = { grant: "folder" as const, kind: "read", folder: "C:/notes" };
    const site = {
        grant: "pattern" as const,
        kind: "webfetch",
        pattern: "https://docs.rs/*",
        decision: "allow" as const,
    };
    expect([exact, sub, folder, site].map(grantGroup)).toEqual(["shell", "shell", "files", "web"]);
    expect(grantText(exact)).toBe("cargo test");
    expect(grantText(site)).toBe("https://docs.rs/*");
});
test("web search lists providers in the order auto tries them, saved order first, and searches group as web", async () => {
    const { orderedProviders } = await import("../src/ui/settings-web-search");
    const { grantGroup } = await import("../src/ui/settings-permissions");
    const provider = (id: string) => ({ id, name: id, builtIn: true, keys: [], fromEnvironment: false });
    const state = {
        settings: { provider: "auto", order: ["tavily", "searxng"] },
        providers: ["firecrawl", "exa", "tavily", "searxng"].map(provider),
    };
    expect(orderedProviders(state).map((p) => p.id)).toEqual(["tavily", "searxng", "firecrawl", "exa"]);
    expect(grantGroup({ grant: "pattern", kind: "websearch", pattern: "*", decision: "allow" })).toBe("web");
});
test("image generation says what the chosen provider still needs, and when the ChatGPT plan alone pays", async () => {
    const { imageProviderNote } = await import("../src/ui/settings-image-generation");
    const provider = (id: string, ready: boolean, hasPlan = false) => ({
        id,
        name: id.toUpperCase(),
        models: ["m"],
        typed: false,
        access: "catalog",
        references: 5,
        hasKey: ready && !hasPlan,
        fromEnvironment: false,
        hasPlan,
        ready,
    });
    const state = (chosen: string) => ({
        settings: { provider: chosen, model: "", size: "auto", openaiFirst: "plan" as const, servers: {} },
        providers: [provider("openai", true), provider("xai", false), provider("plan", true, true)],
    });

    expect(imageProviderNote(state("xai"))).toContain("XAI");
    expect(imageProviderNote(state("openai"))).not.toContain("OPENAI");
    expect(imageProviderNote(state("off"))).toBe(imageProviderNote(state("openai")));
    expect(imageProviderNote(state("plan"))).toContain("ChatGPT plan");
});

test("tool grouping is one multi-select list and Code settings preview a diff with every setting applied", async () => {
    const general = await Bun.file("src/ui/settings-general.tsx").text();
    expect(general).toContainCode("checked={toolGroupKinds.filter((kind) => toolGroups()[kind])}");
    expect(general).toContainCode("fallbackLabel={groupedSummary()}");
    expect(general).not.toContain("<For each={toolGroupKinds}>");

    const picker = await Bun.file("src/ui/picker.tsx").text();
    expect(picker).toContainCode("if (!props.checked) setOpen(false);");

    const code = await Bun.file("src/ui/settings-code.tsx").text();
    expect(code).toContainCode('<DiffPanel diff={previewDiff} filename="greet.ts" />');

    const appearance = await Bun.file("src/ui/settings-appearance.tsx").text();
    expect(appearance).toContainCode("<ThinkingSettings />");
    expect(await Bun.file("src/ui/settings-general.tsx").text()).not.toContain("ThinkingSettings");
});

test("a theme saved before bases and accents became its base and the accent it showed", async () => {
    const { migratedTheme } = await import("../src/state/theme");

    expect(migratedTheme("drift-dark", "")).toEqual(["drift-charcoal", ""]);
    expect(migratedTheme("drift-default", "")).toEqual(["drift-charcoal", ""]);
    expect(migratedTheme("drift-graphite", "")).toEqual(["drift-charcoal", "silver"]);
    expect(migratedTheme("drift-slate", "")).toEqual(["drift-midnight", "teal"]);
    expect(migratedTheme("drift-aubergine", "")).toEqual(["drift-plum", ""]);
    expect(migratedTheme("drift-slate", "clay"), "an accent already chosen stays").toEqual(["drift-midnight", "clay"]);
    expect(migratedTheme("drift-paper", "green")).toEqual(["drift-paper", "green"]);
    expect(migratedTheme("someone-elses-theme", "")).toEqual(["drift-charcoal", ""]);
});

test("each area's text size is kept within its range and reaches the CSS for that area", async () => {
    const { conversationScale, setConversationScale, maxTextScale } = await import("../src/state/text-size");
    setConversationScale(1.2);
    expect(conversationScale()).toBe(1.2);
    localStorage.setItem("drift.text.conversation", JSON.stringify(9));
    const { persisted } = await import("../src/state/persist");
    const clamped = (value: unknown) => (typeof value === "number" ? Math.min(maxTextScale, Math.max(0.8, value)) : 1);
    expect(persisted("drift.text.conversation", 1, clamped)[0]()).toBe(maxTextScale);
    setConversationScale(1);

    const styles = await Bun.file("src/styles/app.css").text();
    expect(styles).toContainCode("font-size: calc(0.9375rem * var(--conversation-scale, 1));");
    expect(styles).toContainCode("--text-xs: calc(0.75rem * var(--sidebar-scale, 1));");
    expect(await Bun.file("src/ui/composer.tsx").text()).toContain("var(--composer-scale,1)");
});

test("the interface size applies when its slider is let go, so dragging it never rescales the slider under the pointer", async () => {
    const appearance = await Bun.file("src/ui/settings-appearance.tsx").text();
    expect(appearance).toContainCode("onInput={setInterfaceZoom} settleOnRelease");
    expect(appearance).toContainCode("onDragging={props.settleOnRelease ? setDragged : undefined}");

    const controls = await Bun.file("src/ui/controls.tsx").text();
    expect(controls).toContainCode("if (dragging()) props.onDragging?.(value); else props.onInput(value);");
    expect(controls).toContainCode("onPointerUp={(event) => dragging() && settle(Number(event.currentTarget.value))}");
});

test("the interface, Settings and icons each have a size of their own, read by their part of the page alone", async () => {
    const styles = await Bun.file("src/styles/app.css").text();
    expect(styles).toContainCode("--text-xs: calc(0.75rem * var(--ui-text-scale, 1));");
    expect(styles).toContainCode(".settings-scale { --text-xs: calc(0.75rem * var(--settings-scale, 1));");
    expect(styles).toContainCode(".app-icon { scale: var(--icon-scale, 1); }");
    expect(await Bun.file("src/ui/settings.tsx").text()).toContain("settings-dialog settings-scale");
    expect(await Bun.file("src/ui/icons.tsx").text()).toContain("app-icon");

    const { setIconScale, iconScale, maxIconScale } = await import("../src/state/text-size");
    setIconScale(1.2);
    expect(iconScale()).toBe(1.2);
    expect(maxIconScale).toBeLessThan(1.5);
    setIconScale(1);
});
