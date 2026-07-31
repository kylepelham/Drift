import { blockSamples, sampleRate } from "./audio"

const processorName = "drift-capture"
// Loaded from a blob so the worklet needs no separate build output.
const processorSource = `
class DriftCapture extends AudioWorkletProcessor {
  process(inputs) {
    const channel = inputs[0] && inputs[0][0]
    if (channel && channel.length) this.port.postMessage(new Float32Array(channel))
    return true
  }
}
registerProcessor(${JSON.stringify(processorName)}, DriftCapture)
`

export type Capture = { stop: () => Promise<void> }

export function captureUnsupported() {
  return Object.assign(new Error("capture unsupported"), { name: "NotSupportedError" })
}

export async function startCapture(onBlock: (block: Float32Array) => void): Promise<Capture> {
  const media = globalThis.navigator?.mediaDevices
  if (!media?.getUserMedia || typeof AudioContext === "undefined") throw captureUnsupported()
  const stream = await media.getUserMedia({
    audio: { echoCancellation: true, noiseSuppression: true, autoGainControl: true },
  })
  // Whisper wants 16 kHz, so the graph runs at that rate instead of resampling afterwards.
  const context = new AudioContext({ sampleRate })
  const url = URL.createObjectURL(new Blob([processorSource], { type: "application/javascript" }))
  try {
    await context.audioWorklet.addModule(url)
  } catch (cause) {
    await release(context, stream)
    throw cause
  } finally {
    URL.revokeObjectURL(url)
  }

  const source = context.createMediaStreamSource(stream)
  const node = new AudioWorkletNode(context, processorName)
  node.port.onmessage = accumulate(onBlock)
  source.connect(node)
  // The graph is only pulled when it reaches the destination; the node writes no output, so it is silent.
  node.connect(context.destination)
  if (context.state === "suspended") await context.resume()

  return {
    async stop() {
      node.port.onmessage = null
      source.disconnect()
      node.disconnect()
      await release(context, stream)
    },
  }
}

/** The worklet delivers 128 samples at a time; VAD wants uniform blocks. */
function accumulate(onBlock: (block: Float32Array) => void) {
  let buffer = new Float32Array(blockSamples)
  let filled = 0
  return (event: MessageEvent) => {
    const chunk = event.data as Float32Array
    let offset = 0
    while (offset < chunk.length) {
      const take = Math.min(blockSamples - filled, chunk.length - offset)
      buffer.set(chunk.subarray(offset, offset + take), filled)
      filled += take
      offset += take
      if (filled < blockSamples) continue
      onBlock(buffer)
      buffer = new Float32Array(blockSamples)
      filled = 0
    }
  }
}

async function release(context: AudioContext, stream: MediaStream) {
  for (const track of stream.getTracks()) track.stop()
  if (context.state !== "closed") await context.close().catch(() => undefined)
}
