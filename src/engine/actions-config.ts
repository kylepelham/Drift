import { produce, reconcile } from "solid-js/store";
import { handOverAutoAccept } from "../state/prefs";
import { sessionInWorkspace } from "./sessions";
import { EngineError } from "./native/client";
import { putSession } from "./store";

import type { AgentInfo, CommandInfo, McpServerConfig, McpServerStatus } from "./store";
import type { PermissionGrant, PermissionRule, Client } from "./native/client";
import type { ActionContext } from "./actions-context";

/**
 * Engine and workspace configuration: settings, agents and commands, MCP servers, grants, plugins, skills and prompts.
 */
export function createConfigActions({ requireClient, state, set, workspaces, notice }: ActionContext) {
    async function engineSettings() {
        return requireClient().settings();
    }

    async function setAutoCompact(autoCompact: boolean) {
        return requireClient().putSettings({ autoCompact });
    }

    /** How many background subagents run at once; the engine starts or holds queued ones to match. */
    async function setBackgroundTaskLimit(backgroundTaskLimit: number) {
        return requireClient().putSettings({ backgroundTaskLimit });
    }

    /**
     * The engine answers this session's asks, and its subagents', except secrets and anything outside the workspace.
     */
    async function setAutoAccept(id: string, autoAccept: boolean) {
        const updated = await requireClient().updateSession(id, { autoAccept });
        putSession(set, sessionInWorkspace(updated, workspaces()));
    }

    async function setAutoAcceptAll(autoAcceptAll: boolean) {
        const settings = await requireClient().putSettings({ autoAcceptAll });
        set("autoAcceptAll", !!settings.autoAcceptAll);
    }

    /** Settings the UI shows from the engine; auto-accept the webview once kept is handed over the first time. */
    async function refreshEngineSettings() {
        const settings = await requireClient().settings();
        set("autoAcceptAll", !!settings.autoAcceptAll);
        await handOverAutoAccept(async (kept) => {
            if (kept.all && !settings.autoAcceptAll) await setAutoAcceptAll(true);
            const left: string[] = [];
            for (const id of kept.sessions) {
                // A session the engine no longer has is let go; any other failure is offered again next time.
                const settled = await setAutoAccept(id, true).then(
                    () => true,
                    (cause) => cause instanceof EngineError && cause.status === 404,
                );
                if (!settled) left.push(id);
            }
            return { all: false, sessions: left };
        });
    }

    /** Agents and commands come from the workspace's drift.json and .drift/ directory. */
    async function refreshAgents() {
        const workspace = workspaces().id(state.directory);
        if (!workspace) return;
        const config = await requireClient().workspaceConfig(workspace);
        const agents: AgentInfo[] = config.agents.map((agent) => ({
            name: agent.name,
            description: agent.description,
            mode: agent.kind === "subagent" || agent.kind === "all" ? agent.kind : "primary",
            hidden: agent.kind === "action" || !!agent.hidden,
            builtIn: agent.builtin,
            tools: agent.tools ?? [],
            permissions: agent.permissions ?? [],
            ...(agent.variant ? { variant: agent.variant } : {}),
            ...(agent.problem ? { problem: agent.problem } : {}),
            ...(agent.prompt ? { prompt: agent.prompt } : {}),
            ...(agent.steps ? { steps: agent.steps } : {}),
            ...(agent.model ? { model: { providerID: agent.model.provider, modelID: agent.model.model } } : {}),
        }));
        // Skill usage and choices fill the slash menu; an MCP prompt's arguments become its usage, word by word.
        const commands: CommandInfo[] = config.commands.map((command) => {
            const usage =
                command.usage ??
                (command.arguments?.length
                    ? command.arguments.map((argument) => `<${argument}>`).join(" ")
                    : undefined);
            return {
                name: command.name,
                description: command.description,
                template: command.template,
                ...(usage ? { usage } : {}),
                ...(command.subcommands?.length
                    ? {
                          subcommands: command.subcommands.map((choice) => ({
                              name: choice.name,
                              description: choice.description,
                              ...(choice.usage ? { usage: choice.usage } : {}),
                          })),
                      }
                    : {}),
            };
        });
        // A config file that cannot be read stops every turn here until it is fixed; say so before the first send.
        for (const problem of config.problems ?? [])
            notice({
                id: `config-${workspace}`,
                title: "Couldn't read the workspace config",
                message: problem,
                variant: "error",
                duration: 15_000,
            });
        for (const warning of config.warnings ?? [])
            notice({
                id: `config-warning-${workspace}-${warning}`,
                title: "Part of the workspace config is ignored",
                message: warning,
                variant: "warning",
                duration: 10_000,
            });
        // A broken agent refuses only its own turns; name it so the first refusal is no surprise.
        for (const agent of agents.filter((agent) => agent.problem))
            notice({
                id: `agent-${workspace}-${agent.name}`,
                title: `The ${agent.name} agent can't run`,
                message: agent.problem!,
                variant: "warning",
                duration: 15_000,
            });
        set("agents", agents);
        set("commands", commands);
    }

    async function refreshMcp() {
        const servers = await requireClient().mcpServers();
        set("mcpServers", reconcile(Object.fromEntries(servers.map((server) => [server.name, server]))));
    }

    /** Runs one MCP change on the engine and records the state it reports back. */
    async function mcpChange(change: () => Promise<McpServerStatus>) {
        const server = await change();
        set("mcpServers", server.name, reconcile(server));
        return server;
    }

    /** A workspace folder's "always" grants; a folder the engine has no workspace for has none. */
    async function workspaceGrants(directory: string): Promise<PermissionGrant[]> {
        const id = workspaces().id(directory);
        return id ? requireClient().permissionGrants(id) : [];
    }

    /** Takes back one grant, or every grant of the folder's workspace when `grant` is left out. */
    async function revokeGrant(directory: string, grant?: PermissionGrant) {
        const id = workspaces().id(directory);
        if (!id) return;
        await (grant ? requireClient().revokePermissionGrant(id, grant) : requireClient().revokePermissionGrants(id));
    }

    /**
     * `create`: adding a server, which the engine refuses rather than replace one of the same name; left out,
     * `readOnlyTrusted` is the engine's default.
     */
    function mcpSave(
        name: string,
        config: McpServerConfig,
        options: { create?: boolean; readOnlyTrusted?: boolean; directory?: string } = {},
    ) {
        const { directory, ...rest } = options;
        return mcpChange(() =>
            requireClient().saveMcpServer(name, config, { ...rest, workspace: workspaceOf(directory) }),
        );
    }

    /** The engine's id for the folder a stdio server should connect in, if it knows the folder. */
    function workspaceOf(directory?: string) {
        return directory ? workspaces().id(directory) : undefined;
    }

    /** The engine renames in one step and refuses a name already taken, so no other server is ever replaced. */
    async function mcpRename(from: string, to: string, directory?: string) {
        const renamed = await requireClient().renameMcpServer(from, to, workspaceOf(directory));
        set(
            "mcpServers",
            produce((servers) => void delete servers[from]),
        );
        set("mcpServers", renamed.name, reconcile(renamed));
        return renamed;
    }

    async function mcpRemove(name: string) {
        await requireClient().removeMcpServer(name);
        set(
            "mcpServers",
            produce((servers) => void delete servers[name]),
        );
    }

    return {
        engineSettings,
        setBackgroundTaskLimit,
        putEngineSettings: (body: Parameters<Client["putSettings"]>[0]) => requireClient().putSettings(body),
        fetchRegistry: (source: string) => requireClient().fetchRegistry(source),
        setAutoCompact,
        setAutoAccept,
        setAutoAcceptAll,
        refreshEngineSettings,
        refreshAgents,
        basePrompts: () => requireClient().basePrompts(),
        /** Tool names an agent can be limited to: the built-ins and the folder's workspace MCP tools. */
        toolNames: (directory?: string) => requireClient().tools(directory ? workspaces().id(directory) : undefined),
        plugins: () => requireClient().plugins(),
        reloadPlugins: () => requireClient().reloadPlugins(),
        setPluginEnabled: (path: string, enabled: boolean) => requireClient().setPluginEnabled(path, enabled),
        installPlugin: (body: { id: string; url: string; sha256: string; config: unknown; registry?: string }) =>
            requireClient().installPlugin(body),
        removePlugin: (path: string) => requireClient().removePlugin(path),
        configurePlugin: (path: string, config: unknown) => requireClient().configurePlugin(path, config),
        skills: (directory?: string) => requireClient().skills(directory ? workspaces().id(directory) : undefined),
        setSkillEnabled: (path: string, enabled: boolean, directory?: string) =>
            requireClient().setSkillEnabled(path, enabled, directory ? workspaces().id(directory) : undefined),
        skillPacks: () => requireClient().skillPacks(),
        installSkillPack: (body: {
            id: string;
            name: string;
            archive: string;
            subdirs: string[];
            source?: string;
            image?: string;
            skills: string[];
            registry?: string;
        }) => requireClient().installSkillPack(body),
        removeSkillPack: (id: string) => requireClient().removeSkillPack(id),
        saveBasePrompt: (id: string, text: string) => requireClient().saveBasePrompt(id, text),
        resetBasePrompt: (id: string) => requireClient().resetBasePrompt(id),
        permissionRules: () => requireClient().permissionRules(),
        savePermissionRules: (rules: PermissionRule[]) => requireClient().savePermissionRules(rules),
        workspaceGrants,
        revokeGrant,
        refreshMcp,
        mcpSave,
        mcpRename,
        mcpRemove,
        mcpSetEnabled: (name: string, enabled: boolean, directory?: string) =>
            mcpChange(() => requireClient().setMcpServerEnabled(name, enabled, workspaceOf(directory))),
        mcpConnect: (name: string, directory?: string) =>
            mcpChange(() => requireClient().connectMcpServer(name, workspaceOf(directory))),
        mcpDisconnect: (name: string, directory?: string) =>
            mcpChange(() => requireClient().disconnectMcpServer(name, workspaceOf(directory))),
        /** The page to open in the browser; the server connects by itself once the user comes back. */
        mcpSignIn: async (name: string) => (await requireClient().signInMcpServer(name)).url,
        mcpSignOut: (name: string) => mcpChange(() => requireClient().signOutMcpServer(name)),
    };
}
