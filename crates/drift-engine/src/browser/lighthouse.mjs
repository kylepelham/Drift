// Runs one Lighthouse audit on a page of a browser Drift already runs, connecting over its DevTools port.
// Argument: JSON with port, target, mode, device, userAgent, categories, out. Prints one JSON summary line.

import { writeFile } from "node:fs/promises";
import { join } from "node:path";

import puppeteer from "puppeteer-core";
import { navigation, snapshot, generateReport } from "lighthouse";

const MAX_NODES = 10;
const DESKTOP = { mobile: false, width: 1350, height: 940, deviceScaleFactor: 1, disabled: false };
const PHONE = { mobile: true, width: 412, height: 823, deviceScaleFactor: 1.75, disabled: false };

const options = JSON.parse(process.argv[2]);

function nodes(details, found) {
    if (Array.isArray(details)) {
        for (const item of details) nodes(item, found);
        return;
    }
    if (!details || typeof details !== "object" || details.type === "debugdata") return;
    if (details.type === "node") {
        const key = details.lhId ?? details.path ?? `${details.selector}|${details.snippet}`;
        found.set(key, {
            selector: details.selector,
            snippet: details.snippet,
            label: details.nodeLabel,
            explanation: details.explanation,
            ...found.get(key),
        });
        return;
    }
    for (const [name, child] of Object.entries(details)) {
        if (name !== "subItems") nodes(child, found);
    }
}

function failed(lhr) {
    const categories = new Map();
    for (const category of Object.values(lhr.categories)) {
        for (const ref of category.auditRefs) {
            categories.set(ref.id, [...(categories.get(ref.id) ?? []), category.title]);
        }
    }

    return Object.values(lhr.audits)
        .filter((audit) => audit.score !== null && audit.score < 1 && audit.scoreDisplayMode !== "informative")
        .map((audit) => {
            const found = new Map();
            nodes(audit.details, found);
            return {
                id: audit.id,
                title: audit.title,
                score: audit.score,
                value: audit.displayValue,
                categories: categories.get(audit.id) ?? [],
                nodes: [...found.values()].slice(0, MAX_NODES),
                totalNodes: found.size,
            };
        });
}

async function audit(page) {
    const phone = options.device === "mobile";
    const flags = {
        onlyCategories: options.categories,
        formFactor: phone ? "mobile" : "desktop",
        screenEmulation: phone ? PHONE : DESKTOP,
        emulatedUserAgent: phone ? undefined : (options.userAgent ?? false),
        throttlingMethod: "provided",
        disableStorageReset: true,
        maxWaitForLoad: 30_000,
        output: ["json", "html"],
    };
    if (options.mode === "snapshot") return snapshot(page, { flags });

    return navigation(page, page.url(), { flags });
}

const browser = await puppeteer.connect({
    browserURL: `http://127.0.0.1:${options.port}`,
    defaultViewport: null,
});
try {
    const pages = await browser.pages();
    const page = pages.find((page) => page.target()._targetId === options.target);
    if (!page) throw new Error("the page to audit was not found in the browser");

    const result = await audit(page);
    if (!result) throw new Error("Lighthouse returned no result");
    const lhr = result.lhr;
    const reports = [];
    for (const format of ["html", "json"]) {
        const path = join(options.out, `lighthouse.${format}`);
        await writeFile(path, await generateReport(lhr, format));
        reports.push(path);
    }

    console.log(
        JSON.stringify({
            url: lhr.finalDisplayedUrl,
            version: lhr.lighthouseVersion,
            runtimeError: lhr.runtimeError?.message,
            warnings: lhr.runWarnings,
            scores: Object.values(lhr.categories).map((category) => ({ title: category.title, score: category.score })),
            passed: Object.values(lhr.audits).filter((audit) => audit.score === 1).length,
            failed: failed(lhr),
            reports,
        }),
    );
} finally {
    await browser.disconnect();
}
