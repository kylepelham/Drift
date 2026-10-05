use serde::Serialize;
use serde_json::Value;
use std::sync::OnceLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const DAY_MINUTES: u64 = 24 * 60;
const WEEK_MINUTES: u64 = 7 * DAY_MINUTES;

#[derive(Serialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum WindowKind {
    Session,
    Weekly,
    Monthly,
    Period,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UsageWindow {
    kind: WindowKind,
    label: Option<String>,
    used_percent: f64,
    resets_at: Option<i64>,
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum UsageStatus {
    Ok,
    Expired,
    Unsubscribed,
}

#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProviderUsage {
    status: UsageStatus,
    plan: Option<String>,
    windows: Vec<UsageWindow>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Source {
    Anthropic,
    Codex,
    Zai(&'static str),
    OpenCodeGo,
    Grok,
    Kimi,
    Copilot,
}

#[derive(Debug, PartialEq)]
enum Credential {
    OAuth { access: String, expires: i64, account_id: Option<String>, enterprise: bool },
    Api { key: String },
}

#[derive(Debug, PartialEq)]
struct Request {
    url: &'static str,
    headers: Vec<(&'static str, String)>,
}

fn source(provider: &str) -> Option<Source> {
    Some(match provider {
        "anthropic" => Source::Anthropic,
        "openai" => Source::Codex,
        "zai-coding-plan" => Source::Zai("https://api.z.ai/api/monitor/usage/quota/limit"),
        "zhipuai-coding-plan" => Source::Zai("https://open.bigmodel.cn/api/monitor/usage/quota/limit"),
        "opencode-go" => Source::OpenCodeGo,
        "xai" => Source::Grok,
        "kimi-for-coding" | "kimi-code-plan-global" => Source::Kimi,
        "github-copilot" => Source::Copilot,
        _ => return None,
    })
}

/// The engine's credential as the usage requests need it; a cloud route's own credentials have no plan.
fn from_engine(credential: drift_engine::llm::Credential) -> Option<Credential> {
    match credential {
        drift_engine::llm::Credential::OAuth { access, expires_at, account, .. } => Some(Credential::OAuth { access, expires: expires_at, account_id: account, enterprise: false }),
        drift_engine::llm::Credential::ApiKey { key } => Some(Credential::Api { key }),
        drift_engine::llm::Credential::Ambient { .. } => None,
    }
}

/// Subscription windows exist only for subscription sign-ins; plain API keys have none to report.
fn request(source: Source, credential: &Credential) -> Option<Request> {
    let json = ("Accept", "application/json".to_owned());
    match (source, credential) {
        (Source::Anthropic, Credential::OAuth { access, .. }) => Some(Request {
            url: "https://api.anthropic.com/api/oauth/usage",
            headers: vec![json, bearer(access), ("anthropic-beta", "oauth-2025-04-20".into())],
        }),
        (Source::Codex, Credential::OAuth { access, account_id, .. }) => {
            let mut headers = vec![json, bearer(access)];
            headers.extend(account_id.clone().map(|id| ("ChatGPT-Account-Id", id)));
            Some(Request { url: "https://chatgpt.com/backend-api/wham/usage", headers })
        }
        (Source::Grok, Credential::OAuth { access, .. }) => Some(Request {
            url: "https://cli-chat-proxy.grok.com/v1/billing?format=credits",
            headers: vec![json, bearer(access), ("x-xai-token-auth", "xai-grok-cli".into())],
        }),
        (Source::Copilot, Credential::OAuth { access, enterprise: false, .. }) => Some(Request {
            url: "https://api.github.com/copilot_internal/user",
            headers: vec![json, ("Authorization", format!("token {access}")), ("X-Github-Api-Version", "2025-04-01".into())],
        }),
        (Source::Zai(url), Credential::Api { key }) => Some(Request { url, headers: vec![json, bearer(key)] }),
        (Source::OpenCodeGo, Credential::Api { key }) => {
            Some(Request { url: "https://opencode.ai/zen/go/v1/usage", headers: vec![json, bearer(key)] })
        }
        (Source::Kimi, Credential::Api { key }) => {
            Some(Request { url: "https://api.kimi.com/coding/v1/usages", headers: vec![json, bearer(key)] })
        }
        _ => None,
    }
}

fn bearer(token: &str) -> (&'static str, String) {
    ("Authorization", format!("Bearer {token}"))
}

fn parse(source: Source, body: &Value, now: i64) -> ProviderUsage {
    let (plan, windows) = match source {
        Source::Anthropic => (None, anthropic(body)),
        Source::Codex => (text(body, "plan_type"), codex(body)),
        Source::Zai(_) => (text(&body["data"], "level"), zai(&body["data"])),
        Source::OpenCodeGo => (None, opencode_go(&body["usage"], now)),
        Source::Grok => (text(&body["config"], "subscriptionTier"), grok(&body["config"])),
        Source::Kimi => (None, kimi(body)),
        Source::Copilot => (text(body, "copilot_plan"), copilot(body)),
    };
    ProviderUsage { status: UsageStatus::Ok, plan, windows }
}

fn text(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).filter(|text| !text.is_empty()).map(str::to_owned)
}

fn window(kind: WindowKind, label: Option<String>, used: f64, resets_at: Option<i64>) -> UsageWindow {
    UsageWindow { kind, label, used_percent: used.clamp(0.0, 100.0), resets_at }
}

fn kind_for_minutes(minutes: u64) -> WindowKind {
    match minutes {
        0..=DAY_MINUTES => WindowKind::Session,
        m if m <= WEEK_MINUTES + DAY_MINUTES => WindowKind::Weekly,
        _ => WindowKind::Monthly,
    }
}

/// Epoch milliseconds from ISO 8601, a bare date, epoch seconds, or epoch milliseconds.
fn timestamp(value: &Value) -> Option<i64> {
    if let Some(number) = value.as_f64() {
        return Some(if number > 1e12 { number as i64 } else { (number * 1000.0) as i64 });
    }
    let text = value.as_str()?;
    let full = if text.len() == 10 { format!("{text}T00:00:00Z") } else { text.to_owned() };
    let parsed = OffsetDateTime::parse(&full, &Rfc3339).ok()?;
    Some((parsed.unix_timestamp_nanos() / 1_000_000) as i64)
}

fn anthropic(body: &Value) -> Vec<UsageWindow> {
    let limits: Vec<UsageWindow> = body["limits"].as_array().into_iter().flatten().filter_map(anthropic_limit).collect();
    if !limits.is_empty() {
        return limits;
    }
    [("five_hour", WindowKind::Session), ("seven_day", WindowKind::Weekly)]
        .into_iter()
        .filter_map(|(key, kind)| {
            let entry = body.get(key)?;
            Some(window(kind, None, entry["utilization"].as_f64()?, timestamp(&entry["resets_at"])))
        })
        .collect()
}

/// Model-scoped weekly caps only matter once they are in use, so idle ones are left out.
fn anthropic_limit(limit: &Value) -> Option<UsageWindow> {
    let percent = limit["percent"].as_f64()?;
    let resets_at = timestamp(&limit["resets_at"]);
    match limit["kind"].as_str()? {
        "session" => Some(window(WindowKind::Session, None, percent, resets_at)),
        "weekly_all" => Some(window(WindowKind::Weekly, None, percent, resets_at)),
        "weekly_scoped" if percent > 0.0 => {
            Some(window(WindowKind::Weekly, text(&limit["scope"]["model"], "display_name"), percent, resets_at))
        }
        _ => None,
    }
}

fn codex(body: &Value) -> Vec<UsageWindow> {
    ["primary_window", "secondary_window"]
        .into_iter()
        .filter_map(|key| {
            let entry = body["rate_limit"].get(key).filter(|entry| entry.is_object())?;
            let minutes = entry["limit_window_seconds"].as_u64().unwrap_or(0) / 60;
            Some(window(kind_for_minutes(minutes), None, entry["used_percent"].as_f64()?, timestamp(&entry["reset_at"])))
        })
        .collect()
}

/// z.ai `unit` codes: 1 day, 3 hour, 5 minute, 6 week. TIME_LIMIT is the MCP tool quota, not coding usage.
fn zai(data: &Value) -> Vec<UsageWindow> {
    data["limits"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|limit| matches!(limit["type"].as_str(), Some("TOKENS_LIMIT" | "CREDIT_LIMIT")))
        .filter_map(|limit| {
            let unit = match limit["unit"].as_u64()? {
                1 => DAY_MINUTES,
                3 => 60,
                5 => 1,
                6 => WEEK_MINUTES,
                _ => return None,
            };
            let minutes = unit * limit["number"].as_u64().unwrap_or(1);
            Some(window(kind_for_minutes(minutes), None, limit["percentage"].as_f64()?, timestamp(&limit["nextResetTime"])))
        })
        .collect()
}

fn opencode_go(usage: &Value, now: i64) -> Vec<UsageWindow> {
    [("rolling", WindowKind::Session), ("weekly", WindowKind::Weekly), ("monthly", WindowKind::Monthly)]
        .into_iter()
        .filter_map(|(key, kind)| {
            let entry = usage.get(key)?;
            let used = first(entry, &["usagePercent", "usedPercent", "percent", "used_percent", "utilization"])?.as_f64()?;
            let relative = first(entry, &["resetInSec", "resetInSeconds", "reset_in_sec", "resetSec"])
                .and_then(Value::as_f64)
                .map(|seconds| now + (seconds * 1000.0) as i64);
            let absolute = first(entry, &["resetAt", "resetsAt", "reset_at", "resets_at"]).and_then(timestamp);
            Some(window(kind, None, used, relative.or(absolute)))
        })
        .collect()
}

fn first<'a>(value: &'a Value, keys: &[&str]) -> Option<&'a Value> {
    keys.iter().find_map(|key| value.get(*key).filter(|found| !found.is_null()))
}

fn grok(config: &Value) -> Vec<UsageWindow> {
    let direct = config["creditUsagePercent"].as_f64();
    let derived = config["onDemandCap"]["val"]
        .as_f64()
        .filter(|cap| *cap > 0.0)
        .and_then(|cap| Some(config["onDemandUsed"]["val"].as_f64()? / cap * 100.0));
    let Some(used) = direct.or(derived) else { return Vec::new() };
    let resets_at = timestamp(&config["currentPeriod"]["end"]).or_else(|| timestamp(&config["billingPeriodEnd"]));
    let kind = match config["currentPeriod"]["type"].as_str().unwrap_or_default() {
        period if period.contains("WEEKLY") => WindowKind::Weekly,
        period if period.contains("MONTHLY") => WindowKind::Monthly,
        _ => WindowKind::Period,
    };
    vec![window(kind, None, used, resets_at)]
}

/// Kimi's ratio pools can read zero while its counters move, so counters win when present.
fn kimi(body: &Value) -> Vec<UsageWindow> {
    let pool = |key: &str, kind| {
        let entry = body["usages"].get(key)?;
        Some(window(kind, None, entry["used_ratio"].as_f64()? * 100.0, timestamp(&entry["reset_time"])))
    };
    let session = counted(&body["limits"][0]["detail"], WindowKind::Session).or_else(|| pool("limit_5h", WindowKind::Session));
    let weekly = counted(&body["usage"], WindowKind::Weekly).or_else(|| pool("limit_7d", WindowKind::Weekly));
    let monthly = pool("limit_month_total", WindowKind::Monthly);
    [session, weekly, monthly].into_iter().flatten().collect()
}

fn counted(detail: &Value, kind: WindowKind) -> Option<UsageWindow> {
    let number = |key: &str| detail.get(key).and_then(|value| value.as_f64().or_else(|| value.as_str()?.parse().ok()));
    let limit = number("limit").filter(|limit| *limit > 0.0)?;
    let used = number("used").or_else(|| Some(limit - number("remaining")?))?;
    let resets_at = first(detail, &["resetTime", "resetAt", "reset_time", "reset_at"]).and_then(timestamp);
    Some(window(kind, None, used / limit * 100.0, resets_at))
}

fn copilot(body: &Value) -> Vec<UsageWindow> {
    let resets_at = timestamp(&body["quota_reset_date"]);
    [("premium_interactions", "premium"), ("chat", "chat")]
        .into_iter()
        .filter_map(|(key, label)| {
            let quota = body["quota_snapshots"].get(key).filter(|quota| quota["unlimited"] != Value::Bool(true))?;
            let entitlement = quota["entitlement"].as_f64().filter(|value| *value > 0.0);
            let remaining = quota["percent_remaining"]
                .as_f64()
                .or_else(|| Some(quota["remaining"].as_f64()? / entitlement? * 100.0))?;
            Some(window(WindowKind::Monthly, Some(label.into()), 100.0 - remaining, resets_at))
        })
        .collect()
}

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|elapsed| elapsed.as_millis() as i64).unwrap_or(0)
}

fn client() -> Result<&'static reqwest::Client, String> {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    if let Some(client) = CLIENT.get() {
        return Ok(client);
    }
    let built = reqwest::Client::builder().timeout(REQUEST_TIMEOUT).build().map_err(|error| error.to_string())?;
    Ok(CLIENT.get_or_init(|| built))
}

fn signed_out(status: UsageStatus) -> ProviderUsage {
    ProviderUsage { status, plan: None, windows: Vec::new() }
}

fn failure_status(code: u16, body: &str) -> Option<UsageStatus> {
    match code {
        401 => Some(UsageStatus::Expired),
        403 if body.to_lowercase().contains("subscription") => Some(UsageStatus::Unsubscribed),
        403 => Some(UsageStatus::Expired),
        _ => None,
    }
}

/// Plan usage for the provider behind the current model, with the sign-in the engine holds (renewed
/// first when it has expired). Credentials stay on this side of the bridge.
#[tauri::command]
pub(crate) async fn provider_usage(native: tauri::State<'_, crate::native::Native>, provider: String) -> Result<Option<ProviderUsage>, String> {
    let Some(source) = source(&provider) else { return Ok(None) };
    let engine = native.engine().clone();
    if engine.credentials.get(&provider).is_none() {
        return Ok(None);
    }
    let Some(credential) = engine.current_credential(&provider).await.and_then(from_engine) else {
        return Ok(Some(signed_out(UsageStatus::Expired)));
    };
    let Some(request) = request(source, &credential) else { return Ok(None) };
    let mut builder = client()?.get(request.url).header("User-Agent", concat!("drift/", env!("CARGO_PKG_VERSION")));
    for (name, value) in request.headers {
        builder = builder.header(name, value);
    }
    let response = builder.send().await.map_err(|error| error.to_string())?;
    let code = response.status().as_u16();
    let body = response.bytes().await.map_err(|error| error.to_string())?;
    if !(200..300).contains(&code) {
        let text = String::from_utf8_lossy(&body);
        return failure_status(code, &text).map(|status| Some(signed_out(status))).ok_or(format!("usage request failed ({code})"));
    }
    let parsed: Value = serde_json::from_slice(&body).map_err(|error| error.to_string())?;
    Ok(Some(parse(source, &parsed, now_ms())))
}

#[cfg(test)]
#[path = "usage_limits_tests.rs"]
mod tests;
