declare const __DRIFT_VERSION__: string

interface Window {
  __DRIFT_PRELOAD_READY__?: Promise<void>
}
