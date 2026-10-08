import { expect, test } from "bun:test";

test("byte sizes are formatted to the nearest sensible unit", async () => {
    const { formatBytes } = await import("../src/state/storage");
    expect(formatBytes(0)).toBe("0 B");
    expect(formatBytes(-1)).toBe("0 B");
    expect(formatBytes(512)).toBe("512 B");
    expect(formatBytes(1024)).toBe("1.0 KB");
    expect(formatBytes(1024 * 1024 * 1.5)).toBe("1.5 MB");
    expect(formatBytes(1024 ** 3 * 11.7)).toBe("11.7 GB");
    // Three-digit values drop the decimal so the column stays narrow.
    expect(formatBytes(1024 ** 2 * 512)).toBe("512 MB");
});

test("without the desktop backend each action says so instead of failing silently", async () => {
    const storage = await import("../src/state/storage");
    expect(await storage.pruneStorage()).toBeUndefined();
    expect(storage.storageError()).toBe("Storage management needs the Drift host backend");
    expect(storage.storageBusy()).toBeNull();
});
