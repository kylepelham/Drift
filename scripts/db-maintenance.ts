/**
 * Audits and prunes the OpenCode session database.
 *
 * The engine is event sourced: `event` is an append-only log and `message` / `part` are projections
 * derived from it. Nothing ever trims the log, and every `message.part.updated` /
 * `message.updated` event stores a *full* JSON snapshot rather than a delta. A single streamed
 * reply therefore leaves behind one ever-larger copy per update, and the log grows to several times
 * the size of the data it describes.
 *
 * Transcripts are read from `message` / `part` (see `message-v2.ts` upstream, which selects from
 * MessageTable / PartTable), not by replaying the log. Superseded snapshots are therefore
 * recoverable without changing anything the user can see.
 *
 * By default this writes a pruned *copy* and leaves the original untouched, so a bad run costs
 * nothing. Pass --in-place only against a database you have backed up.
 *
 *   bun scripts/db-maintenance.ts --audit
 *   bun scripts/db-maintenance.ts --prune                  (dry run: reports, writes nothing)
 *   bun scripts/db-maintenance.ts --prune --apply          (writes <db>.pruned.db)
 *   bun scripts/db-maintenance.ts --prune --apply --in-place
 */

import { Database } from "bun:sqlite"
import { existsSync, rmSync, statSync } from "node:fs"
import path from "node:path"

const BYTES_PER_GB = 1024 ** 3

/** Mirrors engine_db.rs opencode_data_dir(). */
function opencodeDataDir() {
  const xdg = process.env.XDG_DATA_HOME
  if (xdg) return path.join(xdg, "opencode")
  const home = process.env.USERPROFILE ?? process.env.HOME
  if (!home) throw new Error("cannot resolve the OpenCode data directory")
  return path.join(home, ".local", "share", "opencode")
}

/**
 * Drift keeps its own archive list in drift.db (`session_meta.archived_at`), separate from the
 * engine's `session.time_archived`. The two overlap but are not the same set, so archiving a
 * session in Drift does not mark it archived in the engine database. Both are consulted.
 */
function driftDatabasePath() {
  const appData = process.env.APPDATA
  return appData ? path.join(appData, "dev.drift.app", "drift.db") : undefined
}

const DRIFT_SCHEMA = "drift"

/**
 * A prune rule. `describe` counts and measures what the rule would remove so a dry run can report
 * it; `apply` performs the deletion. Both must select exactly the same rows.
 */
type Rule = {
  name: string
  summary: string
  /** Rows and payload bytes this rule would delete. */
  describe: string
  apply: (db: Database) => void
}

/**
 * Snapshot events supersede each other: for one part (or message) only the newest carries the
 * current state, so every older copy is redundant. `seq` orders events within an aggregate.
 */
function supersededSnapshots(type: string, idPath: string): Rule {
  const superseded = `
    SELECT rowid FROM (
      SELECT rowid, ROW_NUMBER() OVER (
        PARTITION BY json_extract(data, '${idPath}') ORDER BY seq DESC
      ) AS rank
      FROM event WHERE type = '${type}'
    ) WHERE rank > 1`
  return {
    name: `superseded:${type}`,
    summary: `superseded ${type} snapshots (keeps the newest per ${idPath})`,
    describe: `SELECT COUNT(*) rows, COALESCE(SUM(LENGTH(CAST(data AS BLOB))), 0) bytes
               FROM event WHERE rowid IN (${superseded})`,
    apply: (db) => db.exec(`DELETE FROM event WHERE rowid IN (${superseded})`),
  }
}

/** Events whose aggregate is selected by `subquery`. */
function eventsWhereAggregate(name: string, summary: string, subquery: string): Rule {
  const selector = `aggregate_id IN (${subquery})`
  return {
    name,
    summary,
    describe: `SELECT COUNT(*) rows, COALESCE(SUM(LENGTH(CAST(data AS BLOB))), 0) bytes
               FROM event WHERE ${selector}`,
    apply: (db) => db.exec(`DELETE FROM event WHERE ${selector}`),
  }
}

function buildRules(driftAttached: boolean): Rule[] {
  // Archived means archived in either place: the engine's own flag, or Drift's archive list.
  const archived = driftAttached
    ? `SELECT id FROM session WHERE time_archived IS NOT NULL
       UNION SELECT session_id FROM ${DRIFT_SCHEMA}.session_meta WHERE archived_at IS NOT NULL`
    : `SELECT id FROM session WHERE time_archived IS NOT NULL`

  return [
    supersededSnapshots("message.part.updated.1", "$.part.id"),
    supersededSnapshots("message.updated.1", "$.info.id"),
    // A subagent reports a summary back to its parent, and that summary already lives in the
    // parent's transcript. The child's own log has no reader once the run is over.
    eventsWhereAggregate(
      "subagent-events",
      "events for subagent (child) sessions",
      "SELECT id FROM session WHERE parent_id IS NOT NULL",
    ),
    eventsWhereAggregate(
      "archived-events",
      driftAttached ? "events for sessions archived in Drift or the engine" : "events for engine-archived sessions",
      archived,
    ),
    orphanEvents,
  ]
}

// event_sequence has no foreign key to session, so deleting a session leaves its aggregate behind.
// These are unreachable.
const orphanEvents: Rule = {
  name: "orphan-events",
  summary: "events and aggregates whose session no longer exists",
  describe: `SELECT COUNT(*) rows, COALESCE(SUM(LENGTH(CAST(data AS BLOB))), 0) bytes
             FROM event WHERE aggregate_id NOT IN (SELECT id FROM session)`,
  apply: (db) => {
    db.exec("DELETE FROM event WHERE aggregate_id NOT IN (SELECT id FROM session)")
    db.exec("DELETE FROM event_sequence WHERE aggregate_id NOT IN (SELECT id FROM session)")
  },
}

const gb = (bytes: number) => `${(bytes / BYTES_PER_GB).toFixed(2)} GB`
const pragma = (db: Database, name: string) => Number(Object.values(db.query(`PRAGMA ${name}`).get() as object)[0])
const count = (db: Database, sql: string) => Number(Object.values(db.query(sql).get() as object)[0])

/**
 * Attaches drift.db so the archive rule can see sessions archived from Drift's UI. Nothing here
 * ever writes to that schema; `mode=ro` URIs are not available because bun:sqlite opens databases
 * without SQLITE_USE_URI. Returns false when it is unavailable, in which case the archive rule
 * falls back to the engine flag alone.
 */
function attachDrift(db: Database, driftPath: string | undefined) {
  if (!driftPath || !existsSync(driftPath)) return false
  try {
    db.exec(`ATTACH DATABASE '${driftPath.replaceAll("\\", "/")}' AS ${DRIFT_SCHEMA}`)
    // Confirm the table this depends on actually exists in that schema.
    db.query(`SELECT 1 FROM ${DRIFT_SCHEMA}.session_meta LIMIT 1`).get()
    return true
  } catch {
    return false
  }
}

function audit(dbPath: string) {
  const db = new Database(dbPath, { readonly: true })
  try {
    const pages = pragma(db, "page_count")
    const pageSize = pragma(db, "page_size")
    console.log(`database : ${dbPath}`)
    console.log(`size     : ${gb(pages * pageSize)}`)
    console.log(`reclaimable by VACUUM alone: ${gb(pragma(db, "freelist_count") * pageSize)}`)

    console.log("\npayload by table")
    for (const [table, column] of [
      ["event", "data"],
      ["part", "data"],
      ["message", "data"],
    ]) {
      const row = db
        .query(`SELECT COUNT(*) rows, COALESCE(SUM(LENGTH(CAST("${column}" AS BLOB))), 0) bytes FROM "${table}"`)
        .get() as { rows: number; bytes: number }
      console.log(`  ${table.padEnd(10)} ${gb(row.bytes).padStart(9)}  ${row.rows.toLocaleString()} rows`)
    }

    console.log("\nsessions")
    for (const [label, where] of [
      ["total", "1=1"],
      ["top-level", "parent_id IS NULL"],
      ["subagent", "parent_id IS NOT NULL"],
      ["archived", "time_archived IS NOT NULL"],
    ]) {
      console.log(`  ${label.padEnd(16)} ${String(count(db, `SELECT COUNT(*) c FROM session WHERE ${where}`)).padStart(7)}`)
    }
    if (attachDrift(db, driftDatabasePath())) {
      const drift = count(db, `SELECT COUNT(*) c FROM ${DRIFT_SCHEMA}.session_meta WHERE archived_at IS NOT NULL`)
      const union = count(
        db,
        `SELECT COUNT(*) c FROM (SELECT id FROM session WHERE time_archived IS NOT NULL
         UNION SELECT session_id FROM ${DRIFT_SCHEMA}.session_meta WHERE archived_at IS NOT NULL)`,
      )
      console.log(`  archived (Drift) ${String(drift).padStart(7)}`)
      console.log(`  archived (union) ${String(union).padStart(7)}`)
    }
  } finally {
    db.close()
  }
}

/** Reports what every rule would remove. Rules overlap, so totals are not additive. */
function report(dbPath: string) {
  const db = new Database(dbPath, { readonly: true })
  try {
    const driftAttached = attachDrift(db, driftDatabasePath())
    if (!driftAttached) console.log("note: drift.db unavailable, archive rule uses the engine flag only\n")
    console.log("rule                                 rows        bytes")
    for (const rule of buildRules(driftAttached)) {
      const row = db.query(rule.describe).get() as { rows: number; bytes: number }
      console.log(`  ${rule.name.padEnd(34)} ${String(row.rows).padStart(9)}  ${gb(row.bytes).padStart(9)}`)
    }
    console.log("\nrules overlap (a subagent event can also be a superseded snapshot),")
    console.log("so these figures are upper bounds per rule, not a sum.")
  } finally {
    db.close()
  }
}

function prune(sourcePath: string, inPlace: boolean) {
  let target = sourcePath
  if (!inPlace) {
    target = sourcePath.replace(/\.db$/, "") + ".pruned.db"
    if (existsSync(target)) rmSync(target)
    // VACUUM INTO takes a consistent snapshot even while the engine is writing, which a file copy
    // would not: the WAL could be mid-transaction.
    console.log(`copying to ${target} ...`)
    const source = new Database(sourcePath, { readonly: true })
    try {
      source.exec(`VACUUM INTO '${target.replaceAll("\\", "/")}'`)
    } finally {
      source.close()
    }
  }

  const before = statSync(target).size
  const db = new Database(target)
  try {
    // This is a disposable copy unless --in-place was passed, so durability buys nothing here and
    // costs a great deal of time on a multi-gigabyte delete.
    if (!inPlace) db.exec("PRAGMA journal_mode = OFF; PRAGMA synchronous = OFF")
    const driftAttached = attachDrift(db, driftDatabasePath())
    if (!driftAttached) console.log("note: drift.db unavailable, archive rule uses the engine flag only")
    for (const rule of buildRules(driftAttached)) {
      const started = Date.now()
      rule.apply(db)
      console.log(`  applied ${rule.name.padEnd(34)} ${((Date.now() - started) / 1000).toFixed(1)}s`)
    }
    console.log("vacuuming ...")
    db.exec("VACUUM")
    const check = Object.values(db.query("PRAGMA integrity_check").get() as object)[0]
    const violations = (db.query("PRAGMA foreign_key_check").all() as unknown[]).length
    console.log(`integrity_check   : ${check}`)
    console.log(`foreign_key_check : ${violations === 0 ? "clean" : `${violations} violations`}`)
  } finally {
    db.close()
  }

  const after = statSync(target).size
  console.log(`\n${gb(before)} -> ${gb(after)}   reclaimed ${gb(before - after)} (${Math.round((1 - after / before) * 100)}%)`)
  if (!inPlace) console.log(`\nPruned copy: ${target}\nClose Drift, then swap it in to adopt it.`)
}

const args = process.argv.slice(2)
const flag = (name: string) => args.includes(`--${name}`)
const value = (name: string) => {
  const index = args.indexOf(`--${name}`)
  return index >= 0 ? args[index + 1] : undefined
}

const dbPath = value("db") ?? path.join(opencodeDataDir(), "opencode.db")
if (!existsSync(dbPath)) {
  console.error(`database not found: ${dbPath}`)
  process.exit(1)
}

if (flag("prune")) {
  const apply = flag("apply")
  const inPlace = flag("in-place")
  if (!apply) {
    console.log(`dry run against ${dbPath}\n`)
    report(dbPath)
    console.log("\nRe-run with --apply to write a pruned copy, or --apply --in-place to rewrite this database.")
  } else {
    if (inPlace) console.log("!! --in-place rewrites the database. Close Drift and keep a backup.\n")
    prune(dbPath, inPlace)
  }
} else if (flag("audit")) {
  audit(dbPath)
} else {
  console.log("usage: bun scripts/db-maintenance.ts (--audit | --prune [--apply] [--in-place]) [--db <path>]")
  process.exit(1)
}
