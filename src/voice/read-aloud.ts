import { readAloudAuto, readAloudEnabled, speechProvider, speechVoices, type SpeechProvider } from "../state/voice";
import { speakableText, speechChunks } from "./speakable";
import { createSignal } from "solid-js";
import { t } from "../state/i18n";

type SpokenAudio = { audio: string; mime: string };
/** Speaks one piece of text with a cloud provider, through the engine. */
export type Speaker = (body: { provider: string; text: string; voice?: string | null }) => Promise<SpokenAudio>;

/** Below OpenAI's 4096-character limit, and short enough that the first piece starts playing soon. */
const maxChunkChars = 2500;

const [reading, setReading] = createSignal<string | null>(null);

/** The id of the reply being read aloud, if any. */
export const readingReply = reading;

let run = 0;
let player: HTMLAudioElement | undefined;

/** Stops playback: a new prompt, Stop, switching conversations or the reply's own button. */
export function stopReadAloud() {
    run += 1;
    const playing = player;
    player = undefined;
    playing?.pause();
    globalThis.speechSynthesis?.cancel();
    setReading(null);
}

/** The system voices that speak without a network; online voices would send the text away. */
export function localVoices(synth: SpeechSynthesis | undefined = globalThis.speechSynthesis) {
    return synth?.getVoices().filter((voice) => voice.localService) ?? [];
}

const [voices, setVoices] = createSignal<SpeechSynthesisVoice[]>([]);
export const systemVoices = voices;

/** Keeps `systemVoices` current; the list fills in after the page loads. */
export function watchLocalVoices(synth: SpeechSynthesis | undefined = globalThis.speechSynthesis) {
    const refresh = () => setVoices(localVoices(synth));
    refresh();
    synth?.addEventListener?.("voiceschanged", refresh);

    return () => synth?.removeEventListener?.("voiceschanged", refresh);
}

async function voicesLoaded(synth: SpeechSynthesis) {
    if (localVoices(synth).length) return;
    await new Promise<void>((resolve) => {
        const done = () => {
            synth.removeEventListener("voiceschanged", done);
            resolve();
        };
        synth.addEventListener("voiceschanged", done);
        setTimeout(done, 1500);
    });
}

/** Reads one reply's prose aloud in place of anything playing; rejects with the reason to show. */
async function readAloud(id: string, markdown: string, speak: Speaker) {
    stopReadAloud();
    const text = speakableText(markdown, { code: t("drift.voice.read.code"), table: t("drift.voice.read.table") });
    if (!text) throw new Error(t("drift.voice.read.empty"));

    const current = run;
    const provider = speechProvider();
    setReading(id);
    try {
        const chunks = speechChunks(text, maxChunkChars);
        if (provider === "local") await speakLocally(chunks, current);
        else await playCloud(chunks, provider, speak, current);
    } catch (cause) {
        if (current === run) throw cause;
    } finally {
        if (current === run) setReading(null);
    }
}

export async function toggleReadAloud(id: string, markdown: string, speak: Speaker) {
    if (reading() === id) return stopReadAloud();
    await readAloud(id, markdown, speak);
}

/** A reply that just finished in the open conversation, read when the auto-read switch is on. */
export async function autoReadReply(id: string, markdown: string, speak: Speaker) {
    if (!readAloudEnabled() || !readAloudAuto()) return;
    await readAloud(id, markdown, speak);
}

async function speakLocally(chunks: string[], current: number) {
    const synth = globalThis.speechSynthesis;
    if (synth) await voicesLoaded(synth);
    if (current !== run) return;
    const voices = localVoices(synth);
    if (!synth || !voices.length) throw new Error(t("drift.voice.read.noLocalVoice"));

    const saved = speechVoices().local;
    const voice = voices.find((item) => item.voiceURI === saved) ?? voices.find((item) => item.default) ?? voices[0]!;

    return new Promise<void>((resolve, reject) => {
        chunks.forEach((chunk, index) => {
            const utterance = new SpeechSynthesisUtterance(chunk);
            utterance.voice = voice;
            utterance.lang = voice.lang;
            if (index === chunks.length - 1) utterance.onend = () => resolve();
            utterance.onerror = (event) =>
                ["canceled", "interrupted"].includes(event.error) ? resolve() : reject(new Error(event.error));
            synth.speak(utterance);
        });
    });
}

// The next piece is fetched while the current one plays, so long replies have no gaps.
async function playCloud(
    chunks: string[],
    provider: Exclude<SpeechProvider, "local">,
    speak: Speaker,
    current: number,
) {
    const voice = speechVoices()[provider] ?? null;
    let next = speak({ provider, text: chunks[0]!, voice });
    for (let index = 0; index < chunks.length; index += 1) {
        const spoken = await next;
        if (current !== run) return;
        if (index + 1 < chunks.length) {
            next = speak({ provider, text: chunks[index + 1]!, voice });
            next.catch(() => undefined);
        }
        await play(spoken);
        if (current !== run) return;
    }
}

function play(spoken: SpokenAudio) {
    return new Promise<void>((resolve, reject) => {
        const audio = new Audio(`data:${spoken.mime};base64,${spoken.audio}`);
        player = audio;
        audio.onended = () => resolve();
        audio.onpause = () => resolve();
        audio.onerror = () => reject(new Error(t("drift.voice.read.playback")));
        audio.play().catch(reject);
    });
}
