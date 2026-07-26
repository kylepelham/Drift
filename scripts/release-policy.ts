import { createHash } from "node:crypto"
import { appendFileSync, readFileSync, readdirSync, writeFileSync } from "node:fs"
import path from "node:path"

export type GitHubReleaseAsset = {
  name?: string
  state?: string
  size?: number
  digest?: string | null
}

export type GitHubRelease = {
  tag_name?: string
  body?: string | null
  draft?: boolean
  prerelease?: boolean
  assets?: GitHubReleaseAsset[]
}

export const stableTagPattern = /^v(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)$/

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

type ReleaseAssetDigest = { name: string; digest: string }

const sha256Pattern = /^sha256:[0-9a-f]{64}$/
const releaseAssetsMarkerPattern = /<!-- drift-release-assets: ([A-Za-z0-9_-]+) -->/g

export function releaseAssetsMarker(assets: ReleaseAssetDigest[]) {
  const sorted = [...assets].sort((left, right) => left.name.localeCompare(right.name))
  if (new Set(sorted.map((asset) => asset.name)).size !== sorted.length) {
    throw new Error("Release asset manifest contains duplicate names")
  }
  for (const asset of sorted) {
    if (!asset.name || !sha256Pattern.test(asset.digest)) {
      throw new Error(`Invalid release asset manifest entry for ${asset.name || "unnamed asset"}`)
    }
  }
  return `<!-- drift-release-assets: ${Buffer.from(JSON.stringify(sorted)).toString("base64url")} -->`
}

function expectedReleaseAssetNames(assets: { name: string }[]) {
  const installers = assets.filter((asset) => asset.name.endsWith("-setup.exe"))
  if (installers.length !== 1) throw new Error("Published release must contain exactly one setup executable")
  return [installers[0].name, `${installers[0].name}.sig`, "latest.json"].sort()
}

export function validatePublishedRelease(release: GitHubRelease, runId: string, commit: string) {
  if (release.draft || release.prerelease) throw new Error("Existing stable release is draft or prerelease")

  const body = release.body ?? ""
  const runMarker = releaseRunMarker(runId, commit)
  if (body.split(runMarker).length !== 2) {
    throw new Error(`${release.tag_name} already has a release from another commit or workflow run`)
  }

  const markerMatches = [...body.matchAll(releaseAssetsMarkerPattern)]
  if (markerMatches.length !== 1) throw new Error("Published release must contain exactly one immutable asset manifest")

  let manifest: ReleaseAssetDigest[]
  try {
    manifest = JSON.parse(Buffer.from(markerMatches[0][1], "base64url").toString("utf8"))
  } catch {
    throw new Error("Published release has an invalid immutable asset manifest")
  }
  if (!Array.isArray(manifest)) throw new Error("Published release has an invalid immutable asset manifest")
  releaseAssetsMarker(manifest)

  const assets = (release.assets ?? []).map((asset) => ({
    name: asset.name ?? "",
    state: asset.state,
    size: asset.size,
    digest: asset.digest?.toLowerCase() ?? "",
  }))
  const expectedNames = expectedReleaseAssetNames(assets)
  const actualNames = assets.map((asset) => asset.name).sort()
  if (actualNames.length !== expectedNames.length || actualNames.some((name, index) => name !== expectedNames[index])) {
    throw new Error("Published release assets do not match the required installer, signature, and update manifest")
  }

  const digests = new Map(manifest.map((asset) => [asset.name, asset.digest]))
  for (const asset of assets) {
    if (asset.state !== "uploaded" || !asset.size || !sha256Pattern.test(asset.digest)) {
      throw new Error(`Published release asset ${asset.name} is incomplete or missing its SHA-256 digest`)
    }
    if (digests.get(asset.name) !== asset.digest) {
      throw new Error(`Published release asset ${asset.name} does not match its immutable digest`)
    }
  }
  if (digests.size !== assets.length) throw new Error("Published release asset manifest does not match published assets")
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
    validatePublishedRelease(existing, input.runId, triggerCommit)
    return { version, latest: latestStableTag(input.tags, input.releases, input.tag), published: true }
  }

  const latest = latestStableTag(input.tags, input.releases, input.tag)
  assertVersionIsNewer(input.tag, latest)
  return { version, latest, published: false }
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

export function sealReleaseNotes(notesFile: string, runId: string, commit: string, assetsDirectory: string) {
  const assetFiles = readdirSync(assetsDirectory)
    .filter((name) => name.endsWith("-setup.exe") || name.endsWith(".sig") || name === "latest.json")
    .map((name) => path.join(assetsDirectory, name))
  expectedReleaseAssetNames(assetFiles.map((file) => ({ name: path.basename(file) })))
  if (assetFiles.length !== 3) throw new Error("Release notes must be sealed with exactly three release assets")

  const assets = assetFiles.map((file) => ({
    name: path.basename(file),
    digest: `sha256:${createHash("sha256").update(readFileSync(file)).digest("hex")}`,
  }))
  appendFileSync(notesFile, `\n${releaseRunMarker(runId, commit)}\n${releaseAssetsMarker(assets)}\n`)
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
  if (output) appendFileSync(output, `version=${result.version}\ncommit=${commit}\npublished=${result.published}\n`)
  if (result.published) console.log(`${tag} (${commit}) is already published with verified immutable assets`)
  else console.log(`${tag} (${commit}) is eligible; previous stable tag is ${result.latest ?? "none"}`)
}

if (import.meta.main) {
  const [command, ...args] = process.argv.slice(2)
  if (command === "check" && args.length === 3) await checkPolicy(args[0], args[1], args[2])
  else if (command === "stamp" && args.length === 1) console.log(`Stamped release version ${stampReleaseVersion(args[0])}`)
  else if (command === "seal" && args.length === 4) sealReleaseNotes(args[0], args[1], args[2], args[3])
  else throw new Error(`Unknown release policy command: ${command}`)
}
