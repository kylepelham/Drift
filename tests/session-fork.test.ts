import { createSignal } from "solid-js/dist/solid.js";
import { expect, mock, test } from "bun:test";
import * as ts from "typescript";

const source = await Bun.file(new URL("../src/ui/workspaces.tsx", import.meta.url)).text();
const parsed = ts.createSourceFile("workspaces.tsx", source, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
// The row and the fork helper it shares with the thread menu, with what they import supplied below.
const named = (node: ts.Statement, name: string) =>
    (ts.isFunctionDeclaration(node) && node.name?.text === name) ||
    (ts.isVariableStatement(node) &&
        node.declarationList.declarations.some((item) => ts.isIdentifier(item.name) && item.name.text === name));
const wanted = ["forksUnderWay", "forkThread", "ThreadItem"];
const pieces = parsed.statements.filter((node) => wanted.some((name) => named(node, name)));
const compiled = ts.transpileModule(pieces.map((node) => node.getText(parsed).replace(/^export /, "")).join("\n"), {
    compilerOptions: { target: ts.ScriptTarget.ES2022, jsx: ts.JsxEmit.React, jsxFactory: "jsx" },
    fileName: "workspaces.tsx",
}).outputText;

type Node = { type: string; props: { onClick?: () => void }; children: Node[] };

function setup() {
    let selection = "source";
    let workspace = "workspace";
    let finish!: (session: { id: string } | undefined) => void;
    const pending = new Promise<{ id: string } | undefined>((resolve) => {
        finish = resolve;
    });
    const fork = mock(() => pending);
    const selectSession = mock((id: string) => {
        selection = id;
    });
    const dependencies = {
        createSignal,
        useEngine: () => ({ actions: { fork } }),
        selectedSession: () => selection,
        activeWorkspaceId: () => workspace,
        selectWorkspace: (id: string) => {
            workspace = id;
        },
        selectSession,
        t: (key: string) => key,
        ago: () => "now",
        RowButton: "button",
        StatusDot: "status",
        IconBranch: "branch",
        IconArchive: "archive",
        IconDots: "dots",
        jsx: (type: string, props: Node["props"], ...children: Node[]): Node => ({ type, props, children }),
    };
    const render = new Function(
        ...Object.keys(dependencies),
        `${compiled}\nreturn ThreadItem({ sessionId: "source", title: "Long session", updated: 0, workspace: { id: "workspace" } });`,
    );
    const tree = render(...Object.values(dependencies)) as Node;

    const buttons = (node: Node): Node[] => {
        if (!node || typeof node !== "object") return [];
        return [...(node.type === "button" ? [node] : []), ...node.children.flatMap(buttons)];
    };

    return {
        fork,
        selectSession,
        finish,
        click: buttons(tree)[0].props.onClick!,
        navigate: (id: string) => {
            selection = id;
        },
    };
}

test("sidebar forks all finished history and ignores repeat clicks until the copy finishes", async () => {
    const view = setup();
    view.click();
    view.click();
    expect(view.fork).toHaveBeenCalledTimes(1);
    expect(view.fork).toHaveBeenCalledWith("source");
    view.finish({ id: "forked" });
    await new Promise<void>((resolve) => setTimeout(resolve, 0));
    expect(view.selectSession).toHaveBeenCalledWith("forked");
    view.click();
    expect(view.fork).toHaveBeenCalledTimes(2);
});

test("sidebar fork completion preserves navigation made while copying", async () => {
    const view = setup();
    view.click();
    view.navigate("another-session");
    view.finish({ id: "forked" });
    await new Promise<void>((resolve) => setTimeout(resolve, 0));
    expect(view.selectSession).not.toHaveBeenCalled();
});

test("a failed sidebar fork can be retried", async () => {
    const view = setup();
    view.click();
    view.finish(undefined);
    await new Promise<void>((resolve) => setTimeout(resolve, 0));
    expect(view.selectSession).not.toHaveBeenCalled();
    view.click();
    expect(view.fork).toHaveBeenCalledTimes(2);
});
