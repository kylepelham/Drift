// Fills in every locale from en.ts. Add English to en.ts only, then run `bun run i18n`: keys a locale lacks are
// translated, a translation already there is kept, and each file is written in en.ts's order.
// `--update <key...>` translates those keys again everywhere, for English whose meaning changed.
// `--check` lists what is missing without translating. DRIFT_I18N_MODEL picks another provider/model.
import { englishStrings, loadCatalog, localePath, localeSource, missingKeys } from "./i18n/catalog";
import { startModel, translate } from "./i18n/translate";
import { invariantKeys } from "./i18n/invariants";

import type { Catalog, Locale } from "./i18n/catalog";

const model = process.env.DRIFT_I18N_MODEL ?? "google/gemini-3.8-flash";
const batchSize = 60;
const exampleCount = 24;
const parallel = 32;

const catalog = await loadCatalog();
const english = englishStrings(catalog);
const updates = updateKeys(process.argv.slice(2));
const work = catalog.locales
    .map((locale) => ({ locale, keys: [...new Set([...missingKeys(catalog, locale), ...updates])] }))
    .filter((item) => item.keys.length > 0);

if (process.argv.includes("--check")) {
    for (const { locale, keys } of work)
        console.log(`${locale.id}: ${keys.length} missing (${keys.slice(0, 3).join(", ")})`);
    if (work.length) console.log("Run `bun run i18n` to translate them.");
    process.exit(work.length ? 1 : 0);
}

const started = performance.now();
const failures: string[] = [];
let translated = 0;
const notUpdated: string[] = [];
if (work.length) {
    const engine = await startModel(model);
    const jobs = work.flatMap(({ locale, keys }) => chunks(keys, batchSize).map((chunk) => ({ locale, chunk })));

    try {
        await inPool(jobs, parallel, async ({ locale, chunk }) => {
            const strings = Object.fromEntries(chunk.map((key) => [key, english[key]!]));
            const examples = examplesFor(catalog, locale, chunk);
            const done = await translate(engine, { locale: locale.name, strings, examples }).catch((error: Error) => {
                failures.push(`${locale.id}: ${error.message}`);
                return {};
            });
            Object.assign(locale.values, done);
            translated += Object.keys(done).length;
            notUpdated.push(
                ...chunk.filter((key) => updates.includes(key) && !(key in done)).map((key) => `${locale.id} ${key}`),
            );
        });
    } finally {
        await engine.close();
    }
}

// Every file is written in en.ts's order, so a later run only adds lines; keys en.ts dropped go too.
for (const locale of catalog.locales) {
    const file = Bun.file(localePath(locale.id));
    const source = await localeSource(catalog, locale.values);
    if ((await file.text()) !== source) await Bun.write(file, source);
}

const left = catalog.locales.flatMap((locale) => missingKeys(catalog, locale).map((key) => `${locale.id} ${key}`));
const seconds = ((performance.now() - started) / 1000).toFixed(1);
console.log(`Translated ${translated} strings with ${model} in ${seconds}s.`);
for (const failure of new Set(failures)) console.log(`failed ${failure}`);
if (notUpdated.length)
    console.log(`Kept the old translation of ${notUpdated.length}: ${notUpdated.slice(0, 10).join(", ")}`);
if (left.length || notUpdated.length) {
    if (left.length) console.log(`Still missing ${left.length}: ${left.slice(0, 10).join(", ")}`);
    console.log(
        "Save a Gemini API key in Drift's Settings > Providers > Google, or set GEMINI_API_KEY, then run it again.",
    );
    process.exit(1);
}

/** The keys named after `--update`, up to the next flag; each must be an English key that is translated. */
function updateKeys(args: string[]) {
    const at = args.indexOf("--update");
    if (at === -1) return [];

    const after = args.slice(at + 1);
    const nextFlag = after.findIndex((arg) => arg.startsWith("--"));
    const named = nextFlag === -1 ? after : after.slice(0, nextFlag);
    const unknown = named.filter((key) => !(key in english) || invariantKeys.has(key));
    if (named.length === 0 || unknown.length) {
        console.log(
            named.length ? `Not translated English keys: ${unknown.join(", ")}` : "--update needs one or more keys.",
        );
        process.exit(2);
    }

    return named;
}

function chunks<T>(items: T[], size: number) {
    const out: T[][] = [];
    for (let at = 0; at < items.length; at += size) out.push(items.slice(at, at + size));

    return out;
}

/** Runs `run` over `items`, at most `limit` at once. */
async function inPool<T>(items: T[], limit: number, run: (item: T) => Promise<void>) {
    let next = 0;
    const worker = async () => {
        while (next < items.length) await run(items[next++]!);
    };

    await Promise.all(Array.from({ length: Math.min(limit, items.length) }, worker));
}

/** Existing translations near `keys` (the same parent key first), which show the locale's terms and tone. */
function examplesFor(catalog: Catalog, locale: Locale, keys: string[]) {
    const parent = (key: string, depth: number) => key.split(".").slice(0, depth).join(".");
    const wanted = new Set(keys);
    const translated = Object.keys(englishStrings(catalog)).filter((key) => key in locale.values && !wanted.has(key));
    const near = (depth: (key: string) => number) => {
        const prefixes = new Set(keys.map((key) => parent(key, depth(key))));
        return translated.filter((key) => prefixes.has(parent(key, depth(key))));
    };
    const picked = [...new Set([...near((key) => key.split(".").length - 1), ...near(() => 2)])].slice(0, exampleCount);

    return Object.fromEntries(picked.map((key) => [key, [english[key]!, locale.values[key]!] as [string, string]]));
}
