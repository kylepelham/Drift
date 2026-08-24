import { execFileSync } from "node:child_process"
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs"
import os from "node:os"
import path from "node:path"
import { describe, expect, test } from "bun:test"

const root = path.resolve(import.meta.dir, "..")
const updateWorkflow = readFileSync(path.join(root, ".github/workflows/opencode-update.yml"), "utf8")
const ciWorkflow = readFileSync(path.join(root, ".github/workflows/ci.yml"), "utf8")

function git(cwd: string, ...args: string[]) {
  return execFileSync("git", args, { cwd, encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] }).trim()
}

describe("OpenCode update workflow", () => {
  test("runs daily and manually with explicit write permissions", () => {
    expect(updateWorkflow).toContain('cron: "17 6 * * *"')
    expect(updateWorkflow).toContain("workflow_dispatch:")
    expect(updateWorkflow).toContain("actions: write")
    expect(updateWorkflow).toContain("contents: write")
    expect(updateWorkflow).toContain("pull-requests: write")
  })

  test("imports a graph-clean snapshot through a temporary no-tags ref", () => {
    expect(updateWorkflow).toContain("git fetch --no-tags")
    expect(updateWorkflow).toContain("refs/remotes/opencode-update/dev")
    expect(updateWorkflow).toContain("REVISION_FILE: engine/upstream.commit")
    expect(updateWorkflow).toContain("git read-tree --prefix=engine/upstream/")
    expect(updateWorkflow).toContain('git update-ref -d "$UPSTREAM_REF"')
    expect(readFileSync(path.join(root, "engine/upstream.commit"), "utf8").trim()).toMatch(/^[0-9a-f]{40}$/)
    expect(updateWorkflow).not.toContain("FETCH_HEAD")
    expect(updateWorkflow).not.toContain("git subtree")
    expect(updateWorkflow).not.toContain("git-subtree-")
  })

  test("snapshot import keeps one parent, exact contents, no tags, and no upstream ref", () => {
    const temporary = mkdtempSync(path.join(os.tmpdir(), "drift-opencode-update-"))
    const upstream = path.join(temporary, "upstream")
    const drift = path.join(temporary, "drift")
    try {
      mkdirSync(upstream)
      git(upstream, "init")
      git(upstream, "config", "user.name", "Test")
      git(upstream, "config", "user.email", "test@example.com")
      git(upstream, "switch", "-c", "dev")
      writeFileSync(path.join(upstream, "old.txt"), "old\n")
      git(upstream, "add", "old.txt")
      git(upstream, "commit", "-m", "old")
      const previous = git(upstream, "rev-parse", "HEAD")
      rmSync(path.join(upstream, "old.txt"))
      mkdirSync(path.join(upstream, "src"))
      writeFileSync(path.join(upstream, "src", "engine.ts"), "export const version = 2\n")
      git(upstream, "add", "--all")
      git(upstream, "commit", "-m", "new")
      git(upstream, "tag", "v99.0.0")
      const latest = git(upstream, "rev-parse", "HEAD")
      const latestTree = git(upstream, "rev-parse", "HEAD^{tree}")

      mkdirSync(path.join(drift, "engine", "upstream"), { recursive: true })
      git(drift, "init")
      git(drift, "config", "user.name", "Test")
      git(drift, "config", "user.email", "test@example.com")
      writeFileSync(path.join(drift, "engine", "upstream", "old.txt"), "old\n")
      writeFileSync(path.join(drift, "engine", "upstream.commit"), `${previous}\n`)
      git(drift, "add", ".")
      git(drift, "commit", "-m", "baseline")
      const parent = git(drift, "rev-parse", "HEAD")

      git(drift, "fetch", "--no-tags", upstream, "+dev:refs/remotes/opencode-update/dev")
      git(drift, "merge-base", "--is-ancestor", previous, latest)
      git(drift, "rm", "-r", "--quiet", "engine/upstream")
      git(drift, "read-tree", "--prefix=engine/upstream/", "-u", "refs/remotes/opencode-update/dev^{tree}")
      writeFileSync(path.join(drift, "engine", "upstream.commit"), `${latest}\n`)
      git(drift, "add", "engine/upstream.commit")
      git(drift, "commit", "-m", "update snapshot")
      git(drift, "update-ref", "-d", "refs/remotes/opencode-update/dev")

      expect(git(drift, "rev-list", "--parents", "-1", "HEAD").split(/\s+/)).toHaveLength(2)
      expect(git(drift, "rev-parse", "HEAD^")).toBe(parent)
      expect(git(drift, "rev-parse", "HEAD:engine/upstream")).toBe(latestTree)
      expect(git(drift, "tag", "--list")).toBe("")
      expect(() => git(drift, "show-ref", "--verify", "refs/remotes/opencode-update/dev")).toThrow()
    } finally {
      rmSync(temporary, { recursive: true, force: true })
    }
  }, 15_000)

  test("keeps one review PR and explicitly dispatches CI", () => {
    expect(updateWorkflow).toContain("ref: master")
    expect(updateWorkflow).toContain('gh pr list --state open --head "$UPDATE_BRANCH"')
    expect(updateWorkflow).toContain('gh pr create --base master --head "$UPDATE_BRANCH"')
    expect(updateWorkflow).toContain('gh workflow run ci.yml --ref "$UPDATE_BRANCH"')
    expect(updateWorkflow).not.toContain("gh pr merge")
    expect(updateWorkflow).not.toContain("--auto")
    expect(ciWorkflow).toContain("workflow_dispatch:")
    expect(ciWorkflow).toContain("if: github.event_name != 'pull_request'")
  })

  test("pins every third-party action by commit", () => {
    const actions = [...updateWorkflow.matchAll(/^\s*-?\s*uses:\s*([^\s#]+)/gm)].map((match) => match[1])
    expect(actions).toEqual(["actions/checkout@11bd71901bbe5b1630ceea73d27597364c9af683"])
  })
})
