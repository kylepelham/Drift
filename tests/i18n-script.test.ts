import { localePath, localeSource, loadCatalog, missingKeys } from "../scripts/i18n/catalog";
import { checked, parseAnswer } from "../scripts/i18n/translate";
import { expect, test } from "bun:test";
import { t } from "../src/state/i18n";

test("a value is put in as written, even one with replacement patterns in it", () => {
    expect(t("drift.path.copied", { path: "C:/$$tmp/$&x" })).toBe("Copied C:/$$tmp/$&x");
});

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

test("examples show the keys beside a string and how the locale already says the words it uses", async () => {
    const { examplesFor, words } = await import("../scripts/i18n/examples");
    expect(words("Queue {{count}} prompts with Alt+Enter")).toEqual(["queu", "prompt", "enter"]);
    expect(words("Queued"), "an ending does not hide the word").toEqual(["queu"]);

    const english = {
        "drift.prompt.steer": "Steer",
        "drift.prompt.steerHint": "Steer (hold Alt to queue for after this turn)",
        "drift.queue.label": "Queued",
        "drift.queue.discard": "Discard",
        "drift.settings.theme": "Theme",
    };
    const japanese = {
        "drift.prompt.steer": "方向を修正",
        "drift.queue.label": "キュー",
        "drift.queue.discard": "破棄",
        "drift.settings.theme": "テーマ",
    };
    const examples = examplesFor(english, japanese, ["drift.prompt.steerHint"]);

    expect(examples["drift.prompt.steer"], "its neighbour").toEqual(["Steer", "方向を修正"]);
    expect(examples["drift.queue.label"], "the locale's word for queue, from elsewhere").toEqual(["Queued", "キュー"]);
    expect(Object.keys(examples)).not.toContain("drift.settings.theme");
    expect(Object.keys(examples)).not.toContain("drift.prompt.steerHint");
});

test("the same English already translated elsewhere comes first, and the wider area only pads a short list", async () => {
    const { examplesFor } = await import("../scripts/i18n/examples");
    const english = {
        "drift.preview.mode.none": "None",
        "drift.settings.groupTools.commands": "Commands",
        "drift.settings.groupTools.none": "None",
        "drift.settings.prompts.unsaved": "Unsaved changes",
    };
    const chinese = {
        "drift.preview.mode.none": "无",
        "drift.settings.groupTools.commands": "命令",
        "drift.settings.prompts.unsaved": "未保存的更改",
    };
    const examples = Object.keys(examplesFor(english, chinese, ["drift.settings.groupTools.none"]));

    expect(examples[0], "the locale's word for None, first").toBe("drift.preview.mode.none");
    expect(examples).toContain("drift.settings.groupTools.commands");
    expect(examples, "one neighbour is too few, so the wider area pads it").toContain("drift.settings.prompts.unsaved");
});
