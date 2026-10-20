//! The user agent a page reports. Headless Chrome names itself `HeadlessChrome` in its user agent and client
//! hints; this replaces both with what the same browser reports with a window, and makes the mobile one.

use serde_json::{Value, json};

use super::cdp::Connection;
use crate::browser::Kind;

/// The desktop override (none with a real window, which reports itself truthfully) and the mobile one.
pub(crate) struct Agents {
    pub(crate) desktop: Option<Value>,
    pub(crate) mobile: Value,
}

/// What the browser and its first page say about themselves.
pub(crate) async fn agents(cdp: &Connection, kind: Kind, headless: bool) -> Agents {
    let version = cdp
        .call("Browser.getVersion", json!({}), None)
        .await
        .unwrap_or_default();
    let user_agent = version["userAgent"]
        .as_str()
        .unwrap_or_default()
        .replace("HeadlessChrome", "Chrome");
    let product = version["product"].as_str().unwrap_or_default();

    let chromium_full = after(&user_agent, "Chrome/").unwrap_or("0.0.0.0").to_string();
    let chromium_full = after(product, "Chrome/").map_or(chromium_full, str::to_string);
    let brand_full = match kind {
        Kind::Edge => after(&user_agent, "Edg/").unwrap_or(&chromium_full).to_string(),
        _ => chromium_full.clone(),
    };
    let versions = Versions {
        kind,
        chromium_full,
        brand_full,
    };

    let desktop = if headless && !user_agent.is_empty() {
        let hints = page_hints(cdp).await.unwrap_or_else(default_hints);
        Some(json!({ "userAgent": user_agent, "userAgentMetadata": metadata(&versions, &hints, false) }))
    } else {
        None
    };
    let mobile_hints = json!({
        "platform": "Android",
        "platformVersion": "10.0.0",
        "architecture": "",
        "bitness": "",
        "model": "K",
    });
    let mobile_agent = format!(
        "Mozilla/5.0 (Linux; Android 10; K) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/{}.0.0.0 Mobile \
         Safari/537.36",
        major(&versions.chromium_full)
    );
    let mobile = json!({ "userAgent": mobile_agent, "userAgentMetadata": metadata(&versions, &mobile_hints, true) });

    Agents { desktop, mobile }
}

struct Versions {
    kind: Kind,
    chromium_full: String,
    brand_full: String,
}

/// The version after `token` in a user agent, up to the next space.
fn after<'a>(text: &'a str, token: &str) -> Option<&'a str> {
    let start = text.find(token)? + token.len();
    text[start..].split_whitespace().next()
}

fn major(full: &str) -> u32 {
    full.split('.').next().and_then(|major| major.parse().ok()).unwrap_or(0)
}

/// What a real page would get from `getHighEntropyValues`, read from the browser's first tab before any site loads.
async fn page_hints(cdp: &Connection) -> Option<Value> {
    let targets = cdp.call("Target.getTargets", json!({}), None).await.ok()?;
    let target = targets["targetInfos"]
        .as_array()?
        .iter()
        .find(|target| target["type"] == "page")?["targetId"]
        .clone();
    let attached = cdp
        .call(
            "Target.attachToTarget",
            json!({ "targetId": target, "flatten": true }),
            None,
        )
        .await
        .ok()?;
    let session = attached["sessionId"].as_str()?.to_string();

    let expression = "navigator.userAgentData.getHighEntropyValues(\
        ['architecture','bitness','model','platformVersion','wow64'])\
        .then(values => ({ ...values, platform: navigator.userAgentData.platform }))";
    let result = cdp
        .call(
            "Runtime.evaluate",
            json!({ "expression": expression, "awaitPromise": true, "returnByValue": true }),
            Some(&session),
        )
        .await;
    let _ = cdp
        .call("Target.detachFromTarget", json!({ "sessionId": session }), None)
        .await;

    let value = result.ok()?["result"]["value"].clone();
    value.is_object().then_some(value)
}

fn default_hints() -> Value {
    let platform = if cfg!(windows) {
        "Windows"
    } else if cfg!(target_os = "macos") {
        "macOS"
    } else {
        "Linux"
    };

    json!({ "platform": platform, "platformVersion": "", "architecture": "x86", "bitness": "64", "model": "" })
}

fn metadata(versions: &Versions, hints: &Value, mobile: bool) -> Value {
    let seed = major(&versions.chromium_full);
    let brand = if versions.kind == Kind::Edge {
        "Microsoft Edge"
    } else {
        "Google Chrome"
    };

    json!({
        "brands": brands(seed, brand, &major(&versions.brand_full).to_string(), &seed.to_string(), false),
        "fullVersionList": brands(seed, brand, &versions.brand_full, &versions.chromium_full, true),
        "fullVersion": versions.brand_full,
        "platform": hints["platform"],
        "platformVersion": hints["platformVersion"],
        "architecture": hints["architecture"],
        "bitness": hints["bitness"],
        "model": hints["model"],
        "wow64": hints["wow64"].as_bool().unwrap_or(false),
        "mobile": mobile,
    })
}

/// Chromium's brand list for a major version: a "Not A Brand" entry and its order both follow from the major
/// version (`GenerateBrandVersionList` in Chromium's `user_agent_utils.cc`), so a made-up order would stand out.
fn brands(seed: u32, brand: &str, brand_version: &str, chromium_version: &str, full: bool) -> Value {
    const GREASE: [&str; 11] = [" ", "(", ":", "-", ".", "/", ")", ";", "=", "?", "_"];
    const GREASE_VERSIONS: [&str; 3] = ["8", "99", "24"];
    const ORDERS: [[usize; 3]; 6] = [[0, 1, 2], [0, 2, 1], [1, 0, 2], [1, 2, 0], [2, 0, 1], [2, 1, 0]];
    let seed = seed as usize;

    let grease_name = format!("Not{}A{}Brand", GREASE[seed % 11], GREASE[(seed + 1) % 11]);
    let grease_version = GREASE_VERSIONS[seed % 3];
    let grease_version = if full {
        format!("{grease_version}.0.0.0")
    } else {
        grease_version.to_string()
    };

    let entries = [
        json!({ "brand": grease_name, "version": grease_version }),
        json!({ "brand": "Chromium", "version": chromium_version }),
        json!({ "brand": brand, "version": brand_version }),
    ];
    let mut ordered = vec![Value::Null; 3];
    for (entry, slot) in entries.into_iter().zip(ORDERS[seed % 6]) {
        ordered[slot] = entry;
    }

    Value::Array(ordered)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(list: &Value) -> Vec<String> {
        list.as_array()
            .unwrap()
            .iter()
            .map(|entry| {
                format!(
                    "{} {}",
                    entry["brand"].as_str().unwrap(),
                    entry["version"].as_str().unwrap()
                )
            })
            .collect()
    }

    /// Lists real Chrome builds sent in their `Sec-CH-UA` header.
    #[test]
    fn brands_match_what_chrome_itself_reports() {
        assert_eq!(
            names(&brands(120, "Google Chrome", "120", "120", false)),
            ["Not_A Brand 8", "Chromium 120", "Google Chrome 120"]
        );
        assert_eq!(
            names(&brands(124, "Google Chrome", "124", "124", false)),
            ["Chromium 124", "Google Chrome 124", "Not-A.Brand 99"]
        );
        assert_eq!(
            names(&brands(131, "Google Chrome", "131", "131", false)),
            ["Google Chrome 131", "Chromium 131", "Not_A Brand 24"]
        );
    }

    #[test]
    fn versions_are_read_from_the_user_agent() {
        let agent = "Mozilla/5.0 (Windows NT 10.0) AppleWebKit/537.36 Chrome/131.0.0.0 Safari/537.36 Edg/131.0.2903.86";

        assert_eq!(after(agent, "Edg/"), Some("131.0.2903.86"));
        assert_eq!(major("131.0.6778.86"), 131);
    }
}
