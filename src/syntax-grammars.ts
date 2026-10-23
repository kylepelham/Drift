import { createSignal } from "solid-js";

import type { BundledLanguage, BundledTheme, LanguageRegistration, SpecialLanguage } from "shiki";
import type { Grammar } from "./engine/store";
import type * as Shiki from "shiki";

type PluginLanguage = { id: string; registration: LanguageRegistration };

type Index = { byName: Map<string, PluginLanguage>; byFile: { ending: string; name: string }[] };

let index: Index = { byName: new Map(), byFile: [] };
const [revision, setRevision] = createSignal(0);

/** Moves on when plugins' grammars arrive or change; a view reads it to highlight again, nothing reads it by accident. */
export const pluginGrammarsRevision = revision;

/**
 * Takes the plugins' grammars. Each is loaded under its own id, so one named like a bundled language
 * replaces it rather than meeting the bundled one already loaded. Shiki keeps a grammar it has loaded,
 * so a grammar that changes under the same scope takes effect on the next start.
 */
export function setPluginGrammars(grammars: readonly Grammar[]) {
    const byName = new Map<string, PluginLanguage>();
    const byFile: Index["byFile"] = [];
    for (const grammar of grammars) {
        const name = grammar.name.toLowerCase();
        const id = `plugin-${name}`;
        const registration = { ...grammar.grammar, name: id, aliases: [] } as unknown as LanguageRegistration;
        for (const alias of [name, ...grammar.aliases.map((value) => value.toLowerCase())])
            if (!byName.has(alias)) byName.set(alias, { id, registration });
        for (const ending of grammar.extensions) byFile.push({ ending: ending.toLowerCase(), name });
    }
    index = { byName, byFile };
    setRevision((value) => value + 1);
}

/** The plugin grammar a code fence or file language names. */
export function pluginLanguage(lang: string) {
    return index.byName.get(lang.toLowerCase());
}

/** The plugin language of a file: one its whole name is listed for, else the longest ending listed. */
export function pluginFileLanguage(path: string) {
    const basename = (path.replaceAll("\\", "/").split("/").at(-1) ?? "").toLowerCase();
    const { byFile } = index;
    const whole = byFile.find((entry) => !entry.ending.startsWith(".") && entry.ending === basename);
    if (whole) return whole.name;

    const endings = byFile.filter((entry) => entry.ending.startsWith(".") && basename.endsWith(entry.ending));

    return endings.sort((a, b) => b.ending.length - a.ending.length)[0]?.name;
}

/** Tokens from shiki: a bundled language through its shorthand, a plugin's grammar once loaded into its highlighter. */
export async function shikiTokens(
    shiki: typeof Shiki,
    code: string,
    lang: string,
    theme: BundledTheme,
    grammar?: LanguageRegistration,
) {
    if (!grammar) return shiki.codeToTokens(code, { lang: lang as BundledLanguage | SpecialLanguage, theme });

    const highlighter = await shiki.getSingletonHighlighter({ langs: [grammar], themes: [theme] });

    // The highlighter's types name only the bundled languages; it holds this one now.
    return highlighter.codeToTokens(code, { lang: grammar.name as BundledLanguage, theme });
}

/** HTML from shiki, as `shikiTokens` picks its highlighter. */
export async function shikiHtml(
    shiki: typeof Shiki,
    code: string,
    lang: string,
    theme: BundledTheme,
    grammar?: LanguageRegistration,
) {
    if (!grammar) return shiki.codeToHtml(code, { lang, theme });

    const highlighter = await shiki.getSingletonHighlighter({ langs: [grammar], themes: [theme] });

    return highlighter.codeToHtml(code, { lang: grammar.name as BundledLanguage, theme });
}
