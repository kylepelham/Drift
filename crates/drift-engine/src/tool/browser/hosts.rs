//! Which sites a call acts on, for its permission asks, and which of them are the user's own machines and network.

use std::net::IpAddr;

use serde_json::Value;

use super::super::Context;
use crate::browser::instance_url;

/// The hosts a call would act on: the addresses it loads and the page it acts on now.
pub(super) fn of_call(context: &Context, input: &Value) -> Vec<String> {
    let current = context
        .engine
        .browsers
        .get(&context.session_id)
        .and_then(|instance| instance_url(&instance, input["page"].as_str()));
    let mut hosts = Vec::new();

    let steps: Vec<&Value> = match input["action"].as_str() {
        Some("batch") => input["actions"].as_array().into_iter().flatten().collect(),
        _ => vec![input],
    };
    for step in steps {
        for host in of_step(step, current.as_deref()) {
            if !hosts.contains(&host) {
                hosts.push(host);
            }
        }
    }

    hosts
}

fn of_step(step: &Value, current: Option<&str>) -> Vec<String> {
    let action = step["action"].as_str().unwrap_or_default();
    let url = step["url"].as_str();

    match (action, url) {
        ("pages" | "stop" | "select" | "close", _) => Vec::new(),
        ("open" | "navigate", Some(url)) if !matches!(url, "back" | "forward" | "reload") => {
            host(&crate::browser::with_scheme(url)).into_iter().collect()
        }
        ("open", None) => Vec::new(),
        _ => current.and_then(host).into_iter().collect(),
    }
}

/// A URL's host, `file` for local files, nothing for blank and internal pages.
pub(super) fn host(url: &str) -> Option<String> {
    let parsed = reqwest::Url::parse(url).ok()?;
    match parsed.scheme() {
        "http" | "https" => parsed
            .host_str()
            .map(|host| host.trim_matches(['[', ']']).to_lowercase()),
        "file" => Some("file".into()),
        _ => None,
    }
}

/// The user's own machine or network: loopback, private addresses, and intranet names (one label, `.local` and
/// the like), where a dev server or an internal app lives.
pub(super) fn is_private(host: &str) -> bool {
    if let Ok(address) = host.parse::<IpAddr>() {
        return match address {
            IpAddr::V4(v4) => v4.is_loopback() || v4.is_private() || v4.is_link_local(),
            IpAddr::V6(v6) => v6.is_loopback() || v6.is_unique_local() || v6.is_unicast_link_local(),
        };
    }

    let suffixes = [".localhost", ".local", ".internal", ".lan", ".home.arpa", ".test"];
    host == "localhost" || !host.contains('.') || suffixes.iter().any(|suffix| host.ends_with(suffix))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dev_servers_and_intranet_hosts_are_private() {
        for host in [
            "localhost",
            "127.0.0.1",
            "192.168.1.20",
            "10.0.0.4",
            "::1",
            "devbox",
            "app.local",
            "web.test",
        ] {
            assert!(is_private(host), "{host}");
        }
        for host in ["example.com", "8.8.8.8", "intranet.company.com"] {
            assert!(!is_private(host), "{host}");
        }
    }

    #[test]
    fn hosts_come_from_addresses() {
        assert_eq!(host("http://localhost:5173/a"), Some("localhost".into()));
        assert_eq!(host("https://[::1]:8080/"), Some("::1".into()));
        assert_eq!(host("file:///C:/site/index.html"), Some("file".into()));
        assert_eq!(host("about:blank"), None);
    }

    #[test]
    fn a_batch_asks_for_every_site_it_loads_and_the_page_it_acts_on() {
        let batch = serde_json::json!({ "action": "batch", "actions": [
            { "action": "navigate", "url": "example.com" },
            { "action": "click", "ref": 3 },
            { "action": "navigate", "url": "back" }
        ]});
        let steps: Vec<Vec<String>> = batch["actions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|step| of_step(step, Some("http://localhost:3000/")))
            .collect();

        assert_eq!(steps, [vec!["example.com"], vec!["localhost"], vec!["localhost"]]);
    }
}
