import { expect, test } from "bun:test";

if (!("localStorage" in globalThis))
    Object.defineProperty(globalThis, "localStorage", {
        value: { getItem: () => null, setItem: () => undefined },
    });

import { appendDictation, cleanTranscript, formatDictationElapsed } from "../src/voice/transcript";
import { captureConstraints, deviceUnavailable, stopStreamTracks } from "../src/voice/capture";
import { downloadPercent, formatBytes } from "../src/voice/models";
import {
    blockEnergy,
    blockSamples,
    createSegmenter,
    defaultSegmenterConfig,
    drainSegmenter,
    encodePcm16,
    phraseSeconds,
    pushBlock,
} from "../src/voice/audio";
import {
    availableCaptureDeviceId,
    enumerateAudioInputs,
    filterAudioInputDevices,
    watchAudioInputDevices,
} from "../src/voice/devices";

const silence = () => new Float32Array(blockSamples);
const speech = () => new Float32Array(blockSamples).fill(0.3);

function feed(state: ReturnType<typeof createSegmenter>, block: () => Float32Array, count: number) {
    const phrases: Float32Array[] = [];
    for (let index = 0; index < count; index += 1) {
        const phrase = pushBlock(state, block());
        if (phrase) phrases.push(phrase);
    }
    return phrases;
}

test("dictated segments join the draft without disturbing existing text", () => {
    expect(appendDictation("", "Refactor the store")).toBe("Refactor the store");
    expect(appendDictation("Refactor the store", "then run the tests")).toBe("Refactor the store then run the tests");
    expect(appendDictation("Refactor the store", ", then stop")).toBe("Refactor the store, then stop");
    expect(appendDictation("Check this out ", "now")).toBe("Check this out now");
    expect(appendDictation("First line\n", "Second line")).toBe("First line\nSecond line");
    expect(appendDictation("Call foo(", "bar")).toBe("Call foo(bar");
    expect(appendDictation("Keep me", "   ")).toBe("Keep me");
});

test("the recording timer counts past a minute", () => {
    expect(formatDictationElapsed(0)).toBe("0:00");
    expect(formatDictationElapsed(7_400)).toBe("0:07");
    expect(formatDictationElapsed(61_000)).toBe("1:01");
    expect(formatDictationElapsed(-500)).toBe("0:00");
});

test("non-speech markers never reach the draft", () => {
    expect(cleanTranscript("[BLANK_AUDIO]")).toBe("");
    expect(cleanTranscript("  (upbeat music) ")).toBe("");
    expect(cleanTranscript("[ Silence ]")).toBe("");
    expect(cleanTranscript("Refactor the store\n[BLANK_AUDIO]")).toBe("Refactor the store");
    expect(cleanTranscript("First segment\nsecond segment")).toBe("First segment second segment");
    expect(cleanTranscript("open paren (like this) close")).toBe("open paren (like this) close");
    expect(cleanTranscript("   ")).toBe("");
});

test("silence alone never produces a phrase", () => {
    const state = createSegmenter();
    expect(feed(state, silence, 200)).toHaveLength(0);
    expect(state.speaking).toBeFalse();
    expect(state.blocks.length).toBeLessThanOrEqual(defaultSegmenterConfig.prerollBlocks + 1);
});

test("a brief knock is not treated as speech", () => {
    const state = createSegmenter();
    feed(state, speech, defaultSegmenterConfig.minVoicedBlocks - 1);
    expect(state.speaking).toBeFalse();
    expect(feed(state, silence, 60)).toHaveLength(0);
});

test("a spoken phrase is emitted once the speaker pauses", () => {
    const state = createSegmenter();
    expect(feed(state, speech, 20)).toHaveLength(0);
    expect(state.speaking).toBeTrue();
    const phrases = feed(state, silence, defaultSegmenterConfig.hangoverBlocks);
    expect(phrases).toHaveLength(1);
    expect(phraseSeconds(phrases[0]!)).toBeCloseTo((20 * blockSamples) / 16_000, 3);
    expect(state.speaking).toBeFalse();
    expect(state.blocks).toHaveLength(0);
});

test("a long monologue is split rather than buffered forever", () => {
    const state = createSegmenter();
    const phrases = feed(state, speech, defaultSegmenterConfig.maxBlocks + 1);
    expect(phrases.length).toBeGreaterThan(0);
});

test("stopping mid-phrase still transcribes what was said", () => {
    const state = createSegmenter();
    feed(state, speech, 20);
    const tail = drainSegmenter(state);
    expect(tail).toBeDefined();
    expect(state.speaking).toBeFalse();
    expect(drainSegmenter(createSegmenter())).toBeUndefined();
});

test("block energy separates speech from silence", () => {
    expect(blockEnergy(silence())).toBe(0);
    expect(blockEnergy(speech())).toBeCloseTo(0.3, 5);
    expect(blockEnergy(new Float32Array(0))).toBe(0);
    expect(blockEnergy(speech())).toBeGreaterThan(defaultSegmenterConfig.threshold);
});

test("audio is encoded as little-endian 16-bit mono", () => {
    const encoded = encodePcm16(Float32Array.from([0, 1, -1, 0.5]));
    const bytes = Uint8Array.from(atob(encoded), (character) => character.charCodeAt(0));
    expect(bytes).toHaveLength(8);
    const view = new DataView(bytes.buffer);
    expect(view.getInt16(0, true)).toBe(0);
    expect(view.getInt16(2, true)).toBe(32767);
    expect(view.getInt16(4, true)).toBe(-32767);
    expect(view.getInt16(6, true)).toBe(16384);
});

test("clipping cannot overflow the sample range", () => {
    const bytes = Uint8Array.from(atob(encodePcm16(Float32Array.from([9, -9]))), (c) => c.charCodeAt(0));
    const view = new DataView(bytes.buffer);
    expect(view.getInt16(0, true)).toBe(32767);
    expect(view.getInt16(2, true)).toBe(-32767);
});

test("download progress is reported in human units", () => {
    expect(formatBytes(573_571_072)).toBe("547 MB");
    expect(formatBytes(59_768_832)).toBe("57 MB");
    expect(formatBytes(2 * 1024 ** 3)).toBe("2.0 GB");
    expect(downloadPercent({ id: "x", received: 50, total: 200 })).toBe(25);
    expect(downloadPercent({ id: "x", received: 10, total: 0 })).toBe(0);
    expect(downloadPercent(null)).toBe(0);
});

test("dictation is off until it is turned on", async () => {
    const { dictationEnabled } = await import("../src/state/voice");
    expect(dictationEnabled()).toBeFalse();
});

test("stored voice preferences are repaired rather than trusted", async () => {
    const voice = await import("../src/state/voice");
    expect(voice.normalizeLanguage("fr")).toBe("fr");
    expect(voice.normalizeLanguage("klingon")).toBe("en");
    expect(voice.normalizeModel("base-q5_1")).toBe("base-q5_1");
    expect(voice.normalizeModel("../escape")).toBe("large-v3-turbo-q5_0");
    expect(voice.normalizeKeyterms(["ok", 7, null])).toEqual(["ok"]);
    expect(voice.normalizeKeyterms("nope")).toEqual([]);
    expect(voice.normalizeInputDeviceId(" microphone-id ")).toBe("microphone-id");
    expect(voice.normalizeInputDeviceId("default")).toBeNull();
    expect(voice.normalizeInputDeviceId(7)).toBeNull();
});

test("capture uses an exact explicit device and leaves system default unconstrained", () => {
    expect(captureConstraints()).toEqual({
        audio: { echoCancellation: true, noiseSuppression: true, autoGainControl: true },
    });
    expect(captureConstraints("headset")).toEqual({
        audio: {
            echoCancellation: true,
            noiseSuppression: true,
            autoGainControl: true,
            deviceId: { exact: "headset" },
        },
    });
    expect(deviceUnavailable(Object.assign(new Error(), { name: "NotFoundError" }))).toBeTrue();
    expect(deviceUnavailable(Object.assign(new Error(), { name: "OverconstrainedError" }))).toBeTrue();
    expect(deviceUnavailable(Object.assign(new Error(), { name: "NotAllowedError" }))).toBeFalse();
});

test("device enumeration keeps unique physical audio inputs", () => {
    const devices = filterAudioInputDevices([
        { kind: "audiooutput", deviceId: "speaker", label: "Speakers" },
        { kind: "audioinput", deviceId: "default", label: "Default" },
        { kind: "audioinput", deviceId: "mic-1", label: "Headset" },
        { kind: "audioinput", deviceId: "mic-1", label: "Duplicate" },
        { kind: "audioinput", deviceId: "mic-2", label: "" },
    ]);
    expect(devices).toEqual([
        { deviceId: "mic-1", label: "Headset" },
        { deviceId: "mic-2", label: "" },
    ]);
});

test("enumerating inputs does not open a microphone stream", async () => {
    let captures = 0;
    const media = {
        enumerateDevices: async () => [{ kind: "audioinput", deviceId: "mic", label: "Desk mic" }],
        getUserMedia: async () => {
            captures += 1;
        },
    } as unknown as MediaDevices;
    expect(await enumerateAudioInputs(media)).toEqual([{ deviceId: "mic", label: "Desk mic" }]);
    expect(captures).toBe(0);
});

test("device changes refresh settings and cleanup removes the listener", () => {
    let listener: (() => void) | undefined;
    let refreshes = 0;
    const media = {
        addEventListener: (name: string, handler: () => void) => {
            if (name === "devicechange") listener = handler;
        },
        removeEventListener: (_name: string, handler: () => void) => {
            if (listener === handler) listener = undefined;
        },
    } as unknown as MediaDevices;
    const cleanup = watchAudioInputDevices(media, () => refreshes++);
    expect(refreshes).toBe(1);
    listener?.();
    expect(refreshes).toBe(2);
    cleanup();
    expect(listener).toBeUndefined();
});

test("a missing saved device falls back without discarding its reconnection preference", () => {
    const saved = "usb-mic";
    expect(availableCaptureDeviceId(saved, [{ deviceId: "laptop", label: "Laptop" }])).toBeUndefined();
    expect(saved).toBe("usb-mic");
    expect(availableCaptureDeviceId(saved, [{ deviceId: saved, label: "USB microphone" }])).toBe(saved);
});

test("capture cleanup stops every stream track", () => {
    const stopped: string[] = [];
    stopStreamTracks({
        getTracks: () =>
            [{ stop: () => stopped.push("audio") }, { stop: () => stopped.push("other") }] as MediaStreamTrack[],
    });
    expect(stopped).toEqual(["audio", "other"]);
});

test("Windows privacy denials are classified as actionable permission errors", async () => {
    const { captureErrorKey } = await import("../src/voice/dictation");
    for (const name of ["NotAllowedError", "SecurityError"]) {
        expect(captureErrorKey(Object.assign(new Error("denied"), { name }))).toBe("drift.voice.error.permission");
    }
});

test("disabling dictation prevents a new capture", async () => {
    const voice = await import("../src/state/voice");
    const dictation = await import("../src/voice/dictation");
    let captures = 0;
    const previous = Object.getOwnPropertyDescriptor(globalThis, "navigator");
    Object.defineProperty(globalThis, "navigator", {
        configurable: true,
        value: { mediaDevices: { getUserMedia: async () => (captures += 1) } },
    });
    try {
        voice.persistDictationEnabled(true);
        await dictation.setDictationEnabled(false);
        await dictation.startDictation(() => undefined);
        expect(captures).toBe(0);
        expect(voice.dictationEnabled()).toBeFalse();
    } finally {
        if (previous) Object.defineProperty(globalThis, "navigator", previous);
        else delete (globalThis as { navigator?: Navigator }).navigator;
    }
});

test("custom vocabulary becomes a whisper prompt", async () => {
    const { formatKeyterms, keytermPrompt, maxKeyterms, parseKeyterms } = await import("../src/state/voice");
    expect(parseKeyterms("Tauri, solidjs , SolidJS,, Tauri")).toEqual(["Tauri", "solidjs"]);
    expect(formatKeyterms(["a", "b"])).toBe("a, b");
    expect(keytermPrompt([])).toBe("");
    expect(keytermPrompt(["Tauri", "SolidJS"])).toBe("Vocabulary: Tauri, SolidJS.");
    expect(parseKeyterms(Array.from({ length: 80 }, (_, index) => `term${index}`).join(","))).toHaveLength(maxKeyterms);
});

test("dictation is gated behind the enable toggle and never calls a provider from the webview", async () => {
    const [composer, dictation, capture, readAloud] = await Promise.all([
        Bun.file("src/ui/composer.tsx").text(),
        Bun.file("src/voice/dictation.ts").text(),
        Bun.file("src/voice/capture.ts").text(),
        Bun.file("src/voice/read-aloud.ts").text(),
    ]);
    expect(composer).toContain("<Show when={dictationEnabled()}>");
    expect(composer).toContain("modelInstalled(dictationModel())");
    expect(composer).toContain("stopDictation()");
    expect(dictation).toContain('<string>("voice_transcribe"');
    expect(capture).toContain("sampleRate");
    for (const source of [composer, dictation, capture, readAloud]) {
        expect(source).not.toContain("fetch(");
        expect(source).not.toContain("apiKey");
        expect(source).not.toMatch(/api\.(openai|groq|deepgram|elevenlabs|assemblyai)/);
    }
});

test("cloud voices and read aloud are off until picked", async () => {
    const voice = await import("../src/state/voice");
    expect(voice.dictationProvider()).toBe("local");
    expect(voice.readAloudEnabled()).toBeFalse();
    expect(voice.readAloudAuto()).toBeFalse();
    expect(voice.speechProvider()).toBe("local");
    expect(voice.speechVoices()).toEqual({});
    expect(voice.normalizeChoice(voice.dictationProviders, "deepgram")).toBe("deepgram");
    expect(voice.normalizeChoice(voice.dictationProviders, "acme")).toBe("local");
    expect(voice.normalizeChoice(voice.speechProviders, "groq")).toBe("local");
    expect(voice.normalizeVoices({ openai: " coral ", elevenlabs: 7, acme: "x" })).toEqual({ openai: "coral" });
    expect(voice.normalizeVoices("nope")).toEqual({});
});

const labels = { code: "Code block", table: "A table" };

test("read aloud speaks prose and names code blocks, tables and files", async () => {
    const { speakableText } = await import("../src/voice/speakable");
    const reply = [
        "## Done",
        "",
        "I changed `src/ui/composer.tsx` and **fixed** the [docs](https://example.com/a) at crates/drift-engine/src/voice/mod.rs:42.",
        "",
        "```ts",
        "const secret = 1;",
        "```",
        "",
        "| a | b |",
        "|---|---|",
        "| 1 | 2 |",
        "",
        "- Kept `snake_case_name` as it was",
        "- See https://github.com/foo/bar",
        "",
        "> A quoted note",
        "",
        "---",
        "Ready to merge!",
    ].join("\n");
    expect(speakableText(reply, labels).split("\n\n")).toEqual([
        "Done.",
        "I changed composer.tsx and fixed the docs at mod.rs.",
        "Code block.",
        "A table.",
        "Kept snake_case_name as it was.",
        "See github.com.",
        "A quoted note.",
        "Ready to merge!",
    ]);
});

test("a fence opened with tildes or more backticks hides everything until it closes", async () => {
    const { speakableText } = await import("../src/voice/speakable");
    const reply = "Before\n````md\n```\nnot this\n```\n````\n~~~\nnor this\n~~~\nAfter";
    expect(speakableText(reply, labels)).toBe("Before.\n\nCode block.\n\nCode block.\n\nAfter.");
    expect(speakableText("```\nonly code\n```", labels)).toBe("Code block.");
    expect(speakableText("", labels)).toBe("");
});

test("long speech is cut at paragraphs, then sentences, then words", async () => {
    const { speechChunks } = await import("../src/voice/speakable");
    expect(speechChunks("One.\n\nTwo.", 100)).toEqual(["One.\n\nTwo."]);
    expect(speechChunks("First sentence here. Second sentence here.", 25)).toEqual([
        "First sentence here.",
        "Second sentence here.",
    ]);
    const words = speechChunks("word ".repeat(40).trim(), 50);
    expect(words.every((chunk) => chunk.length <= 50)).toBeTrue();
    expect(words.join(" ")).toBe("word ".repeat(40).trim());
});

test("the local voice never uses an online system voice", async () => {
    const { localVoices } = await import("../src/voice/read-aloud");
    const synth = {
        getVoices: () => [
            { name: "Microsoft David", localService: true },
            { name: "Microsoft Aria Online (Natural)", localService: false },
        ],
    } as unknown as SpeechSynthesis;
    expect(localVoices(synth).map((voice) => voice.name)).toEqual(["Microsoft David"]);
    expect(localVoices(undefined)).toEqual([]);
});

/** A microphone that hands the worklet's port to the test, so it can speak into it. */
function fakeMicrophone() {
    const ports: { onmessage: ((event: MessageEvent) => void) | null }[] = [];
    const graph = { connect: () => undefined, disconnect: () => undefined };
    const globals = {
        navigator: { mediaDevices: { getUserMedia: async () => ({ getTracks: () => [] }) } },
        AudioContext: class {
            state = "running";
            destination = {};
            audioWorklet = { addModule: async () => undefined };
            createMediaStreamSource = () => graph;
            close = async () => undefined;
        },
        AudioWorkletNode: class {
            port = { onmessage: null };
            connect = graph.connect;
            disconnect = graph.disconnect;
            constructor() {
                ports.push(this.port);
            }
        },
    };
    const previous = Object.keys(globals).map(
        (key) => [key, Object.getOwnPropertyDescriptor(globalThis, key)] as const,
    );
    for (const [key, value] of Object.entries(globals))
        Object.defineProperty(globalThis, key, { configurable: true, writable: true, value });

    return {
        speak: () => ports.at(-1)!.onmessage!({ data: new Float32Array(512 * 20).fill(0.3) } as MessageEvent),
        restore: () => {
            for (const [key, descriptor] of previous)
                if (descriptor) Object.defineProperty(globalThis, key, descriptor);
                else delete (globalThis as Record<string, unknown>)[key];
        },
    };
}

test("a failed cloud phrase is kept for a retry and never sent anywhere else", async () => {
    const voice = await import("../src/state/voice");
    const dictation = await import("../src/voice/dictation");
    const microphone = fakeMicrophone();
    const sent: string[] = [];
    const heard: string[] = [];
    let refuse = true;
    const transcriber = async (body: { provider: string }) => {
        sent.push(body.provider);
        if (refuse) throw new Error("Groq refused the key: invalid");
        return "Run the tests.";
    };
    try {
        voice.persistDictationEnabled(true);
        voice.setDictationProvider("groq");
        await dictation.startDictation((text) => heard.push(text), transcriber);
        microphone.speak();
        dictation.stopDictation();
        await Bun.sleep(20);
        expect(sent).toEqual(["groq"]);
        expect(dictation.dictationError()).toBe("Groq refused the key: invalid");
        expect(dictation.dictationKept()).toBe(1);
        expect(heard).toEqual([]);

        refuse = false;
        dictation.retryDictation();
        await Bun.sleep(20);
        expect(sent).toEqual(["groq", "groq"]);
        expect(heard).toEqual(["Run the tests."]);
        expect(dictation.dictationKept()).toBe(0);
    } finally {
        voice.setDictationProvider("local");
        voice.persistDictationEnabled(false);
        microphone.restore();
    }
});
