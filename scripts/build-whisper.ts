import { $ } from "bun"
import { cpSync, existsSync, mkdirSync, readdirSync, rmSync } from "node:fs"
import path from "node:path"

const root = path.resolve(import.meta.dirname, "..")
const triple = "x86_64-pc-windows-msvc"
const tag = "v1.9.1"
const workspace = path.join(root, ".build", "whisper.cpp")
const out = path.join(root, "src-tauri", "binaries")
const force = process.argv.includes("--force")

/** glslc compiles the compute shaders, so the SDK is a build-time requirement for GPU support. */
function vulkanSdk() {
  const candidates = [process.env.VULKAN_SDK, ...versionedSdks()].filter(Boolean) as string[]
  return candidates.find((candidate) => existsSync(path.join(candidate, "Bin", "glslc.exe")))
}

function versionedSdks() {
  const base = "C:\\VulkanSDK"
  if (!existsSync(base)) return []
  return readdirSync(base)
    .sort()
    .reverse()
    .map((version) => path.join(base, version))
}

async function build(name: string, dir: string, sdk?: string) {
  const target = path.join(out, `${name}-${triple}.exe`)
  if (existsSync(target) && !force) {
    console.log(`${name} already built; pass --force to rebuild`)
    return
  }
  if (force) rmSync(path.join(workspace, dir), { recursive: true, force: true })
  const backend = sdk ? ["-DGGML_VULKAN=ON"] : []
  // Static linking keeps each sidecar a single file, and NATIVE off keeps it runnable on any x64 machine.
  await $`cmake -B ${dir} -S . -DCMAKE_BUILD_TYPE=Release -DBUILD_SHARED_LIBS=OFF -DGGML_NATIVE=OFF -DWHISPER_BUILD_TESTS=OFF -DWHISPER_BUILD_SERVER=OFF -DWHISPER_BUILD_EXAMPLES=ON ${backend}`
    .cwd(workspace)
    .env({ ...process.env, ...(sdk ? { VULKAN_SDK: sdk } : {}) })
  await $`cmake --build ${dir} --config Release --target whisper-cli --parallel`.cwd(workspace)

  const built = [
    path.join(workspace, dir, "bin", "Release", "whisper-cli.exe"),
    path.join(workspace, dir, "bin", "whisper-cli.exe"),
  ].find(existsSync)
  if (!built) throw new Error(`whisper-cli.exe was not produced by the ${name} build`)
  mkdirSync(out, { recursive: true })
  cpSync(built, target)
  cpSync(built, path.join(out, `${name}.exe`))
  console.log(`${name} ready in src-tauri/binaries`)
}

if (!existsSync(path.join(workspace, "CMakeLists.txt"))) {
  mkdirSync(path.dirname(workspace), { recursive: true })
  await $`git clone --depth 1 --branch ${tag} https://github.com/ggml-org/whisper.cpp.git ${workspace}`
}

// The CPU build is the fallback for machines with no Vulkan driver, where the GPU binary cannot load.
await build("whisper-cli", "build-cpu")

const sdk = vulkanSdk()
if (sdk) {
  console.log(`building the Vulkan sidecar from ${sdk}`)
  await build("whisper-cli-vulkan", "build-vulkan", sdk)
} else if (process.env.CI) {
  // Bundling declares the GPU sidecar, so a release without it would fail later and less clearly.
  throw new Error("Vulkan SDK not found. CI must install it before building the speech recognizer.")
} else {
  console.warn("Vulkan SDK not found: skipping the GPU sidecar. Install it to get GPU acceleration.")
}
