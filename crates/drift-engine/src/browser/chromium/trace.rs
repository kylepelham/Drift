//! Recording a performance trace of a page through the `Tracing` domain, streamed back gzipped.

use std::sync::atomic::Ordering;
use std::time::Duration;

use base64::Engine as _;
use serde_json::{Value, json};
use tokio::sync::oneshot;

use super::page::Page;

/// What DevTools' Performance panel records, less its filmstrip, which multiplies a trace's size.
const CATEGORIES: &[&str] = &[
    "devtools.timeline",
    "disabled-by-default-devtools.timeline",
    "disabled-by-default-devtools.timeline.frame",
    "disabled-by-default-devtools.timeline.stack",
    "disabled-by-default-v8.cpu_profiler",
    "toplevel",
    "blink.console",
    "blink.user_timing",
    "latencyInfo",
    "loading",
    "v8.execute",
    "v8",
];
/// How long the browser may take to hand over a trace once asked to stop.
const COMPLETE_WAIT: Duration = Duration::from_secs(60);
const CHUNK: u64 = 4 * 1024 * 1024;
/// A mid-range phone on a slow 4G connection, as Lighthouse's mobile run assumes.
const CPU_SLOWDOWN: u64 = 4;
const SLOW_4G: (u64, u64, u64) = (150, 1_600_000 / 8, 750_000 / 8);

impl Page {
    pub(crate) async fn start_tracing(&self, throttle: bool) -> Result<(), String> {
        if self.tracing.swap(true, Ordering::SeqCst) {
            return Err("a trace is already recording on this page; stop it first".into());
        }

        let params = json!({
            "transferMode": "ReturnAsStream",
            "streamFormat": "json",
            "streamCompression": "gzip",
            "traceConfig": {
                "recordMode": "recordAsMuchAsPossible",
                "includedCategories": CATEGORIES,
                "excludedCategories": ["*"],
            },
        });
        self.throttled.store(throttle, Ordering::SeqCst);
        let started = async {
            if throttle {
                self.throttle(true).await?;
            }
            self.call("Tracing.start", params).await
        };
        if let Err(error) = started.await {
            self.tracing.store(false, Ordering::SeqCst);
            self.unthrottle().await;
            return Err(error.to_string());
        }

        Ok(())
    }

    pub(crate) async fn stop_tracing(&self) -> Result<Vec<u8>, String> {
        if !self.tracing.load(Ordering::SeqCst) {
            return Err("no trace is recording on this page; start one with `record: start`".into());
        }

        let (sender, complete) = oneshot::channel();
        *self.trace_complete.lock().unwrap() = Some(sender);
        let ended = self.call("Tracing.end", json!({})).await;
        self.tracing.store(false, Ordering::SeqCst);
        self.unthrottle().await;
        ended.map_err(|error| error.to_string())?;
        let complete = tokio::time::timeout(COMPLETE_WAIT, complete)
            .await
            .map_err(|_| "the browser did not hand over the trace within 60 s")?
            .map_err(|_| "the page closed before its trace was handed over")?;
        let stream = complete["stream"].as_str().ok_or("the browser returned no trace")?;

        let read = self.read_stream(stream).await;
        let _ = self.call("IO.close", json!({ "handle": stream })).await;

        read
    }

    /// Takes in `Tracing.tracingComplete`, which says where the finished trace can be read.
    pub(crate) fn trace_completed(&self, params: &Value) {
        if let Some(sender) = self.trace_complete.lock().unwrap().take() {
            let _ = sender.send(params.clone());
        }
    }

    /// Slows the page's CPU and network for a recording, or puts them back.
    async fn throttle(&self, on: bool) -> Result<Value, super::cdp::CdpError> {
        let (latency, download, upload) = SLOW_4G;
        let rate = if on { CPU_SLOWDOWN } else { 1 };
        let network = if on {
            json!({ "offline": false, "latency": latency, "downloadThroughput": download, "uploadThroughput": upload })
        } else {
            json!({ "offline": false, "latency": 0, "downloadThroughput": -1, "uploadThroughput": -1 })
        };

        self.call("Emulation.setCPUThrottlingRate", json!({ "rate": rate }))
            .await?;
        self.call("Network.emulateNetworkConditions", network).await
    }

    async fn unthrottle(&self) {
        if self.throttled.swap(false, Ordering::SeqCst) {
            let _ = self.throttle(false).await;
        }
    }

    async fn read_stream(&self, stream: &str) -> Result<Vec<u8>, String> {
        let mut bytes = Vec::new();
        loop {
            let chunk = self
                .call("IO.read", json!({ "handle": stream, "size": CHUNK }))
                .await
                .map_err(|error| error.to_string())?;
            let data = chunk["data"].as_str().unwrap_or_default();
            if chunk["base64Encoded"].as_bool() == Some(true) {
                let decoded = base64::engine::general_purpose::STANDARD
                    .decode(data)
                    .map_err(|error| error.to_string())?;
                bytes.extend_from_slice(&decoded);
            } else {
                bytes.extend_from_slice(data.as_bytes());
            }
            if chunk["eof"].as_bool() != Some(false) {
                return Ok(bytes);
            }
        }
    }
}
