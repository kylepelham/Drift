// The existing translations shown beside a batch, so a new string reads like the rest of its locale.

type Strings = Record<string, string>;
type Examples = Record<string, [string, string]>;

/** The same English already translated elsewhere, per string. */
const samePerString = 2;
/** Keys under the same parent: all of them, padded with the wider area's up to `widerFill`. */
const neighbourCount = 16;
const widerFill = 8;
/** Strings sharing a word with the batch: the shortest, as they show the term most plainly. */
const termCount = 24;
const termsPerWord = 2;
/** Words too common to name anything; a term worth keeping consistent is never one of these. */
const common = new Set([
    "about",
    "after",
    "also",
    "been",
    "before",
    "each",
    "from",
    "have",
    "into",
    "more",
    "only",
    "some",
    "than",
    "that",
    "their",
    "them",
    "then",
    "there",
    "they",
    "this",
    "what",
    "when",
    "where",
    "which",
    "while",
    "will",
    "with",
    "your",
]);

/** For one locale, the translations to show beside `keys`: the same English, the same place, then the same words. */
export function examplesFor(english: Strings, translated: Strings, keys: string[]): Examples {
    const wanted = new Set(keys);
    const known = Object.keys(english).filter((key) => key in translated && !wanted.has(key));
    const picked = [
        ...new Set([...same(english, known, keys), ...neighbours(known, keys), ...terms(english, known, keys)]),
    ];

    return Object.fromEntries(picked.map((key) => [key, [english[key]!, translated[key]!]]));
}

/** Strings elsewhere whose English is the batch's own: the locale already has a word for it. */
function same(english: Strings, known: string[], keys: string[]) {
    const plain = (text: string) => text.trim().toLowerCase();

    return keys.flatMap((key) =>
        known.filter((other) => plain(english[other]!) === plain(english[key]!)).slice(0, samePerString),
    );
}

/** Keys under the same parent as the batch's, then the wider area's when those are few: where the text appears. */
function neighbours(known: string[], keys: string[]) {
    const parent = (key: string, depth: number) => key.split(".").slice(0, depth).join(".");
    const under = (depth: (key: string) => number) => {
        const prefixes = new Set(keys.map((key) => parent(key, depth(key))));
        return known.filter((key) => prefixes.has(parent(key, depth(key))));
    };
    const closest = under((key) => key.split(".").length - 1);
    const wider = under(() => 2).filter((key) => !closest.includes(key));

    return [...closest, ...wider.slice(0, Math.max(0, widerFill - closest.length))].slice(0, neighbourCount);
}

/** For each word the batch's English uses, the shortest strings elsewhere that use it: they show its term. */
function terms(english: Strings, known: string[], keys: string[]) {
    const byLength = [...known].sort((a, b) => english[a]!.length - english[b]!.length);
    const picked: string[] = [];

    for (const word of new Set(keys.flatMap((key) => words(english[key]!)))) {
        const using = byLength.filter((key) => words(english[key]!).includes(word)).slice(0, termsPerWord);
        picked.push(...using);
    }

    return [...new Set(picked)].slice(0, termCount);
}

/** The words of `text` that can name something, stemmed: four letters or more, not common, placeholders left out. */
export function words(text: string) {
    const plain = text.replace(/\{\{\w+\}\}/g, " ").toLowerCase();

    return (plain.match(/\p{L}{4,}/gu) ?? []).filter((word) => !common.has(word)).map(stem);
}

/** `queue`, `queued` and `queues` as one word: the first ending whose removal leaves four letters goes. */
function stem(word: string) {
    const ending = ["ing", "ed", "es", "s", "e"].find((end) => word.endsWith(end) && word.length - end.length >= 4);

    return ending ? word.slice(0, -ending.length) : word;
}
