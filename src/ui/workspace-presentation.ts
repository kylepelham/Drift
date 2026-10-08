import { t } from "../state/i18n";

export function workspaceInitials(name: string) {
    const words = name.split(/[\s\-_.]+/).filter(Boolean);
    const letters = words.slice(0, 2).map((word) => word.charAt(0));

    return (letters.join("") || name.charAt(0)).toUpperCase();
}

/** Local midnight at or before `timestamp`. */
export function startOfDay(timestamp: number) {
    const date = new Date(timestamp);
    date.setHours(0, 0, 0, 0);

    return date.getTime();
}

/** Today, Yesterday, a weekday within the past week, or the local date. */
export function dayLabel(timestamp: number, now: number) {
    const days = Math.round((startOfDay(now) - startOfDay(timestamp)) / 86_400_000);
    if (days <= 0) return t("drift.sidebar.today");
    if (days === 1) return t("drift.sidebar.yesterday");

    const date = new Date(timestamp);
    if (days < 7) return date.toLocaleDateString(undefined, { weekday: "long" });

    const sameYear = date.getFullYear() === new Date(now).getFullYear();
    const options: Intl.DateTimeFormatOptions = sameYear
        ? { day: "numeric", month: "short" }
        : { day: "numeric", month: "short", year: "numeric" };

    return date.toLocaleDateString(undefined, options);
}

/** The heading each day's first thread carries, for threads newest first. */
export function dayDividers(rows: { id: string; updated: number }[], now: number) {
    const headings = new Map<string, string>();
    let previous: number | undefined;

    for (const row of rows) {
        const day = startOfDay(row.updated);
        if (day !== previous) headings.set(row.id, dayLabel(row.updated, now));
        previous = day;
    }

    return headings;
}

export function ago(timestamp: number) {
    const seconds = Math.max(0, (Date.now() - timestamp) / 1000);
    if (seconds < 60) return t("common.time.justNow");
    if (seconds < 3600) return t("common.time.minutesAgo.short", { count: Math.floor(seconds / 60) });
    if (seconds < 86400) return t("common.time.hoursAgo.short", { count: Math.floor(seconds / 3600) });

    return t("common.time.daysAgo.short", { count: Math.floor(seconds / 86400) });
}
