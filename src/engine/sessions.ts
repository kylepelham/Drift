import type { components } from "./native/types";

/** A native session with the workspace directory needed by file views. */
export type Session = components["schemas"]["Session"] & { directory: string };

/** The shell and engine share workspace IDs; views also need their directories. */
export type WorkspaceIndex = { path(id: string): string | undefined; id(path: string): string | undefined };

export function sessionInWorkspace(session: components["schemas"]["Session"], workspaces: WorkspaceIndex): Session {
    const directory = workspaces.path(session.workspaceId) ?? session.workspaceId;

    return { ...session, directory };
}

/** Points loaded sessions at their workspace's current directory, so a moved workspace keeps its threads. */
export function rehomeSessions(sessions: Record<string, Session>, workspaces: WorkspaceIndex) {
    for (const session of Object.values(sessions)) {
        const directory = workspaces.path(session.workspaceId);
        if (directory && directory !== session.directory) session.directory = directory;
    }
}

/** Only hidden workers nest under their parent; spawned siblings stay in the thread list. */
export function hiddenParent(session: Pick<Session, "visibility" | "parentId"> | undefined) {
    if (session?.visibility !== "hidden") return undefined;

    return session.parentId ?? undefined;
}
