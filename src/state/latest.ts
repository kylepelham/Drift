/**
 * Guards against stale async results.
 *
 * When an operation can be started again before an earlier run finishes, the earlier run must not
 * be allowed to publish its result: responses can arrive out of order, and the newest request is
 * the one the user is waiting on. Each start takes a token; only the holder of the newest token is
 * allowed to commit.
 *
 * ```ts
 * const load = createLatestOnly()
 * const token = load.begin()
 * const value = await fetchSomething()
 * if (load.isCurrent(token)) setValue(value)
 * ```
 */
export function createLatestOnly() {
  let current = 0
  return {
    /** Marks the start of an attempt and returns its token. */
    begin: () => ++current,
    /** True while no newer attempt has started. */
    isCurrent: (token: number) => token === current,
  }
}
