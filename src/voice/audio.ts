/** Samples per VAD block. 512 at 16 kHz is 32ms, short enough to react and long enough to be stable. */
export const blockSamples = 512
export const sampleRate = 16_000

export type SegmenterConfig = {
  /** RMS above which a block counts as speech. */
  threshold: number
  /** Silent blocks tolerated before a phrase is considered finished. */
  hangoverBlocks: number
  /** Voiced blocks required before a phrase is real rather than a keyboard knock. */
  minVoicedBlocks: number
  /** Blocks of audio kept ahead of speech so the first word is not clipped. */
  prerollBlocks: number
  /** Hard ceiling so a monologue is transcribed in pieces rather than held forever. */
  maxBlocks: number
}

export const defaultSegmenterConfig: SegmenterConfig = {
  threshold: 0.012,
  hangoverBlocks: 22,
  minVoicedBlocks: 8,
  prerollBlocks: 10,
  maxBlocks: 780,
}

export type SegmenterState = {
  speaking: boolean
  voiced: number
  silent: number
  blocks: Float32Array[]
}

export function createSegmenter(): SegmenterState {
  return { speaking: false, voiced: 0, silent: 0, blocks: [] }
}

export function blockEnergy(block: Float32Array) {
  if (!block.length) return 0
  let total = 0
  for (const sample of block) total += sample * sample
  return Math.sqrt(total / block.length)
}

/**
 * Feeds one block in and returns a finished phrase when speech has stopped. Silence never produces
 * a phrase, which is what keeps whisper from inventing text during a pause.
 */
export function pushBlock(state: SegmenterState, block: Float32Array, config = defaultSegmenterConfig) {
  const voiced = blockEnergy(block) >= config.threshold
  state.blocks.push(block)
  if (!state.speaking) return openPhrase(state, voiced, config)
  state.silent = voiced ? 0 : state.silent + 1
  if (state.silent < config.hangoverBlocks && state.blocks.length < config.maxBlocks) return undefined
  return closePhrase(state, config)
}

/** Flushes whatever speech is buffered, for when the user stops recording mid-phrase. */
export function drainSegmenter(state: SegmenterState, config = defaultSegmenterConfig) {
  if (!state.speaking) {
    reset(state)
    return undefined
  }
  return closePhrase(state, config)
}

function openPhrase(state: SegmenterState, voiced: boolean, config: SegmenterConfig) {
  if (!voiced) {
    state.voiced = 0
    if (state.blocks.length > config.prerollBlocks) state.blocks.shift()
    return undefined
  }
  state.voiced += 1
  if (state.voiced >= config.minVoicedBlocks) {
    state.speaking = true
    state.silent = 0
  }
  return undefined
}

function closePhrase(state: SegmenterState, config: SegmenterConfig) {
  const spoken = state.blocks.length > config.hangoverBlocks ? state.blocks.slice(0, -config.hangoverBlocks) : state.blocks
  const phrase = concatBlocks(spoken)
  reset(state)
  return phrase.length ? phrase : undefined
}

function reset(state: SegmenterState) {
  state.speaking = false
  state.voiced = 0
  state.silent = 0
  state.blocks = []
}

export function concatBlocks(blocks: Float32Array[]) {
  const total = blocks.reduce((sum, block) => sum + block.length, 0)
  const merged = new Float32Array(total)
  let offset = 0
  for (const block of blocks) {
    merged.set(block, offset)
    offset += block.length
  }
  return merged
}

/** Converts to the 16-bit mono PCM the sidecar expects, base64 encoded for the command boundary. */
export function encodePcm16(samples: Float32Array) {
  const bytes = new Uint8Array(samples.length * 2)
  const view = new DataView(bytes.buffer)
  for (let index = 0; index < samples.length; index += 1) {
    const clamped = Math.max(-1, Math.min(1, samples[index]!))
    view.setInt16(index * 2, Math.round(clamped * 32767), true)
  }
  return base64(bytes)
}

function base64(bytes: Uint8Array) {
  let binary = ""
  const chunk = 0x8000
  for (let offset = 0; offset < bytes.length; offset += chunk) {
    binary += String.fromCharCode(...bytes.subarray(offset, offset + chunk))
  }
  return btoa(binary)
}

export function phraseSeconds(samples: Float32Array) {
  return samples.length / sampleRate
}
