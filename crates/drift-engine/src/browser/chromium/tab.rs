//! The tool's actions on a Chrome or Edge page, through the helpers `page.js` installs out of the page's sight.

use std::sync::atomic::Ordering;

use async_trait::async_trait;
use serde_json::{Value, json};

use super::page::Page;
use crate::browser::PageRecord;
use crate::browser::driver::{Language, Settled, Shot, Size, Tab, Target};
use crate::browser::input::{Key, parse_chord};

impl Page {
    /// Where to click: an element's centre, scrolled into view, or a screenshot point in CSS pixels.
    async fn point(&self, target: Target) -> Result<(f64, f64, String), String> {
        match target {
            Target::Ref(reference) => {
                let point = self.helper(&format!("__drift.point({reference})")).await?;
                let x = point["x"].as_f64().ok_or("the element has no position")?;
                let y = point["y"].as_f64().ok_or("the element has no position")?;
                Ok((x, y, format!("ref {reference}")))
            }
            Target::Point(x, y) => {
                let (css_x, css_y) = self.to_css(x, y);
                Ok((css_x, css_y, format!("{x},{y}")))
            }
        }
    }
}

#[async_trait]
impl Tab for Page {
    fn id(&self) -> String {
        self.target.clone()
    }

    fn record(&self, active: bool) -> PageRecord {
        self.page_record(active)
    }

    fn url(&self) -> String {
        self.current_url()
    }

    fn dialog(&self) -> Option<crate::browser::DialogRecord> {
        self.current_dialog()
    }

    async fn snapshot(&self, interactive: bool) -> Result<String, String> {
        let options = json!({ "interactive": interactive });
        let tree = self.helper(&format!("__drift.snapshot({options})")).await?;

        Ok(tree.as_str().unwrap_or_default().to_string())
    }

    async fn text(&self) -> Result<String, String> {
        let shown = self.helper("__drift.text()").await?;

        Ok(shown.as_str().unwrap_or_default().to_string())
    }

    async fn click(&self, target: Target, double: bool) -> Result<String, String> {
        let (x, y, what) = self.point(target).await?;
        self.click_at(x, y, double).await.map_err(|error| error.to_string())?;

        Ok(format!("Clicked {what}."))
    }

    async fn hover(&self, target: Target) -> Result<String, String> {
        let (x, y, what) = self.point(target).await?;
        self.mouse("mouseMoved", x, y, 0)
            .await
            .map_err(|error| error.to_string())?;

        Ok(format!("Hovered {what}."))
    }

    async fn fill(&self, reference: u64, text: &str, checked: Option<bool>) -> Result<String, String> {
        let checked = checked.map_or("undefined".to_string(), |checked| checked.to_string());
        let call = format!("__drift.prepare({reference}, {}, {checked})", json!(text));
        let prepared = self.helper(&call).await?;

        if let Some(done) = prepared["done"].as_str() {
            return Ok(format!("Ref {reference}: {done}."));
        }
        if let Some(point) = prepared.get("click") {
            let x = point["x"].as_f64().unwrap_or_default();
            let y = point["y"].as_f64().unwrap_or_default();
            self.click_at(x, y, false).await.map_err(|error| error.to_string())?;
            return Ok(format!("Clicked ref {reference} to change it."));
        }

        // The field's text is selected, so typing replaces it; an empty value deletes it.
        if text.is_empty() {
            let (delete, _) = parse_chord("Delete")?;
            self.press_key(&delete, 0).await.map_err(|error| error.to_string())?;
            return Ok(format!("Cleared ref {reference}."));
        }
        self.insert_text(text).await.map_err(|error| error.to_string())?;

        Ok(format!("Filled ref {reference}."))
    }

    async fn scroll(&self, reference: Option<u64>, dy: f64) -> Result<String, String> {
        if let Some(reference) = reference {
            self.helper(&format!("__drift.scroll({reference})")).await?;
            return Ok(format!("Scrolled ref {reference} into view."));
        }

        self.wheel(400.0, 300.0, dy).await.map_err(|error| error.to_string())?;

        Ok(format!("Scrolled {dy} pixels."))
    }

    async fn press(&self, key: &Key, modifiers: u8) -> Result<(), String> {
        self.press_key(key, modifiers).await.map_err(|error| error.to_string())
    }

    async fn type_text(&self, text: &str) -> Result<(), String> {
        self.insert_text(text).await.map_err(|error| error.to_string())
    }

    async fn go(&self, url: &str) -> Result<(), String> {
        self.go_to(url).await
    }

    async fn settle(&self, url_before: &str) -> Settled {
        self.settle_after(url_before).await
    }

    async fn screenshot(&self) -> Result<Shot, String> {
        self.capture().await
    }

    async fn zoom(&self, region: [f64; 4]) -> Result<Shot, String> {
        self.zoom_into(region).await
    }

    async fn evaluate(&self, script: &str, language: Language) -> Result<Value, String> {
        if language == Language::VbScript {
            return Err("VBScript runs only in IE mode".into());
        }

        self.evaluate_page(script).await
    }

    fn logs(&self, network: bool, matches: &dyn Fn(&str) -> bool, limit: usize) -> Result<Vec<String>, String> {
        Ok(self.log_lines(network, matches, limit))
    }

    async fn response_body(&self, request: &str) -> Result<String, String> {
        self.body_of(request).await
    }

    async fn answer_dialog(&self, accept: bool, text: Option<&str>) -> Result<(), String> {
        self.answer(accept, text).await
    }

    /// A phone's size also brings touch and a phone's user agent; pages decide on both as they load, so it reloads.
    async fn viewport(&self, size: Option<Size>) -> Result<(), String> {
        let call = |method: &'static str, params: Value| async move {
            self.call(method, params).await.map_err(|error| error.to_string())
        };

        let Some(size) = size else {
            // The pane's size takes over again from the one the agent chose.
            self.agent_sized.store(false, Ordering::SeqCst);
            if !self.fit_pane() {
                call("Emulation.clearDeviceMetricsOverride", json!({})).await?;
            }
            call("Emulation.setTouchEmulationEnabled", json!({ "enabled": false })).await?;
            let desktop = self
                .agents
                .desktop
                .clone()
                .unwrap_or_else(|| json!({ "userAgent": "" }));
            call("Emulation.setUserAgentOverride", desktop).await?;
            return self.reload().await;
        };

        let scale = if size.mobile { 2 } else { 1 };
        let metrics = json!({
            "width": size.width,
            "height": size.height,
            "deviceScaleFactor": scale,
            "mobile": size.mobile,
        });
        self.agent_sized.store(true, Ordering::SeqCst);
        call("Emulation.setDeviceMetricsOverride", metrics).await?;
        let touch = json!({ "enabled": size.mobile, "maxTouchPoints": 5 });
        call("Emulation.setTouchEmulationEnabled", touch).await?;
        if size.mobile {
            call("Emulation.setUserAgentOverride", self.agents.mobile.clone()).await?;
        }

        self.reload().await
    }

    async fn start_trace(&self, throttle: bool) -> Result<(), String> {
        self.start_tracing(throttle).await
    }

    async fn stop_trace(&self) -> Result<Vec<u8>, String> {
        self.stop_tracing().await
    }
}
