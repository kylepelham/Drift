/** Unchanged lines kept beside each change; longer unchanged runs fold into a bar. */
const reviewContext = 3;

export type ReviewRow =
    | { kind: "add" | "del" | "ctx"; line: number; text: string; index: number }
    | { kind: "fold"; start: number; count: number };

/** A run of unchanged lines folds only when hiding it saves more than the bar costs. */
const minimumFold = 2;

/**
 * Rows for a file's merged lines (each prefixed ` `, `+` or `-`), with unchanged runs beyond
 * `reviewContext` lines of a change folded unless their first index is in `opened`.
 */
export function reviewRows(lines: string[], opened: ReadonlySet<number>, context = reviewContext): ReviewRow[] {
    const folds = foldedRuns(lines, opened, context);
    const rows: ReviewRow[] = [];
    let oldLine = 1;
    let newLine = 1;

    for (let index = 0; index < lines.length;) {
        const end = folds.get(index);
        if (end !== undefined) {
            rows.push({ kind: "fold", start: index, count: end - index });
            oldLine += end - index;
            newLine += end - index;
            index = end;
            continue;
        }

        const mark = lines[index][0];
        const text = lines[index].slice(1);
        if (mark === "+") rows.push({ kind: "add", line: newLine++, text, index });
        else if (mark === "-") rows.push({ kind: "del", line: oldLine++, text, index });
        else {
            rows.push({ kind: "ctx", line: newLine, text, index });
            oldLine++;
            newLine++;
        }
        index++;
    }

    return rows;
}

/** Each folded run's start and end: unchanged lines away from any change, unless opened. */
function foldedRuns(lines: string[], opened: ReadonlySet<number>, context: number) {
    const changed = lines.map((line) => line[0] === "+" || line[0] === "-");
    const near = nearChanges(changed, context);
    const folds = new Map<number, number>();
    for (let index = 0; index < lines.length; index++) {
        if (changed[index] || near[index]) continue;

        let end = index;
        while (end < lines.length && !changed[end] && !near[end]) end++;
        if (end - index >= minimumFold && !opened.has(index)) folds.set(index, end);
        index = end;
    }

    return folds;
}

/** Which lines are within `context` lines of a change. */
function nearChanges(changed: boolean[], context: number) {
    const near = changed.map(() => false);
    changed.forEach((isChange, index) => {
        if (!isChange) return;
        for (
            let offset = Math.max(0, index - context);
            offset <= Math.min(changed.length - 1, index + context);
            offset++
        )
            near[offset] = true;
    });

    return near;
}
