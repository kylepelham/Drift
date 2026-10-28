import { createDeltaBuffer, foldDeltas } from "../src/engine/deltas";
import { expect, test } from "bun:test";

import type { Envelope } from "../src/engine/native/client";

let seq = 0;
function delta(partId: string, offset: number, text: string) {
    return {
        seq: ++seq,
        type: "part.delta",
        sessionId: "s",
        messageId: "m",
        partId,
        offset,
        delta: text,
    } as Extract<Envelope, { type: "part.delta" }>;
}

test("a part's contiguous deltas fold into one that ends at the last one's seq", () => {
    const first = delta("p", 0, "ab");
    const last = delta("p", 2, "cd");
    const folded = foldDeltas([first, last]);

    expect(folded).toHaveLength(1);
    expect(folded[0]).toMatchObject({ partId: "p", offset: 0, delta: "abcd", seq: last.seq });
});

test("deltas with a gap, a rewind or another part between them stay apart", () => {
    expect(foldDeltas([delta("p", 0, "ab"), delta("p", 5, "cd")])).toHaveLength(2);
    expect(foldDeltas([delta("p", 0, "ab"), delta("p", 1, "x")])).toHaveLength(2);
    expect(foldDeltas([delta("p", 0, "ab"), delta("q", 0, "x"), delta("p", 2, "cd")])).toHaveLength(3);
});

test("the buffer holds deltas until its timer and applies them folded", async () => {
    const applied: Envelope[] = [];
    const buffer = createDeltaBuffer((envelope) => applied.push(envelope), 5);
    buffer.push(delta("p", 0, "ab"));
    buffer.push(delta("p", 2, "cd"));

    expect(applied).toHaveLength(0);
    await Bun.sleep(20);
    expect(applied.map((envelope) => (envelope.type === "part.delta" ? envelope.delta : ""))).toEqual(["abcd"]);
});

test("any other event flushes held deltas first, keeping the order", () => {
    const applied: string[] = [];
    const buffer = createDeltaBuffer((envelope) => applied.push(envelope.type), 10_000);
    buffer.push(delta("p", 0, "ab"));
    buffer.push({ seq: ++seq, type: "session.idle", sessionId: "s" } as unknown as Envelope);

    expect(applied).toEqual(["part.delta", "session.idle"]);
    buffer.flush();
    expect(applied).toEqual(["part.delta", "session.idle"]);
});
