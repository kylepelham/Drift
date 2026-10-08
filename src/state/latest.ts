/** Issues attempt tokens so only the newest asynchronous result can be applied. */
export function createLatestOnly() {
    let current = 0;

    return {
        /** Marks the start of an attempt and returns its token. */
        begin: () => ++current,
        /** True while no newer attempt has started. */
        isCurrent: (token: number) => token === current,
    };
}
