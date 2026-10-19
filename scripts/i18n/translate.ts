// English interface strings into one locale, sent through a headless engine on the user's saved credentials.
import { mkdtempSync, rmSync } from "node:fs";
import path from "node:path";
import os from "node:os";

const root = path.resolve(import.meta.dirname, "../..");
const placeholder = /\{\{\w+\}\}/g;

/** The English to translate for one locale, and translations it already has that show its terms and tone. */
export type Batch = { locale: string; strings: Record<string, string>; examples: Record<string, [string, string]> };

/** A model the script can ask; `close` ends its engine. */
export type Model = { complete: (system: string, prompt: string) => Promise<string>; close: () => Promise<void> };

/**
 * Starts `drift-engined --complete` on a scratch data folder. Credentials come from the OS credential store
 * Drift saves to, then the provider's environment variable, so the key never passes through this script.
 */
export async function startModel(model: string): Promise<Model> {
    const built = Bun.spawnSync(["cargo", "build", "-q", "-p", "drift-engined"], { cwd: root, stderr: "inherit" });
    if (built.exitCode !== 0) throw new Error("drift-engined did not build");

    const data = mkdtempSync(path.join(os.tmpdir(), "drift-i18n-"));
    const binary = path.join(
        root,
        "target",
        "debug",
        process.platform === "win32" ? "drift-engined.exe" : "drift-engined",
    );
    const engine = Bun.spawn([binary, "--data-dir", data, "--complete", "--model", model], {
        stdin: "pipe",
        stdout: "pipe",
        stderr: "inherit",
    });
    const pending = new Map<string, { resolve: (text: string) => void; reject: (error: Error) => void }>();
    void readReplies(engine.stdout, pending);
    void engine.exited.then(() => {
        for (const waiting of pending.values()) waiting.reject(new Error("drift-engined exited before answering"));
    });

    let next = 0;
    const complete = (system: string, prompt: string) =>
        new Promise<string>((resolve, reject) => {
            const id = String(next++);
            pending.set(id, { resolve, reject });
            void engine.stdin.write(`${JSON.stringify({ id, system, prompt, maxTokens: 8192 })}\n`);
            void engine.stdin.flush();
        });
    const close = async () => {
        void engine.stdin.end();
        await engine.exited;
        rmSync(data, { recursive: true, force: true });
    };

    return { complete, close };
}

async function readReplies(
    stdout: ReadableStream<Uint8Array>,
    pending: Map<string, { resolve: (text: string) => void; reject: (error: Error) => void }>,
) {
    let buffered = "";
    const decoder = new TextDecoder();

    for await (const chunk of stdout) {
        buffered += decoder.decode(chunk, { stream: true });
        const lines = buffered.split("\n");
        buffered = lines.pop() ?? "";

        for (const line of lines.filter((line) => line.trim())) {
            const reply = JSON.parse(line) as { id: string; text?: string; error?: string };
            const waiting = pending.get(reply.id);
            pending.delete(reply.id);
            if (reply.text !== undefined) waiting?.resolve(reply.text);
            else waiting?.reject(new Error(reply.error ?? "no answer"));
        }
    }
}

function instructions(locale: string) {
    return [
        `You translate the interface of Drift, a desktop app for working with AI coding agents, from English into ${locale}.`,
        'You get JSON: "examples" maps keys to [English, existing translation], "translate" maps keys to English.',
        'Answer with only a JSON object mapping every key in "translate" to its translation.',
        "Keep every {{placeholder}} exactly as written. Keep product, protocol and file names, code, commands,",
        "keyboard keys (Alt+Enter, Ctrl) and units as they are.",
        "Write short, plain interface text in the terms and tone of the examples. The key says where the text appears.",
        "Never use an em dash.",
    ].join("\n");
}

/** Translations for the keys of `batch` that passed the checks; a key that failed twice is left out. */
export async function translate(model: Model, batch: Batch): Promise<Record<string, string>> {
    const kept = await ask(model, batch, batch.strings);
    const failed = Object.keys(batch.strings).filter((key) => !(key in kept));
    if (failed.length === 0) return kept;

    const again = Object.fromEntries(failed.map((key) => [key, batch.strings[key]!]));

    return { ...kept, ...(await ask(model, batch, again)) };
}

async function ask(model: Model, batch: Batch, strings: Record<string, string>) {
    const prompt = JSON.stringify({ examples: batch.examples, translate: strings });
    const answer = parseAnswer(await model.complete(instructions(batch.locale), prompt));

    return Object.fromEntries(
        Object.entries(strings).flatMap(([key, english]) => {
            const translated = checked(english, answer[key]);
            return translated === undefined ? [] : [[key, translated]];
        }),
    );
}

/** The translation when it can stand in for `english`: the same placeholders, and never empty when English is not. */
export function checked(english: string, translated: unknown): string | undefined {
    if (typeof translated !== "string") return undefined;
    if (english.trim() && !translated.trim()) return undefined;

    const wanted = (english.match(placeholder) ?? []).sort().join();
    const found = (translated.match(placeholder) ?? []).sort().join();
    if (wanted !== found) return undefined;

    // The repository bans em dashes, which some languages write by habit.
    return translated.replace(/\s*\u2014\s*/g, " - ");
}

/** The JSON object in a model's answer, which may come wrapped in a code fence or a sentence. */
export function parseAnswer(text: string): Record<string, unknown> {
    const start = text.indexOf("{");
    const end = text.lastIndexOf("}");
    if (start === -1 || end <= start) return {};

    try {
        const parsed: unknown = JSON.parse(text.slice(start, end + 1));
        const object = parsed && typeof parsed === "object" && !Array.isArray(parsed);
        return object ? (parsed as Record<string, unknown>) : {};
    } catch {
        return {};
    }
}
