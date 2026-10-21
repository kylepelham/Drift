// Locale files as data: what en.ts holds, what each locale lacks, and the file each locale should be.
import { languages } from "../../src/state/language";
import { format, resolveConfig } from "prettier";
import { invariantKeys } from "./invariants";
import path from "node:path";

const folder = path.resolve(import.meta.dirname, "../../src/i18n");
const objects = ["dict", "drift"] as const;

type Strings = Record<string, string>;
type Objects = Record<(typeof objects)[number], Strings>;

export type Locale = { id: string; name: string; values: Strings };

export type Catalog = { english: Objects; locales: Locale[] };

export async function loadCatalog(): Promise<Catalog> {
    const english = (await import(localePath("en"))) as Objects;
    const others = languages.filter((language) => language.id !== "en");
    const locales = await Promise.all(
        others.map(async (language) => {
            const loaded = (await import(localePath(language.id))) as Objects;

            return {
                id: language.id,
                name: `${language.label} (${language.locale})`,
                values: { ...loaded.dict, ...loaded.drift },
            };
        }),
    );

    return { english, locales };
}

/** Every English key and its text, in en.ts's order. */
export function englishStrings(catalog: Catalog): Strings {
    return { ...catalog.english.dict, ...catalog.english.drift };
}

/** The English keys `locale` has no translation for; invariant keys are left to English. */
export function missingKeys(catalog: Catalog, locale: Locale) {
    return Object.keys(englishStrings(catalog)).filter((key) => !invariantKeys.has(key) && !(key in locale.values));
}

/** A locale file as it should be: en.ts's keys in its order and objects, as Prettier writes it. Keys en.ts dropped go. */
export async function localeSource(catalog: Catalog, values: Strings) {
    const blocks = objects.map((object) => {
        const lines = Object.keys(catalog.english[object])
            .filter((key) => key in values && !invariantKeys.has(key))
            .map((key) => `    ${JSON.stringify(key)}: ${JSON.stringify(values[key])},`);

        return `export const ${object} = {\n${lines.join("\n")}\n};\n`;
    });
    const options = (await resolveConfig(localePath("en"))) ?? {};

    return format(blocks.join("\n"), { ...options, filepath: "locale.ts" });
}

export function localePath(id: string) {
    return path.join(folder, `${id}.ts`);
}
