import { blockSamples, sampleRate } from "./audio"
import { refreshAudioInputDevices } from "./devices"

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

export function captureConstraints(deviceId?: string): MediaStreamConstraints {
  return {
    audio: {
      echoCancellation: true,
      noiseSuppression: true,
      autoGainControl: true,
      ...(deviceId ? { deviceId: { exact: deviceId } } : {}),
    },
  }
}

export async function startCapture(onBlock: (block: Float32Array) => void, deviceId?: string): Promise<Capture> {
  const media = globalThis.navigator?.mediaDevices
  if (!media?.getUserMedia || typeof AudioContext === "undefined") throw captureUnsupported()
  let stream: MediaStream
  try {
    stream = await media.getUserMedia(captureConstraints(deviceId))
  } catch (cause) {
    if (!deviceId || !deviceUnavailable(cause)) throw cause
    void refreshAudioInputDevices(media)
    stream = await media.getUserMedia(captureConstraints())
  }
  let context: AudioContext
  try {
    // Whisper wants 16 kHz, so the graph runs at that rate instead of resampling afterwards.
    context = new AudioContext({ sampleRate })
  } catch (cause) {
    stopStreamTracks(stream)
    throw cause
  }
  let url = ""
  try {
    url = URL.createObjectURL(new Blob([processorSource], { type: "application/javascript" }))
    await context.audioWorklet.addModule(url)
  } catch (cause) {
    await release(context, stream)
    throw cause
  } finally {
    if (url) URL.revokeObjectURL(url)
  }

  let source: MediaStreamAudioSourceNode
  let node: AudioWorkletNode
  try {
    source = context.createMediaStreamSource(stream)
    node = new AudioWorkletNode(context, processorName)
    node.port.onmessage = accumulate(onBlock)
    source.connect(node)
    // The graph is only pulled when it reaches the destination; the node writes no output, so it is silent.
    node.connect(context.destination)
    if (context.state === "suspended") await context.resume()
  } catch (cause) {
    await release(context, stream)
    throw cause
  }

  return {
    async stop() {
      node.port.onmessage = null
      try {
        source.disconnect()
      } finally {
        try {
          node.disconnect()
        } finally {
          await release(context, stream)
        }
      }
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
  stopStreamTracks(stream)
  if (context.state !== "closed") await context.close().catch(() => undefined)
}

export function stopStreamTracks(stream: Pick<MediaStream, "getTracks">) {
  for (const track of stream.getTracks()) track.stop()
}

export function deviceUnavailable(cause: unknown) {
  const name = cause instanceof Error ? cause.name : ""
  return name === "NotFoundError" || name === "OverconstrainedError"
}
