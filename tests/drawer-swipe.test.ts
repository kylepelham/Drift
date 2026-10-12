import { swipeCloses } from "../src/ui/drawer-swipe";
import { expect, test } from "bun:test";

test("a drag closes the drawer past a third of its width or on a quick flick, never to the right", () => {
    expect(swipeCloses(-120, 300, -0.2)).toBeTrue();
    expect(swipeCloses(-60, 300, -0.2)).toBeFalse();
    expect(swipeCloses(-40, 300, -0.9)).toBeTrue();
    expect(swipeCloses(120, 300, 0.9)).toBeFalse();
});
