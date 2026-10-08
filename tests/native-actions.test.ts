import { rememberProviderCatalog } from "../src/state/provider-cache";
import { EngineError } from "../src/engine/native/client";
import { createEngineState } from "../src/engine/store";
import { createActions } from "../src/engine/actions";
import { afterEach, expect, test } from "bun:test";

import type { components } from "../src/engine/native/types";
import type { Client } from "../src/engine/native/client";

type Session = components["schemas"]["Session"];

// refreshProviders remembers the catalog in a module signal; other suites expect it empty.
afterEach(() => rememberProviderCatalog([], [], {}));

function session(id: string, workspaceId = "w1"): Session {
    return { id, workspaceId, visibility: "sibling", title: id, agent: "build", createdAt: 1, updatedAt: 2 };
}

function harness(overrides: Partial<Client> = {}) {
    const calls: { method: string; args: unknown[] }[] = [];
    const record =
        <T>(method: string, result: T) =>
        (...args: unknown[]) => {
            calls.push({ method, args });
            return Promise.resolve(result);
        };
    const client = {
        sessions: record("sessions", [session("ses_1"), session("ses_2")]),
        createSession: record("createSession", session("ses_new")),
        updateSession: record("updateSession", { ...session("ses_1"), title: "Renamed" }),
        messages: record("messages", []),
        submit: record("submit", { session: session("ses_1"), message: {} }),
        abort: record("abort", { aborted: true }),
        providers: record("providers", [
            {
                id: "anthropic",
                name: "Anthropic",
                connected: true,
                credential: "keychain",
                models: {
                    claude: {
                        id: "claude",
                        name: "Claude",
                        reasoning: true,
                        attachment: true,
                        temperature: true,
                        family: "c",
                        release_date: "",
                        limit: { context: 200000, output: 8192 },
                        cost: { input: 3, output: 15, cache_read: 0.3, cache_write: 3.75 },
                        profile: "edit",
                    },
                },
            },
            { id: "openai", name: "OpenAI", connected: false, models: {} },
        ]),
        permissions: record("permissions", []),
        replyPermission: record("replyPermission", undefined),
        setProviderKey: record("setProviderKey", undefined),
        ...overrides,
    } as unknown as Client;
    const [state, set] = createEngineState();
    set("directory", "C:/repo");
    const workspaces = () => ({
        path: (id: string) => (id === "w1" ? "C:/repo" : undefined),
        id: (path: string) => (path === "C:/repo" ? "w1" : undefined),
    });
    const actions = createActions(() => client, state, set, workspaces);
    return { actions, state, calls };
}

test("loading sessions for a directory scopes the request to its workspace", async () => {
    const h = harness();
    await h.actions.loadSessions("C:/repo");
    expect(h.calls[0]).toEqual({ method: "sessions", args: [{ workspace: "w1", limit: 200 }] });
    expect(Object.keys(h.state.sessions).sort()).toEqual(["ses_1", "ses_2"]);
    expect(h.state.sessions.ses_1!.directory).toBe("C:/repo");
});

test("file mentions search the current workspace through the engine", async () => {
    const searched: unknown[][] = [];
    const h = harness({
        findFiles: (...args: unknown[]) => (searched.push(args), Promise.resolve(["src/composer.tsx"])),
    } as Partial<Client>);
    expect(await h.actions.findFiles("comp")).toEqual(["src/composer.tsx"]);
    expect(searched).toEqual([["w1", "comp"]]);
});

test("send maps model, agent, files and reasoning effort onto the native prompt", async () => {
    const h = harness();
    const result = await h.actions.send("ses_1", "hello", {
        model: { providerID: "anthropic", modelID: "claude" },
        agent: "plan",
        variant: "high",
        files: [{ mime: "image/png", url: "data:image/png;base64,AAAA", filename: "shot.png" }],
    });
    expect(result).toEqual({ ok: true });
    const submitted = h.calls[0] as { method: string; args: [string, { submissionId?: string }] };
    expect(typeof submitted.args[1].submissionId).toBe("string");
    delete submitted.args[1].submissionId;
    expect(h.calls[0]).toEqual({
        method: "submit",
        args: [
            "ses_1",
            {
                parts: [
                    { type: "text", text: "hello" },
                    { type: "file", mime: "image/png", name: "shot.png", url: "data:image/png;base64,AAAA" },
                ],
                model: { provider: "anthropic", model: "claude" },
                variant: "high",
                agent: "plan",
            },
        ],
    });
});

test("a sent prompt's choice is what the session runs as next, mid-turn or not", async () => {
    const { savedChoice } = await import("../src/engine/store");
    const switched = {
        ...session("ses_1"),
        agent: "plan",
        variant: "high",
        model: { provider: "openai", model: "gpt-5" },
    };
    const h = harness({ submit: () => Promise.resolve({ session: switched, message: {} }) } as Partial<Client>);
    expect(
        await h.actions.send("ses_1", "plan again", {
            model: { providerID: "openai", modelID: "gpt-5" },
            agent: "plan",
            variant: "high",
        }),
    ).toEqual({ ok: true });
    expect(savedChoice(h.state, "ses_1")).toEqual({
        agent: "plan",
        variant: "high",
        model: { providerID: "openai", modelID: "gpt-5" },
    });
});

test("a follow-up names its agent and level only when they change what the session runs as next", async () => {
    const sent: Record<string, unknown>[] = [];
    const saved = { ...session("ses_1"), agent: "plan", variant: "high" };
    const h = harness({
        sessions: () => Promise.resolve([saved]),
        submit: (_id: string, body: Record<string, unknown>) => (sent.push(body), Promise.resolve({ session: saved })),
    } as Partial<Client>);
    await h.actions.loadSessions("C:/repo");
    const named = (index: number) => ({
        agent: sent[index]!.agent,
        variant: sent[index]!.variant,
        hasVariant: "variant" in sent[index]!,
    });
    await h.actions.send("ses_1", "same", { model: null, agent: "plan", variant: "high" });
    await h.actions.send("ses_1", "not offered", { model: null, agent: "plan", variant: undefined });
    await h.actions.send("ses_1", "default", { model: null, agent: "plan", variant: null });
    await h.actions.send("ses_1", "build now", { model: null, agent: "build", variant: "high" });
    expect([0, 1, 2, 3].map(named)).toEqual([
        { agent: undefined, variant: undefined, hasVariant: false },
        { agent: undefined, variant: undefined, hasVariant: false },
        { agent: undefined, variant: null, hasVariant: true },
        { agent: "build", variant: undefined, hasVariant: false },
    ]);
});

test("send failures land in the session's error slot", async () => {
    const h = harness({
        submit: () => Promise.reject(new EngineError(409, "/turns", "busy", "session is already running a turn")),
    });
    const result = await h.actions.send("ses_1", "again", { model: null, agent: "build" });
    expect(result).toEqual({ ok: false, error: "Prompt failed: session is already running a turn" });
    expect(h.state.errors.ses_1).toBe("Prompt failed: session is already running a turn");
    expect(await h.actions.send("ses_1", "   ", { model: null, agent: "build" })).toEqual({
        ok: false,
        error: "Prompt failed: the prompt is empty",
    });
});

test("providers become the catalog shape the picker reads", async () => {
    const h = harness();
    await h.actions.refreshProviders();
    expect(h.state.connected).toEqual(["anthropic"]);
    expect(h.state.providers.map((p) => p.id)).toEqual(["anthropic", "openai"]);
    const model = h.state.providers[0]!.models.claude!;
    expect(model.capabilities.toolcall).toBe(true);
    expect(model.cost.cache).toEqual({ read: 0.3, write: 3.75 });
    expect((model as { limit: { context: number } }).limit.context).toBe(200000);
});

test("permission replies translate reject to deny and forget stale requests", async () => {
    const replies: unknown[][] = [];
    const h = harness({
        replyPermission: (id: string, body: unknown) => {
            replies.push([id, body]);
            return id === "gone" ? Promise.reject(new EngineError(404, "/p")) : Promise.resolve();
        },
    });
    await h.actions.replyPermission("ses_1", "perm_1", "reject");
    await h.actions.replyPermission("ses_1", "perm_2", "always");
    await h.actions.replyPermission("ses_1", "perm_3", "stop");
    expect(replies).toEqual([
        ["perm_1", { reply: "deny" }],
        ["perm_2", { reply: "always" }],
        ["perm_3", { reply: "stop" }],
    ]);
    h.state.permissions.ses_1 = [
        {
            id: "gone",
            kind: "bash",
            tool: "bash",
            sessionId: "ses_1",
            messageId: "m",
            callId: "c",
            title: "t",
            pattern: "git status",
            createdAt: 0,
        },
    ];
    await h.actions.replyPermission("ses_1", "gone", "once");
    expect(h.state.permissions.ses_1).toEqual([]);
});

test("new sessions are created in the active workspace", async () => {
    const h = harness();
    const created = await h.actions.newSession();
    expect(h.calls[0]).toEqual({ method: "createSession", args: [{ workspaceId: "w1" }] });
    expect(created?.id).toBe("ses_new");
    expect(h.state.sessions.ses_new).toBeDefined();
    expect(h.state.loaded.ses_new).toBe(true);
    expect(h.state.transcripts.ses_new).toEqual([]);
});

test("session listings page to the end and restore running status from every page", async () => {
    const pages: unknown[][] = [];
    const full = Array.from({ length: 200 }, (_, i) => ({ ...session(`ses_${i}`), running: i === 7 }));
    const h = harness({
        sessions: ((params: { before?: string }) => {
            pages.push([params]);
            const page = params.before ? [{ ...session("ses_tail"), running: true }] : full;
            return Promise.resolve(page);
        }) as unknown as Client["sessions"],
    });
    await h.actions.loadSessions("C:/repo");
    expect(pages.length).toBe(2);
    expect(pages[1]).toEqual([{ workspace: "w1", before: "ses_199", limit: 200 }]);
    expect(Object.keys(h.state.sessions).length).toBe(201);
    expect(h.state.status.ses_tail).toEqual({ type: "busy" });
    expect(h.state.status.ses_7).toEqual({ type: "busy" });
    expect(h.state.status.ses_0).toEqual({ type: "idle" });
});

test("purge deletes for real and only reports success when the engine confirms", async () => {
    const deleted: string[] = [];
    const ok = harness({ deleteSession: (id: string) => (deleted.push(id), Promise.resolve()) });
    ok.state.sessions.ses_1 = { id: "ses_1", directory: "C:/repo" } as never;
    expect(await ok.actions.purgeSession("ses_1")).toBe(true);
    expect(deleted).toEqual(["ses_1"]);
    expect(ok.state.sessions.ses_1).toBeUndefined();
    expect(ok.calls.some((c) => c.method === "updateSession")).toBe(false);
    const failing = harness({ deleteSession: () => Promise.reject(new EngineError(500, "/sessions/x", "store")) });
    failing.state.sessions.ses_1 = { id: "ses_1", directory: "C:/repo" } as never;
    expect(await failing.actions.purgeSession("ses_1")).toBe(false);
    expect(failing.state.sessions.ses_1).toBeDefined();
    const gone = harness({ deleteSession: () => Promise.reject(new EngineError(404, "/sessions/x")) });
    expect(await gone.actions.purgeSession("ses_1")).toBe(true);
});

test("every send carries a fresh submission id", async () => {
    const h = harness();
    await h.actions.send("ses_1", "a", { model: null, agent: "build" });
    await h.actions.send("ses_1", "b", { model: null, agent: "build" });
    const ids = h.calls
        .filter((c) => c.method === "submit")
        .map((c) => (c.args[1] as { submissionId: string }).submissionId);
    expect(ids).toHaveLength(2);
    expect(ids[0]).toBeTruthy();
    expect(ids[0]).not.toBe(ids[1]);
});

test("resending a prompt whose answer was lost reuses its submission id; a refusal or a changed prompt does not", async () => {
    const ids: string[] = [];
    let failure: Error | undefined;
    const h = harness({
        submit: async (_id: string, body: { submissionId: string }) => {
            ids.push(body.submissionId);
            if (failure) throw failure;
            return { session: session("ses_1"), message: {} };
        },
    } as Partial<Client>);
    const send = (text: string) => h.actions.send("ses_1", text, { model: null, agent: "build" });
    failure = new TypeError("Failed to fetch");
    await send("hello");
    failure = new EngineError(503, "/sessions/ses_1/turns", "store", "busy");
    await send("hello");
    failure = undefined;
    await send("hello");
    expect(new Set(ids).size, "lost answers and server errors keep one identity").toBe(1);
    failure = new EngineError(400, "/sessions/ses_1/turns", "attachment", "too big");
    await send("again");
    failure = undefined;
    await send("again");
    await send("hello");
    expect(new Set(ids).size, "a refusal and a success each end the identity").toBe(4);
});

test("hydration rejects when any of its loads fail, instead of pretending the snapshot landed", async () => {
    const { hydrateFrom } = await import("../src/engine/index");
    const good = {
        refreshProviders: async () => true,
        loadSessions: async () => undefined,
        refreshPermissions: async () => undefined,
        refreshAgents: async () => undefined,
        refreshMcp: async () => undefined,
        refreshEngineSettings: async () => undefined,
    };
    await hydrateFrom(good, "C:/repo");
    await expect(
        hydrateFrom({ ...good, loadSessions: () => Promise.reject(new Error("db")) }, "C:/repo"),
    ).rejects.toThrow("db");
    await expect(hydrateFrom({ ...good, refreshMcp: () => Promise.reject(new Error("mcp")) }, null)).rejects.toThrow(
        "mcp",
    );
    await expect(hydrateFrom({ ...good, refreshProviders: async () => false }, null)).rejects.toThrow(
        "provider catalog",
    );
    await expect(
        hydrateFrom({ ...good, refreshPermissions: () => Promise.reject(new Error("perm")) }, null),
    ).rejects.toThrow("perm");
});

test("archiving and restoring go to the engine and keep the session's record in the store", async () => {
    const sent: unknown[] = [];
    const h = harness({
        updateSession: async (id: string, body: { archived?: boolean }) => {
            sent.push([id, body]);
            return { ...session(id), ...(body.archived ? { archivedAt: 50 } : {}) };
        },
    } as Partial<Client>);
    await h.actions.setArchived("ses_1", true);
    expect(h.state.sessions.ses_1?.archivedAt).toBe(50);
    await h.actions.setArchived("ses_1", false);
    expect(h.state.sessions.ses_1?.archivedAt).toBeUndefined();
    expect(sent).toEqual([
        ["ses_1", { archived: true }],
        ["ses_1", { archived: false }],
    ]);
});

type McpServer = components["schemas"]["ServerStatus"];

function mcpServer(name: string, state: McpServer["state"] = "connected"): McpServer {
    return {
        name,
        config: { type: "stdio", command: "node", args: ["server.js"], env: [] },
        enabled: state !== "disabled",
        updatedAt: 1,
        state,
        tools: [],
    };
}

test("MCP changes go to the engine and the store holds what it reports", async () => {
    const h = harness({
        mcpServers: async () => [mcpServer("docs", "connected")],
        saveMcpServer: async (name: string) => mcpServer(name),
        setMcpServerEnabled: async (name: string, enabled: boolean) => ({
            ...mcpServer(name, enabled ? "connected" : "disabled"),
            enabled,
        }),
        removeMcpServer: async () => undefined,
    } as Partial<Client>);
    await h.actions.refreshMcp();
    expect(Object.keys(h.state.mcpServers)).toEqual(["docs"]);
    await h.actions.mcpSave("files", { type: "stdio", command: "npx", args: [] });
    expect(h.state.mcpServers.files.state).toBe("connected");
    await h.actions.mcpSetEnabled("files", false);
    expect(h.state.mcpServers.files.enabled).toBeFalse();
    await h.actions.mcpRemove("docs");
    expect(Object.keys(h.state.mcpServers)).toEqual(["files"]);
});

test("renaming an MCP server is the engine's one step, and a taken name changes nothing", async () => {
    const sent: unknown[] = [];
    const h = harness({
        mcpServers: async () => [mcpServer("old", "connected"), mcpServer("taken", "connected")],
        renameMcpServer: async (name: string, to: string) => {
            sent.push([name, to]);
            if (to === "taken")
                throw new EngineError(409, `/mcp/${name}/rename`, "taken", "a server named taken already exists");
            return mcpServer(to, "connected");
        },
        saveMcpServer: async (
            name: string,
            _config: unknown,
            options: { create?: boolean; readOnlyTrusted?: boolean },
        ) => {
            sent.push(["save", name, options]);
            return mcpServer(name);
        },
    } as Partial<Client>);
    await h.actions.refreshMcp();
    await expect(h.actions.mcpRename("old", "taken")).rejects.toThrow("already exists");
    expect(Object.keys(h.state.mcpServers).sort()).toEqual(["old", "taken"]);
    await h.actions.mcpRename("old", "new");
    expect(Object.keys(h.state.mcpServers).sort()).toEqual(["new", "taken"]);
    await h.actions.mcpSave("added", { type: "stdio", command: "x" }, { create: true, readOnlyTrusted: false });
    expect(sent).toEqual([
        ["old", "taken"],
        ["old", "new"],
        ["save", "added", { create: true, readOnlyTrusted: false }],
    ]);
});

test("action agents are listed for Settings but hidden from the composer, with their pins and prompts", async () => {
    const config = {
        agents: [
            { name: "build", description: "", builtin: true, kind: "primary" },
            {
                name: "title",
                description: "",
                builtin: true,
                kind: "action",
                prompt: "Name it.",
                model: { provider: "openai", model: "gpt-5-nano" },
            },
            { name: "explore", description: "", builtin: true, kind: "subagent", prompt: "Search." },
        ],
        commands: [],
        skills: [],
    };
    const h = harness({ workspaceConfig: () => Promise.resolve(config) } as Partial<Client>);
    await h.actions.refreshAgents();
    const [build, title, explore] = h.state.agents as ((typeof h.state.agents)[number] & {
        hidden?: boolean;
        prompt?: string;
    })[];
    expect(build!.hidden).toBeFalse();
    expect(build!.mode).toBe("primary");
    expect(explore!.mode).toBe("subagent");
    expect(explore!.prompt).toBe("Search.");
    expect(title!.hidden).toBeTrue();
    expect(title!.prompt).toBe("Name it.");
    expect(title!.model).toEqual({ providerID: "openai", modelID: "gpt-5-nano" });
});

test("a skill's documented choices reach the slash menu, and an MCP prompt's arguments are its usage", async () => {
    const config = {
        agents: [],
        commands: [
            {
                name: "design",
                description: "Design",
                template: "t",
                usage: "[audit|polish] [target]",
                subcommands: [
                    { name: "audit", description: "Check it", usage: "[target]" },
                    { name: "polish", description: "Finish it" },
                ],
            },
            {
                name: "docs:search",
                description: "Search docs",
                template: "",
                server: "docs",
                arguments: ["query", "limit"],
            },
            { name: "plain", description: "Plain", template: "Do it." },
        ],
        skills: [],
    };
    const h = harness({ workspaceConfig: () => Promise.resolve(config) } as Partial<Client>);
    await h.actions.refreshAgents();
    const [design, search, plain] = h.state.commands;
    expect(design).toEqual({
        name: "design",
        description: "Design",
        template: "t",
        usage: "[audit|polish] [target]",
        subcommands: [
            { name: "audit", description: "Check it", usage: "[target]" },
            { name: "polish", description: "Finish it" },
        ],
    });
    expect(search!.usage).toBe("<query> <limit>");
    expect(plain).toEqual({ name: "plain", description: "Plain", template: "Do it." });
});

test("/compact asks the engine to compact and reports a refusal; the auto setting round-trips", async () => {
    const h = harness({
        compactSession: (id: string) =>
            id === "ses_busy"
                ? Promise.reject(new EngineError(409, "/sessions/ses_busy/compact", "busy", "a turn is running"))
                : Promise.resolve(undefined),
        putSettings: (body: { autoCompact: boolean }) => Promise.resolve(body),
    } as Partial<Client>);
    await h.actions.summarize("ses_1");
    expect(h.state.notices.length).toBe(0);
    await h.actions.summarize("ses_busy");
    expect(h.state.notices.some((n) => n.title === "Couldn't compact" && n.message === "a turn is running")).toBeTrue();
    expect(await h.actions.setAutoCompact(false)).toEqual({ autoCompact: false });
});

test("a sign-in hands the panel its device code and no English text, so the panel words it in the user's language", async () => {
    const h = harness({
        startOAuth: (id: string) =>
            Promise.resolve(
                id === "xai"
                    ? {
                          url: "https://accounts.x.ai/device?code=WXYZ-9876",
                          state: "s",
                          method: "auto",
                          userCode: "WXYZ-9876",
                      }
                    : { url: "https://claude.ai/oauth", state: "s", method: "code" },
            ),
    } as Partial<Client>);
    expect(await h.actions.providerAuthorize("xai", 0)).toEqual({
        url: "https://accounts.x.ai/device?code=WXYZ-9876",
        method: "auto",
        instructions: "",
        code: "WXYZ-9876",
    });
    expect(await h.actions.providerAuthorize("anthropic", 0)).toEqual({
        url: "https://claude.ai/oauth",
        method: "code",
        instructions: "",
        code: undefined,
    });
});

test("a removed workspace's purge completes only once the engine holds none of its conversations", async () => {
    const calls: string[] = [];
    const h = harness({
        purgeWorkspace: (id: string) => {
            calls.push(id);
            if (id === "busy")
                return Promise.reject(new EngineError(409, `/workspaces/${id}/purge`, "busy", "running"));
            if (id === "gone")
                return Promise.reject(new EngineError(404, `/workspaces/${id}/purge`, "not_found", "workspace"));
            return Promise.resolve({ deleted: 3 });
        },
    } as Partial<Client>);
    expect(await h.actions.removeAllSessions("ws_1", () => true)).toBeTrue();
    expect(await h.actions.removeAllSessions("busy", () => true)).toBeFalse();
    expect(await h.actions.removeAllSessions("gone", () => true)).toBeTrue();
    expect(await h.actions.removeAllSessions("restored", () => false)).toBeFalse();
    expect(calls).toEqual(["ws_1", "busy", "gone"]);
});

test("undo and redo apply the engine's session and report refusals", async () => {
    const asked: boolean[] = [];
    const h = harness({
        revertSession: (id: string, messageId: string, keepFiles = false) => {
            asked.push(keepFiles);
            return id === "ses_busy"
                ? Promise.reject(new EngineError(409, `/sessions/${id}/revert`, "busy", "stop the running turn first"))
                : Promise.resolve({
                      session: { ...session(id), revert: { messageId } },
                      kept: [],
                      unattributed: [],
                      unrecorded: [],
                  });
        },
        unrevertSession: (id: string) =>
            Promise.resolve({
                session: session(id),
                kept: ["src/app.ts"],
                unattributed: ["dist/out.js"],
                unrecorded: ["C:/repo/old.rs"],
            }),
    } as Partial<Client>);
    expect(await h.actions.revert("ses_1", "msg_2")).toBeTrue();
    expect(await h.actions.revert("ses_1", "msg_2", true)).toBeTrue();
    expect(asked.slice(0, 2), "Shift asks the engine to leave the files").toEqual([false, true]);
    expect(h.state.sessions.ses_1?.revert?.messageId).toBe("msg_2");
    expect(h.state.notices.length).toBe(0);
    expect(await h.actions.unrevert("ses_1")).toBeTrue();
    expect((h.state.sessions.ses_1 as { revert?: unknown }).revert).toBeUndefined();
    expect(h.state.notices.some((n) => n.title === "Kept your changes" && n.message.includes("src/app.ts"))).toBeTrue();
    expect(
        h.state.notices.some(
            (n) => n.title === "Some imported edits were not undone" && n.message.includes("C:/repo/old.rs"),
        ),
    ).toBeTrue();
    expect(
        h.state.notices.some(
            (n) => n.title === "Left files changed during commands" && n.message.includes("dist/out.js"),
        ),
    ).toBeTrue();
    expect(await h.actions.revert("ses_busy", "msg_2")).toBeFalse();
    expect(h.state.notices.some((n) => n.title === "Couldn't undo")).toBeTrue();
});

test("switching a retrying turn's model sends the native model ref and variant and reports refusals", async () => {
    const sent: unknown[] = [];
    const h = harness({
        switchRetryModel: (id: string, model: unknown, variant: string | null) => {
            sent.push([id, model, variant]);
            return id === "ses_idle"
                ? Promise.reject(
                      new EngineError(
                          409,
                          "/sessions/ses_idle/retry",
                          "not_retrying",
                          "the session is not waiting to retry",
                      ),
                  )
                : Promise.resolve(undefined);
        },
    } as Partial<Client>);
    expect(
        await h.actions.switchRetryModel("ses_1", "msg_1", { providerID: "openai", modelID: "gpt-5" }, "high"),
    ).toEqual({ ok: true });
    expect(sent[0]).toEqual(["ses_1", { provider: "openai", model: "gpt-5" }, "high"]);
    expect(await h.actions.switchRetryModel("ses_1", "msg_1", { providerID: "openai", modelID: "gpt-5" })).toEqual({
        ok: true,
    });
    expect(sent[1]).toEqual(["ses_1", { provider: "openai", model: "gpt-5" }, null]);
    expect(await h.actions.switchRetryModel("ses_idle", "msg_1", { providerID: "openai", modelID: "gpt-5" })).toEqual({
        ok: false,
        error: "the session is not waiting to retry",
    });
});

test("a spawned thread is one call, top level and linked to its source, and loads its copied history", async () => {
    const sent: string[][] = [];
    const h = harness({
        spawnThread: (id: string, instruction: string) => {
            sent.push([id, instruction]);
            return Promise.resolve({
                ...session("ses_spawn"),
                parentId: "ses_1",
                branchCutoff: "msg_9",
                title: "Fix lint",
            });
        },
    } as Partial<Client>);
    const created = await h.actions.spawn("ses_1", "fix lint");
    expect(sent).toEqual([["ses_1", "fix lint"]]);
    expect(created?.id).toBe("ses_spawn");
    expect(h.state.sessions.ses_spawn!.visibility).toBe("sibling");
    expect(h.state.links.ses_spawn).toBe("ses_1");
    expect(h.state.loaded.ses_spawn).toBeFalsy();
});

test("a refused spawn reports a notice instead of throwing", async () => {
    const h = harness({
        spawnThread: () =>
            Promise.reject(
                new EngineError(400, "/sessions/ses_1/spawn", "instruction", "say what the new thread should do"),
            ),
    } as Partial<Client>);
    expect(await h.actions.spawn("ses_1", " ")).toBeUndefined();
    expect(h.state.notices.some((n) => n.title === "Couldn't spawn the thread")).toBeTrue();
});

test("fork opens the copy as a new top-level session", async () => {
    const h = harness({
        forkSession: (id: string) => Promise.resolve({ ...session("ses_fork"), title: `${id} (fork)` }),
    } as Partial<Client>);
    const fork = await h.actions.fork("ses_1", "active");
    expect(fork?.id).toBe("ses_fork");
    expect(h.state.sessions.ses_fork!.title).toBe("ses_1 (fork)");
    expect(h.state.sessions.ses_fork!.parentId).toBeUndefined();
});

test("moving resolves the destination workspace and reports the engine's busy refusal", async () => {
    const moved = harness({
        moveSession: (_id: string, workspaceId: string) => Promise.resolve({ moved: ["ses_1", workspaceId] }),
    } as Partial<Client>);
    expect(await moved.actions.moveSession("ses_1", "C:/repo")).toEqual({ ok: true, moved: ["ses_1", "w1"] });
    expect((await moved.actions.moveSession("ses_1", "D:/unknown")).ok).toBeFalse();
    const busy = harness({
        moveSession: () =>
            Promise.reject(new EngineError(409, "/sessions/ses_1/move", "busy", "stop the running turn first")),
    } as Partial<Client>);
    expect(await busy.actions.moveSession("ses_1", "C:/repo")).toEqual({
        ok: false,
        moved: [],
        error: "stop the running turn first",
    });
});

test("re-pointing a workspace folder moves nothing but waits for running threads", async () => {
    const idle = harness();
    expect(await idle.actions.moveWorkspaceSessions("C:/repo", "D:/repo")).toEqual({ ok: true, moved: [] });
    const running = harness({
        sessions: () => Promise.resolve([{ ...session("ses_1"), running: true }]),
    } as Partial<Client>);
    expect((await running.actions.moveWorkspaceSessions("C:/repo", "D:/repo")).ok).toBeFalse();
});

test("a folder's always-grants are read and revoked through its engine workspace; a folder with none has nothing to revoke", async () => {
    const calls: unknown[] = [];
    const grant = { grant: "subcommand" as const, prefix: "cargo test" };
    const h = harness({
        permissionGrants: async (id: string) => (calls.push(["list", id]), [grant]),
        revokePermissionGrant: async (id: string, revoked: unknown) => void calls.push(["revoke", id, revoked]),
        revokePermissionGrants: async (id: string) => void calls.push(["revokeAll", id]),
    } as Partial<Client>);
    expect(await h.actions.workspaceGrants("C:/repo")).toEqual([grant]);
    expect(await h.actions.workspaceGrants("C:/elsewhere")).toEqual([]);
    await h.actions.revokeGrant("C:/repo", grant);
    await h.actions.revokeGrant("C:/repo");
    await h.actions.revokeGrant("C:/elsewhere");
    expect(calls).toEqual([
        ["list", "w1"],
        ["revoke", "w1", grant],
        ["revokeAll", "w1"],
    ]);
});
test("a prompt too large for the engine is refused with its size before it is sent", async () => {
    const { maxRequestBytes } = await import("../src/engine/native/client");
    const engine = await Bun.file("crates/drift-engine/src/api/mod.rs").text();
    expect(engine).toContain(`pub const MAX_REQUEST_BYTES: usize = ${maxRequestBytes / 1024 / 1024} * 1024 * 1024;`);
    const h = harness();
    // A 2.5 MB screenshot, which the engine's old 2 MB default refused, goes through.
    const shot = { mime: "image/png", filename: "shot.png", url: `data:image/png;base64,${"A".repeat(3_400_000)}` };
    expect(await h.actions.send("ses_1", "look", { model: null, agent: "build", files: [shot] as never })).toEqual({
        ok: true,
    });
    const huge = { ...shot, url: `data:image/png;base64,${"A".repeat(maxRequestBytes)}` };
    const result = await h.actions.send("ses_1", "look", { model: null, agent: "build", files: [huge] as never });
    expect(result.ok).toBe(false);
    expect((result as { error: string }).error).toBe(
        "Prompt failed: its attachments come to 65 MB, more than the 64 MB one prompt can carry. Send fewer or smaller files.",
    );
    expect(h.calls.filter((call) => call.method === "submit")).toHaveLength(1);
});
