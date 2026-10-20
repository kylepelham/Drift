//! A classic WebDriver session over HTTP, as IEDriverServer speaks it: one command at a time, each against the
//! window and frame the session is switched to.

use std::time::Duration;

use reqwest::Method;
use serde_json::{Value, json};

/// IE mode is slow to answer; a page load alone may take most of this.
const CALL_TIMEOUT: Duration = Duration::from_secs(90);

pub(crate) struct Session {
    http: reqwest::Client,
    /// `http://127.0.0.1:<port>/session/<id>`.
    base: String,
}

#[derive(Debug)]
pub(crate) struct WdError {
    /// WebDriver's error code, such as `no such alert` or `unexpected alert open`.
    pub(crate) code: String,
    pub(crate) message: String,
}

impl std::fmt::Display for WdError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let first = self.message.lines().next().unwrap_or(&self.message);
        write!(formatter, "{first}")
    }
}

impl From<WdError> for String {
    fn from(error: WdError) -> Self {
        error.to_string()
    }
}

fn client() -> reqwest::Client {
    // IEDriverServer listens on IPv4 only and answers plain HTTP; no proxy may stand between.
    reqwest::Client::builder()
        .no_proxy()
        .timeout(CALL_TIMEOUT)
        .build()
        .unwrap_or_default()
}

/// Waits until a driver at `server` answers its status.
pub(crate) async fn wait_ready(server: &str, limit: Duration) -> Result<(), String> {
    let http = client();
    let deadline = tokio::time::Instant::now() + limit;
    loop {
        let answered = http.get(format!("{server}/status")).send().await;
        if answered.is_ok_and(|response| response.status().is_success()) {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            return Err("IEDriverServer did not start answering in time".into());
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

impl Session {
    pub(crate) async fn create(server: &str, capabilities: Value) -> Result<Self, String> {
        let http = client();
        let response = http
            .post(format!("{server}/session"))
            .json(&json!({ "capabilities": capabilities }))
            .send()
            .await
            .map_err(|error| format!("the driver did not answer: {error}"))?;
        let body: Value = response.json().await.map_err(|error| error.to_string())?;
        let Some(id) = body["value"]["sessionId"].as_str() else {
            let message = body["value"]["message"].as_str().unwrap_or("no session was made");
            return Err(message.lines().next().unwrap_or(message).to_string());
        };

        Ok(Self {
            base: format!("{server}/session/{id}"),
            http,
        })
    }

    pub(crate) async fn call(&self, method: Method, path: &str, body: Option<Value>) -> Result<Value, WdError> {
        let mut request = self.http.request(method.clone(), format!("{}{path}", self.base));
        if method == Method::POST {
            request = request.json(&body.unwrap_or_else(|| json!({})));
        }
        let response = request.send().await.map_err(|error| WdError {
            code: "unknown error".into(),
            message: format!("the driver did not answer: {error}"),
        })?;

        let ok = response.status().is_success();
        let body: Value = response.json().await.unwrap_or_default();
        if ok {
            return Ok(body["value"].clone());
        }

        Err(WdError {
            code: body["value"]["error"].as_str().unwrap_or("unknown error").to_string(),
            message: body["value"]["message"]
                .as_str()
                .unwrap_or("the command failed")
                .to_string(),
        })
    }

    pub(crate) async fn get(&self, path: &str) -> Result<Value, WdError> {
        self.call(Method::GET, path, None).await
    }

    pub(crate) async fn post(&self, path: &str, body: Value) -> Result<Value, WdError> {
        self.call(Method::POST, path, Some(body)).await
    }

    /// Runs a script's body (`return` gives its value) with `args` in the current window and frame.
    pub(crate) async fn execute(&self, script: &str, args: Value) -> Result<Value, WdError> {
        self.post("/execute/sync", json!({ "script": script, "args": args }))
            .await
    }

    pub(crate) async fn end(&self) {
        let _ = self.call(Method::DELETE, "", None).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_error_shows_its_first_line() {
        let error = WdError {
            code: "no such alert".into(),
            message: "No alert is active\nBuild info: ...".into(),
        };

        assert_eq!(String::from(error), "No alert is active");
    }
}
