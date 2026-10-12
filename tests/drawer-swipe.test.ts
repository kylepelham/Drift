import { swipeSettles } from "../src/ui/drawer-swipe";
import { expect, test } from "bun:test";

test("a drag carries the drawer the rest of the way past a third of its width or on a quick flick", () => {
    expect(swipeSettles(-120, 300, -0.2, true)).toBeTrue();
    expect(swipeSettles(-60, 300, -0.2, true)).toBeFalse();
    expect(swipeSettles(-40, 300, -0.9, true)).toBeTrue();

    expect(swipeSettles(120, 300, 0.2, false)).toBeTrue();
    expect(swipeSettles(60, 300, 0.2, false)).toBeFalse();
    expect(swipeSettles(40, 300, 0.9, false)).toBeTrue();
});

test("a drag the wrong way never moves the drawer", () => {
    expect(swipeSettles(120, 300, 0.9, true)).toBeFalse();
    expect(swipeSettles(-120, 300, -0.9, false)).toBeFalse();
});
