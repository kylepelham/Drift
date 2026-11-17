import { expect, test } from "bun:test";

test("a group that forms around a call the user opened opens too, and keeps its own toggle after that", async () => {
    const { groupStartsOpen, rememberGroupOpen, rememberToolOpen } = await import("../src/ui/tool-view");
    const part = (id: string) => ({ id });

    expect(groupStartsOpen([part("g-a"), part("g-b")])).toBeFalse();
    rememberToolOpen("g-a", true);
    expect(groupStartsOpen([part("g-a"), part("g-b")]), "the opened shell now has company").toBeTrue();

    rememberGroupOpen([part("g-a"), part("g-b")], false);
    expect(groupStartsOpen([part("g-a"), part("g-b"), part("g-c")]), "closed by hand, it stays closed").toBeFalse();
    rememberGroupOpen([part("g-a"), part("g-b"), part("g-c")], true);
    expect(groupStartsOpen([part("g-a"), part("g-b"), part("g-c"), part("g-d")])).toBeTrue();
});

test("changing the grouping setting regroups everything closed, as before", async () => {
    const { groupStartsOpen, rememberToolOpen } = await import("../src/ui/tool-view");
    const { setToolGroup, toolGroups } = await import("../src/state/prefs");

    rememberToolOpen("s-a", true);
    setToolGroup("commands", toolGroups().commands);
    expect(groupStartsOpen([{ id: "s-a" }, { id: "s-b" }])).toBeFalse();
    expect(groupStartsOpen([{ id: "s-a" }, { id: "s-b" }], Date.now() + 2000)).toBeTrue();
});
