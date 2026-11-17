/** Visited chats, oldest first, walked back and forward like a browser's history. */
export class VisitHistory {
    private visits: string[] = [];
    private at = -1;

    constructor(private limit = 100) {}

    visit(id: string) {
        if (this.visits[this.at] === id) return;
        this.visits = [...this.visits.slice(0, this.at + 1), id].slice(-this.limit);
        this.at = this.visits.length - 1;
    }

    /** Moves to the nearest visit in `direction` that `live` still accepts, skipping archived or deleted chats. */
    step(direction: 1 | -1, live: (id: string) => boolean) {
        for (let index = this.at + direction; index >= 0 && index < this.visits.length; index += direction) {
            if (!live(this.visits[index])) continue;
            this.at = index;
            return this.visits[index];
        }

        return undefined;
    }
}

/** The item one `step` from `index`, wrapping at either end; from nowhere, the first going forward or the last going back. */
export function stepAround<T>(items: T[], index: number, step: 1 | -1) {
    if (!items.length) return undefined;
    if (index < 0) return step > 0 ? items[0] : items.at(-1);

    return items[(index + step + items.length) % items.length];
}

/** The chat a Ctrl+digit opens: the nth, with 9 always the last, as browsers treat their tabs. */
export function nthSession<T>(items: T[], digit: number) {
    return digit === 9 ? items.at(-1) : items[digit - 1];
}
