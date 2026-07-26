import { expect, test } from "bun:test"
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs"
import os from "node:os"
import path from "node:path"
import {
  assertVersionIsNewer,
  compareStableTags,
  latestStableTag,
  stampReleaseVersion,
  validateReleasePolicy,
  versionFromTag,
} from "../scripts/release-policy"

const root = path.resolve(import.meta.dirname, "..")

test("stable tags have exactly one v and three numeric components", () => {
  expect(versionFromTag("v1.2.3")).toBe("1.2.3")
  for (const tag of [
    "1.2.3",
    "vv1.2.3",
    "v1.2",
    "v1.2.3junk",
    "v1.2.3-beta.1",
    "v1.2.3+build.1",
    "v1.2.3.4",
  ]) expect(() => versionFromTag(tag)).toThrow("must match vMAJOR.MINOR.PATCH exactly")
})

test("stable versions compare numerically", () => {
  expect(compareStableTags("v2.0.0", "v1.999.999")).toBe(1)
  expect(compareStableTags("v1.10.0", "v1.9.99")).toBe(1)
  expect(compareStableTags("v1.2.3", "v1.2.3")).toBe(0)
})

test("release versions must be strictly newer", () => {
  expect(() => assertVersionIsNewer("v1.2.2", "v1.2.3")).toThrow("must be newer")
  expect(() => assertVersionIsNewer("v1.2.3", "v1.2.3")).toThrow("must be newer")
  expect(() => assertVersionIsNewer("v1.2.4", "v1.2.3")).not.toThrow()
  expect(() => assertVersionIsNewer("v1.0.0", undefined)).not.toThrow()
})

test("latest stable candidate excludes prereleases, drafts, malformed tags, and the current rerun", () => {
  expect(latestStableTag(
    ["v1.1.0", "v1.2.0", "v9.0.0-beta.1"],
    [
      { tag_name: "v1.3.0", prerelease: true },
      { tag_name: "v1.4.0", draft: true },
      { tag_name: "v1.2.1junk" },
      { tag_name: "v1.2.0" },
    ],
    "v1.2.0",
  )).toBe("v1.1.0")
})

test("policy rejects a tag whose commit is outside origin/master", () => {
  expect(() => validateReleasePolicy({
    tag: "v1.2.0",
    containedInMaster: false,
    tags: ["v1.1.0"],
    releases: [],
  })).toThrow("not contained in origin/master")
})

test("policy allows no prior release and safe reruns of the newest tag", () => {
  expect(validateReleasePolicy({
    tag: "v1.0.0",
    containedInMaster: true,
    tags: ["v1.0.0"],
    releases: [{ tag_name: "v1.0.0" }],
  })).toEqual({ version: "1.0.0", latest: undefined })

  expect(validateReleasePolicy({
    tag: "v1.2.0",
    containedInMaster: true,
    tags: ["v1.1.0", "v1.2.0"],
    releases: [{ tag_name: "v1.2.0" }],
  })).toEqual({ version: "1.2.0", latest: "v1.1.0" })
})

test("release stamping updates every package version and is idempotent", () => {
  const temporary = mkdtempSync(path.join(os.tmpdir(), "drift-release-policy-"))
  try {
    mkdirSync(path.join(temporary, "src-tauri"))
    writeFileSync(path.join(temporary, "package.json"), '{"version":"1.0.0"}\n')
    writeFileSync(path.join(temporary, "src-tauri/tauri.conf.json"), '{"version":"1.0.0"}\n')
    writeFileSync(path.join(temporary, "src-tauri/Cargo.toml"), '[package]\nname = "drift"\nversion = "1.0.0"\n')
    writeFileSync(path.join(temporary, "src-tauri/Cargo.lock"), '[[package]]\nname = "drift"\nversion = "1.0.0"\n')

    expect(stampReleaseVersion("v2.3.4", temporary)).toBe("2.3.4")
    expect(stampReleaseVersion("v2.3.4", temporary)).toBe("2.3.4")
    for (const relative of ["package.json", "src-tauri/tauri.conf.json", "src-tauri/Cargo.toml", "src-tauri/Cargo.lock"]) {
      expect(readFileSync(path.join(temporary, relative), "utf8")).toContain("2.3.4")
    }
  } finally {
    rmSync(temporary, { recursive: true, force: true })
  }
})

test("release workflow gates secrets and publication on policy and full validation", () => {
  const workflow = readFileSync(path.join(root, ".github/workflows/release.yml"), "utf8")
  const validation = workflow.slice(workflow.indexOf("  validation:"), workflow.indexOf("  publish:"))
  const publish = workflow.slice(workflow.indexOf("  publish:"))

  expect(workflow).toContain("needs: tag-policy")
  expect(publish).toContain("needs: [tag-policy, validation]")
  expect(validation).not.toContain("secrets.")
  for (const command of [
    "bun install --frozen-lockfile",
    "bun run typecheck",
    "bun run test",
    "bun run test:engine",
    "bun run build:engine",
    "cargo test --locked --manifest-path src-tauri/Cargo.toml",
    "bun run build:native",
  ]) expect(validation).toContain(command)
  expect(publish).toContain("TAURI_SIGNING_PRIVATE_KEY: ${{ secrets.TAURI_SIGNING_PRIVATE_KEY }}")
  expect(publish).toContain("uses: softprops/action-gh-release@v2")
})
