import { afterEach, beforeEach, expect, test } from "bun:test";

import type * as TranscriptFind from "../src/ui/transcript-find";

const globalNames = ["document", "NodeFilter", "Range", "Highlight", "CSS", "localStorage"] as const;
let originalGlobals: (PropertyDescriptor | undefined)[];
let find: typeof TranscriptFind;

class ElementDouble {
    open = false;
    dataset: Record<string, string | undefined> = {};
    scrollTop = 0;
    scrollLeft = 0;
    constructor(
        public tagName: string,
        public parentElement: ElementDouble | null = null,
        public ignored = false,
        public nodes: TextDouble[] = [],
    ) {}
    closest(selector: string): ElementDouble | null {
        for (let node: ElementDouble | null = this; node; node = node.parentElement) {
            if (selector === "[data-find-ignore]" && node.ignored) return node;
            if (selector === "[data-find-unit]" && node.dataset.findUnit !== undefined) return node;
            if (selector === "details:not([open])" && node.tagName === "details" && !node.open) return node;
        }
        return null;
    }
    querySelector() {
        return null;
    }
    contains(other: { parentElement?: ElementDouble | null }) {
        for (let node = other.parentElement ?? null; node; node = node.parentElement) if (node === this) return true;
        return false;
    }
    get scrollHeight() {
        return 0;
    }
    get clientHeight() {
        return 0;
    }
    get scrollWidth() {
        return 0;
    }
    get clientWidth() {
        return 0;
    }
}

type TextDouble = { nodeValue: string; parentElement: ElementDouble };
class RangeDouble {
    startContainer!: TextDouble;
    startOffset!: number;
    endContainer!: TextDouble;
    endOffset!: number;
    setStart(node: TextDouble, offset: number) {
        this.startContainer = node;
        this.startOffset = offset;
    }
    setEnd(node: TextDouble, offset: number) {
        this.endContainer = node;
        this.endOffset = offset;
    }
}

class HighlightDouble {
    constructor(...ranges: RangeDouble[]) {
        this.ranges = ranges;
    }
    ranges: RangeDouble[];
}

const registry = new Map<string, HighlightDouble>();
function unit(id: string, parent: ElementDouble | null = null) {
    const element = new ElementDouble("div", parent);
    element.dataset.findUnit = id;
    return element;
}
function text(nodeValue: string, parentElement: ElementDouble): TextDouble {
    const node = { nodeValue, parentElement };
    let owner: ElementDouble | null = parentElement;
    while (owner && owner.dataset.findUnit === undefined) owner = owner.parentElement;
    owner?.nodes.push(node);
    return node;
}
function container(...units: ElementDouble[]) {
    return {
        querySelectorAll: (selector: string) => {
            expect(selector).toBe("[data-find-unit]");
            return units;
        },
    } as unknown as HTMLElement;
}
const at = (id: string, index: number) => ({ rowId: id, unit: id, messageId: id, index });

beforeEach(async () => {
    originalGlobals = globalNames.map((name) => Object.getOwnPropertyDescriptor(globalThis, name));
    const set = (name: (typeof globalNames)[number], value: unknown) =>
        Object.defineProperty(globalThis, name, { configurable: true, writable: true, value });
    if (!("localStorage" in globalThis)) set("localStorage", { getItem: () => null, setItem: () => undefined });
    find = await import("../src/ui/transcript-find");
    registry.clear();
    set("CSS", { highlights: registry });
    set("Highlight", HighlightDouble);
    set("Range", RangeDouble);
    set("NodeFilter", { SHOW_TEXT: 4 });
    set("document", {
        createTreeWalker: (root: ElementDouble, whatToShow: number) => {
            expect(whatToShow).toBe(4);
            let index = 0;
            return { nextNode: () => root.nodes[index++] ?? null };
        },
    });
});

afterEach(() => {
    globalNames.forEach((name, index) => {
        const descriptor = originalGlobals[index];
        if (descriptor) Object.defineProperty(globalThis, name, descriptor);
        else Reflect.deleteProperty(globalThis, name);
    });
});

test("closed clarification highlights count only real QAs, not duplicate preview or empty-answer UI", () => {
    const row = unit("u1");
    const details = new ElementDouble("details", row);
    const summary = new ElementDouble("summary", details, true);
    text("Blue", summary);
    text("Blue, Blue", new ElementDouble("span", summary));
    const question = text("Blue or not Blue?", new ElementDouble("div", details));
    const answer = text("Blue, Blue", new ElementDouble("div", details));
    text("Blue", new ElementDouble("div", details, true));
    const root = container(row);
    for (let index = 0; index < 4; index++) {
        const painted = find.paintFindHighlights(root, "BLUE", at("u1", index));
        expect(registry.get("drift-find")!.ranges).toHaveLength(4);
        expect(painted.active!.startContainer).toBe(index < 2 ? question : answer);
        expect(painted.active!.startOffset).toBe([0, 12, 0, 6][index]);
        expect(painted.counts.get("u1")).toBe(4);
        expect(registry.get("drift-find-active")!.ranges).toEqual([painted.active!]);
        expect(details.open).toBe(false);
    }
});

test("a match split across markdown spans is one range, and occurrences number per unit", () => {
    const prose = unit("p1");
    const start = text("call cache_", new ElementDouble("code", prose));
    const end = text("key now", new ElementDouble("span", prose));
    const other = unit("p2");
    const plain = text("cache_key cache_key", new ElementDouble("div", other));
    const root = container(prose, other);
    const painted = find.paintFindHighlights(root, "cache_key", at("p1", 0));
    expect(painted.active!.startContainer).toBe(start);
    expect(painted.active!.startOffset).toBe(5);
    expect(painted.active!.endContainer).toBe(end);
    expect(painted.active!.endOffset).toBe(3);
    expect(registry.get("drift-find")!.ranges).toHaveLength(3);
    expect(find.paintFindHighlights(root, "cache_key", at("p2", 1)).active!.startContainer).toBe(plain);
    expect(painted.counts).toEqual(
        new Map([
            ["p1", 1],
            ["p2", 2],
        ]),
    );
});

test("a folded row is counted only when the cursor is on it, and a cursor past what it shows lands on its last", () => {
    const folded = unit("t1");
    folded.dataset.findOpen = "false";
    text("needle in the header", new ElementDouble("button", folded));
    const root = container(folded);
    expect(find.paintFindHighlights(root, "needle", at("other", 0)).counts.has("t1")).toBe(false);
    const painted = find.paintFindHighlights(root, "needle", at("t1", 3));
    expect(painted.counts.get("t1")).toBe(1);
    expect(painted.active!.startOffset).toBe(0);
    expect(find.paintFindHighlights(root, "needle", at("missing", 0)).active).toBeUndefined();
    expect(registry.has("drift-find-active")).toBe(false);
    expect(find.paintFindHighlights(root, "", undefined).counts.size).toBe(0);
    expect(registry.size).toBe(0);
});

test("revealing opens the disclosures around a match and closes them once the cursor moves out", () => {
    const row = unit("u1");
    const outer = new ElementDouble("details", row);
    const inner = new ElementDouble("details", outer);
    const first = text("Blue", new ElementDouble("div", inner));
    const elsewhere = text("Blue", new ElementDouble("div", row));
    const scroller = new ElementDouble("div") as unknown as HTMLElement;
    const range = (node: TextDouble) =>
        Object.assign(new RangeDouble(), {
            startContainer: node,
            getBoundingClientRect: () => ({ top: 0, height: 10, left: 0, bottom: 10 }),
        }) as unknown as Range;
    Object.assign(scroller, { getBoundingClientRect: () => ({ top: 0, left: 0, bottom: 100 }) });
    find.revealFindRange(range(first), scroller, 0);
    expect([outer.open, inner.open]).toEqual([true, true]);
    find.revealFindRange(range(elsewhere), scroller, 0);
    expect([outer.open, inner.open]).toEqual([false, false]);
});

test("the timeline follows the occurrence itself, repainting as rows redraw, and stops pulling to the row once there", async () => {
    const source = await Bun.file("src/ui/chat.tsx").text();
    expect(source).toContain("syncTranscriptMatches(timeline(), partsOfRow, rowAtViewTop)");
    expect(source).toContain("new MutationObserver(");
    expect(source).toContain("if (!row || (target && landed() === target.key)) return;");
    expect(source).toContain("revealFindRange(painted.active, scroller, inset)");
    expect(source).toContain("reconcileFindCounts(drawnCounts)");
});

test("a query that has settled is not run again by the next Enter, which would reset the counts instead of stepping", async () => {
    const source = await Bun.file("src/ui/transcript-find.tsx").text();
    expect(source).toMatch(/debounce = setTimeout\(\(\) => \{\s*debounce = undefined;\s*setNeedle\(value\);/);
});
