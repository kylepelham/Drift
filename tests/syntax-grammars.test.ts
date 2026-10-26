import { pluginFileLanguage, pluginLanguage, setPluginGrammars, shikiHtml, shikiTokens } from "../src/syntax-grammars";
import { fileLanguageCandidates, resolveFileLanguage } from "../src/syntax-language";
import { expect, test } from "bun:test";

const roc = await Bun.file(new URL("../plugins/roc/roc.tmLanguage.json", import.meta.url)).json();
const grammar = (name: string, extensions: string[], aliases: string[] = []) => ({
    name,
    aliases,
    extensions,
    grammar: roc,
    plugin: "roc",
});

test("a plugin's grammar names its files and fences, before and over the bundled languages", async () => {
    setPluginGrammars([grammar("Roc", [".roc", "Rocfile"], ["roclang"]), grammar("python", [".py"])]);

    expect(await resolveFileLanguage("C:\\app\\main.roc")).toBe("roc");
    expect(await fileLanguageCandidates("app/Rocfile")).toEqual(["roc"]);
    expect(pluginFileLanguage("rocfile.txt")).toBeUndefined();
    expect(await resolveFileLanguage("tool.py"), "a plugin may replace a bundled language").toBe("python");
    expect(pluginLanguage("ROCLANG")?.id).toBe("plugin-roc");
    expect(pluginLanguage("python")?.registration.name, "loaded under its own id, never the bundled one").toBe(
        "plugin-python",
    );

    setPluginGrammars([]);
    expect(await resolveFileLanguage("tool.py")).toBe("python");
    expect(pluginLanguage("python")).toBeUndefined();
    expect(await resolveFileLanguage("main.roc")).toBe("text");
});

test("code in a plugin's language is highlighted with its grammar, as tokens and as HTML", async () => {
    setPluginGrammars([grammar("roc", [".roc"])]);
    const shiki = await import("shiki");
    const code = 'main = if ready then "go" else crash "no" # done';
    const registration = pluginLanguage("roc")!.registration;

    const tokens = (await shikiTokens(shiki, code, "roc", "github-dark", registration)).tokens[0]!;
    const color = (text: string) => tokens.find((token) => token.content.includes(text))?.color;
    expect(color("if")).not.toBe(color("main"));
    expect(color("# done")).not.toBe(color('"go"'));
    expect(await shikiHtml(shiki, code, "roc", "github-dark", registration)).toContain("<span");

    const plain = await shikiTokens(shiki, "x = 1", "python", "github-dark");
    expect(plain.tokens.length, "a bundled language still goes through the shorthand").toBe(1);
    setPluginGrammars([]);
});

test("a grammar held in the engine's store reaches the syntax worker as a copy it can be posted", () => {
    const proxied = (value: unknown): unknown =>
        value && typeof value === "object"
            ? new Proxy(value, { get: (target, key) => proxied(Reflect.get(target, key)) })
            : value;
    // The app's store proxies every object in it, as Solid's browser build does, and a proxy cannot be posted.
    setPluginGrammars([{ ...grammar("roc", [".roc"]), grammar: proxied(roc) as typeof roc }]);

    expect(() => structuredClone(pluginLanguage("roc")!.registration)).not.toThrow();
    setPluginGrammars([]);
});
