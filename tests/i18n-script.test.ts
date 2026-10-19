import { localePath, localeSource, loadCatalog, missingKeys } from "../scripts/i18n/catalog";
import { checked, parseAnswer } from "../scripts/i18n/translate";
import { expect, test } from "bun:test";

test("a translation is kept only with the same placeholders, and an em dash becomes a hyphen", () => {
    expect(checked("{{count}} prompts waiting", "{{count}} Prompts warten")).toBe("{{count}} Prompts warten");
    expect(checked("{{count}} prompts waiting", "{{anzahl}} Prompts warten"), "a renamed placeholder").toBeUndefined();
    expect(checked("Copy {{path}}", "Copier"), "a lost placeholder").toBeUndefined();
    expect(checked("Retry", "  "), "empty").toBeUndefined();
    expect(checked("Retry", 3), "not text").toBeUndefined();
    expect(checked("Saved, then sent", "Gespeichert \u2014 dann gesendet")).toBe("Gespeichert - dann gesendet");
});

test("the answer's JSON is found inside a code fence or a sentence, and anything else is no answer", () => {
    expect(parseAnswer('```json\n{"drift.queue.edit": "Bearbeiten"}\n```')).toEqual({
        "drift.queue.edit": "Bearbeiten",
    });
    expect(parseAnswer('Here you go: {"a": "b"} Hope it helps.')).toEqual({ a: "b" });
    expect(parseAnswer("I cannot help with that.")).toEqual({});
    expect(parseAnswer("[1, 2]")).toEqual({});
});

test("every locale holds every key en.ts has and is written as `bun run i18n` writes it", async () => {
    const catalog = await loadCatalog();

    for (const locale of catalog.locales) {
        expect(missingKeys(catalog, locale), `${locale.id} is missing keys: run bun run i18n`).toEqual([]);
        const written = await Bun.file(localePath(locale.id)).text();
        expect(written === (await localeSource(catalog, locale.values)), `${locale.id}: run bun run i18n`).toBeTrue();
    }
});
