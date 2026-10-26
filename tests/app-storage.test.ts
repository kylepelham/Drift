import { keepAppStorage } from "../src/app-storage";
import { expect, test } from "bun:test";

class FakeStorage {
    values = new Map<string, string>();
    get length() {
        return this.values.size;
    }
    key(index: number) {
        return [...this.values.keys()][index] ?? null;
    }
    getItem(key: string) {
        return this.values.get(key) ?? null;
    }
    setItem(key: string, value: string) {
        this.values.set(key, value);
    }
    removeItem(key: string) {
        this.values.delete(key);
    }
    clear() {
        this.values.clear();
    }
}

test("the phone app is sent the whole saved set once a burst of changes settles or the app is left, and only for its own storage", async () => {
    const storage = new FakeStorage();
    const other = new FakeStorage();
    storage.setItem("drift.theme", '"drift-dim"');
    const sent: string[] = [];
    const page = Object.assign(new EventTarget(), { visibilityState: "visible" });
    Object.assign(globalThis, { document: page });
    keepAppStorage(
        storage as unknown as Storage,
        (body) => sent.push(body),
        FakeStorage.prototype as unknown as Storage,
    );

    storage.setItem("drift.workspace.collapsed", '["w1"]');
    storage.removeItem("drift.theme");
    other.setItem("ignored", "1");
    await Bun.sleep(400);

    expect(sent).toEqual([JSON.stringify({ "drift.workspace.collapsed": '["w1"]' })]);
    other.setItem("still", "ignored");
    await Bun.sleep(400);
    expect(sent).toHaveLength(1);

    storage.setItem("drift.diff.wrap", "true");
    page.visibilityState = "hidden";
    page.dispatchEvent(new Event("visibilitychange"));
    expect(sent).toHaveLength(2);
    Reflect.deleteProperty(globalThis, "document");
});
