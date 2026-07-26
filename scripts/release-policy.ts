import { appendFileSync, readFileSync, writeFileSync } from "node:fs"
import path from "node:path"

export type GitHubRelease = {
  tag_name?: string
  body?: string | null
  draft?: boolean
  prerelease?: boolean
}

export const stableTagPattern = /^v\d+\.\d+\.\d+$/

export function versionFromTag(tag: string) {
  if (!stableTagPattern.test(tag)) throw new Error(`Tag must match vMAJOR.MINOR.PATCH exactly, got ${tag}`)
  return tag.slice(1)
}

function versionParts(tag: string) {
  return versionFromTag(tag).split(".").map(BigInt)
}

export function compareStableTags(left: string, right: string) {
  const leftParts = versionParts(left)
  const rightParts = versionParts(right)
  for (let index = 0; index < leftParts.length; index++) {
    if (leftParts[index] < rightParts[index]) return -1
    if (leftParts[index] > rightParts[index]) return 1
  }
  return 0
}

export function latestStableTag(tags: string[], releases: GitHubRelease[], currentTag: string) {
  const candidates = [
    ...tags,
    ...releases
      .filter((release) => !release.draft && !release.prerelease)
      .map((release) => release.tag_name ?? ""),
  ].filter((tag) => tag !== currentTag && stableTagPattern.test(tag))

  return candidates.reduce<string | undefined>((latest, tag) => {
    return !latest || compareStableTags(tag, latest) > 0 ? tag : latest
  }, undefined)
}

export function assertVersionIsNewer(currentTag: string, latestTag: string | undefined) {
  versionFromTag(currentTag)
  if (latestTag && compareStableTags(currentTag, latestTag) <= 0) {
    throw new Error(`${currentTag} must be newer than latest stable release/tag ${latestTag}`)
  }
}

export function releaseRunMarker(runId: string, commit: string) {
  return `<!-- drift-release: run=${runId} commit=${commit.toLowerCase()} -->`
}

export function validateReleasePolicy(input: {
  tag: string
  triggerCommit: string
  resolvedCommit: string
  runId: string
  containedInMaster: boolean
  tags: string[]
  releases: GitHubRelease[]
}) {
  const version = versionFromTag(input.tag)
  const triggerCommit = input.triggerCommit.toLowerCase()
  const resolvedCommit = input.resolvedCommit.toLowerCase()
  if (resolvedCommit !== triggerCommit) {
    throw new Error(`${input.tag} resolves to ${resolvedCommit}, not triggering commit ${triggerCommit}`)
  }
  if (!input.containedInMaster) throw new Error(`${input.tag} commit is not contained in origin/master`)

  const existing = input.releases.find((release) => release.tag_name === input.tag)
  if (existing) {
    const sameRunAndCommit = existing.body?.includes(releaseRunMarker(input.runId, triggerCommit))
    if (!sameRunAndCommit) {
      throw new Error(`${input.tag} already has a release from another commit or workflow run`)
    }
  }

  const latest = latestStableTag(input.tags, input.releases, input.tag)
  assertVersionIsNewer(input.tag, latest)
  return { version, latest }
}

function git(args: string[], allowFailure = false) {
  const result = Bun.spawnSync({ cmd: ["git", ...args], stdout: "pipe", stderr: "pipe" })
  if (!allowFailure && result.exitCode !== 0) {
    throw new Error(result.stderr.toString().trim() || `git ${args.join(" ")} failed`)
  }
  return { exitCode: result.exitCode, output: result.stdout.toString().trim() }
}

async function githubReleases(repository: string, token: string) {
  const releases: GitHubRelease[] = []
  for (let page = 1; ; page++) {
    const response = await fetch(`https://api.github.com/repos/${repository}/releases?per_page=100&page=${page}`, {
      headers: {
        Accept: "application/vnd.github+json",
        Authorization: `Bearer ${token}`,
        "X-GitHub-Api-Version": "2022-11-28",
      },
    })
    if (!response.ok) throw new Error(`GitHub releases API ${response.status}: ${await response.text()}`)
    const pageReleases = await response.json() as GitHubRelease[]
    releases.push(...pageReleases)
    if (pageReleases.length < 100) return releases
  }
}

function replaceRequired(content: string, pattern: RegExp, replacement: string, file: string) {
  if (!pattern.test(content)) throw new Error(`Could not stamp version in ${file}`)
  return content.replace(pattern, replacement)
}

export function stampReleaseVersion(tag: string, root = path.resolve(import.meta.dirname, "..")) {
  const version = versionFromTag(tag)
  for (const relative of ["package.json", "src-tauri/tauri.conf.json"]) {
    const file = path.join(root, relative)
    const contents = JSON.parse(readFileSync(file, "utf8")) as { version: string }
    contents.version = version
    writeFileSync(file, `${JSON.stringify(contents, null, 2)}\n`)
  }

  const cargoManifest = path.join(root, "src-tauri/Cargo.toml")
  writeFileSync(cargoManifest, replaceRequired(
    readFileSync(cargoManifest, "utf8"),
    /(^\[package\][\s\S]*?^version = ")[^"]+("$)/m,
    `$1${version}$2`,
    cargoManifest,
  ))

  const cargoLock = path.join(root, "src-tauri/Cargo.lock")
  writeFileSync(cargoLock, replaceRequired(
    readFileSync(cargoLock, "utf8"),
    /(^\[\[package\]\]\r?\nname = "drift"\r?\nversion = ")[^"]+("$)/m,
    `$1${version}$2`,
    cargoLock,
  ))
  return version
}

async function checkPolicy(tag: string, triggerCommit: string, runId: string) {
  const repository = process.env.GITHUB_REPOSITORY
  const token = process.env.GITHUB_TOKEN
  if (!repository || !token || !triggerCommit || !runId) throw new Error("Missing GitHub release environment")

  versionFromTag(tag)
  const commit = git(["rev-parse", `${tag}^{commit}`]).output
  const containedInMaster = git(["merge-base", "--is-ancestor", commit, "refs/remotes/origin/master"], true).exitCode === 0
  const tags = git(["tag", "--list"]).output.split("\n").filter(Boolean)
  const result = validateReleasePolicy({
    tag,
    triggerCommit,
    resolvedCommit: commit,
    runId,
    containedInMaster,
    tags,
    releases: await githubReleases(repository, token),
  })

  const output = process.env.GITHUB_OUTPUT
  if (output) appendFileSync(output, `version=${result.version}\ncommit=${commit}\n`)
  console.log(`${tag} (${commit}) is eligible; previous stable tag is ${result.latest ?? "none"}`)
}

if (import.meta.main) {
  const [command, tag, triggerCommit, runId] = process.argv.slice(2)
  if (!tag) throw new Error("Usage: bun scripts/release-policy.ts <check|stamp> <tag>")
  if (command === "check") await checkPolicy(tag, triggerCommit, runId)
  else if (command === "stamp") console.log(`Stamped release version ${stampReleaseVersion(tag)}`)
  else throw new Error(`Unknown release policy command: ${command}`)
}
