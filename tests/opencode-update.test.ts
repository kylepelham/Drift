import { readFileSync } from "node:fs"
import path from "node:path"
import { describe, expect, test } from "bun:test"

const root = path.resolve(import.meta.dir, "..")
const updateWorkflow = readFileSync(path.join(root, ".github/workflows/opencode-update.yml"), "utf8")
const ciWorkflow = readFileSync(path.join(root, ".github/workflows/ci.yml"), "utf8")

describe("OpenCode update workflow", () => {
  test("runs daily and manually with explicit write permissions", () => {
    expect(updateWorkflow).toContain('cron: "17 6 * * *"')
    expect(updateWorkflow).toContain("workflow_dispatch:")
    expect(updateWorkflow).toContain("actions: write")
    expect(updateWorkflow).toContain("contents: write")
    expect(updateWorkflow).toContain("pull-requests: write")
  })

  test("uses a dedicated no-tags ref and a durable subtree split marker", () => {
    expect(updateWorkflow).toContain("git fetch --no-tags")
    expect(updateWorkflow).toContain("refs/remotes/opencode-update/dev")
    expect(updateWorkflow).toContain("SPLIT_FILE: engine/upstream.commit")
    expect(updateWorkflow).toContain("git-subtree-split: $CURRENT")
    expect(readFileSync(path.join(root, "engine/upstream.commit"), "utf8").trim()).toMatch(/^[0-9a-f]{40}$/)
    expect(updateWorkflow).not.toContain("FETCH_HEAD")
    expect(updateWorkflow).not.toContain("git subtree pull")
  })

  test("keeps one review PR and explicitly dispatches CI", () => {
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
