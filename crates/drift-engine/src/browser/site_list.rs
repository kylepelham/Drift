//! Edge's Enterprise Mode site list, which opens the sites it names in IE mode and sends paths of them back to
//! Edge. Edge keeps the list it downloaded as JSON in each profile: hosts, then paths, then the engine for each.

use std::path::{Path, PathBuf};

use serde_json::Value;

/// Where an address opens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Route {
    Ie,
    Edge,
}

pub(crate) struct SiteList {
    /// `{ scheme or "*": { host[:port]: { path: { "enterprise": { "engine": 1 | 2 } } } } }`.
    sites: Value,
}

/// Edge's engine numbers in the cached list.
const ENGINE_EDGE: u64 = 1;
const ENGINE_IE: u64 = 2;

impl SiteList {
    /// The list a profile keeps once Edge has downloaded it.
    pub(crate) fn file(profile: &Path) -> PathBuf {
        profile
            .join("Default")
            .join("DualEngine")
            .join("SiteList-Enterprise.json")
    }

    pub(crate) fn read(file: &Path) -> Option<Self> {
        let text = std::fs::read_to_string(file).ok()?;
        let sites = serde_json::from_str(&text).ok()?;

        Some(Self { sites })
    }

    /// The copy the user's own Edge keeps, the one Drift can read before it starts a browser.
    pub(crate) fn users() -> Option<Self> {
        let local = std::env::var_os("LOCALAPPDATA")?;
        let profile = Path::new(&local).join("Microsoft").join("Edge").join("User Data");

        Self::read(&Self::file(&profile))
    }

    /// Where `url` opens, or none when the list does not name its site.
    pub(crate) fn route(&self, url: &str) -> Option<Route> {
        let (scheme, rest) = url.split_once("://")?;
        let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
        let authority = rest[..end].to_ascii_lowercase();
        let path = rest[end..]
            .split(['?', '#'])
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        let path = if path.is_empty() { "/".to_string() } else { path };
        let host = authority.split(':').next().unwrap_or_default();

        let paths = [scheme.to_ascii_lowercase().as_str(), "*"]
            .into_iter()
            .filter_map(|scheme| self.sites.get(scheme))
            .find_map(|hosts| site(hosts, &authority, host))?;
        let engine = paths
            .as_object()?
            .iter()
            .filter(|(prefix, _)| path.starts_with(prefix.as_str()))
            .max_by_key(|(prefix, _)| prefix.len())?
            .1["enterprise"]["engine"]
            .as_u64();

        match engine {
            Some(ENGINE_IE) => Some(Route::Ie),
            Some(ENGINE_EDGE) => Some(Route::Edge),
            _ => None,
        }
    }
}

/// The entry for an address's host: with its port first, then the host, then the domains it belongs to.
fn site<'a>(hosts: &'a Value, authority: &str, host: &str) -> Option<&'a Value> {
    if let Some(paths) = hosts.get(authority) {
        return Some(paths);
    }
    let mut domain = host;
    loop {
        if let Some(paths) = hosts.get(domain) {
            return Some(paths);
        }
        domain = domain.split_once('.')?.1;
    }
}

/// Whether Edge here runs IE mode from a site list, as a group policy sets it up.
#[cfg(windows)]
pub(crate) fn in_force() -> bool {
    use crate::platform::process::{registry_dword, registry_string};
    use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};

    const POLICIES: &str = r"SOFTWARE\Policies\Microsoft\Edge";
    /// `InternetExplorerIntegrationLevel` 1 is IE mode.
    const IE_MODE: u32 = 1;
    let roots = [HKEY_LOCAL_MACHINE, HKEY_CURRENT_USER];

    let ie_mode = roots
        .iter()
        .any(|root| registry_dword(*root, POLICIES, "InternetExplorerIntegrationLevel") == Some(IE_MODE));
    let listed = roots.iter().any(|root| {
        [
            "InternetExplorerIntegrationSiteList",
            "InternetExplorerIntegrationCloudSiteList",
        ]
        .iter()
        .any(|value| registry_string(*root, POLICIES, value).is_some_and(|list| !list.is_empty()))
    });

    ie_mode && listed
}

#[cfg(not(windows))]
pub(crate) fn in_force() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list() -> SiteList {
        let sites = serde_json::json!({ "*": {
            "app.example.local": {
                "/": { "enterprise": { "engine": 2, "compat_mode": "IE8Enterprise" } },
                "/edge": { "enterprise": { "engine": 1 } },
                "/trav/pcipal.asp": { "enterprise": { "engine": 1 } }
            },
            "app.example.local:8080": { "/": { "enterprise": { "engine": 1 } } },
            "example.org": { "/": { "enterprise": { "engine": 2 } } }
        }});

        SiteList { sites }
    }

    #[test]
    fn the_most_specific_path_of_a_listed_site_decides() {
        let list = list();

        assert_eq!(list.route("https://app.example.local/"), Some(Route::Ie));
        assert_eq!(
            list.route("https://App.Example.Local/Main.asp?Site=go"),
            Some(Route::Ie)
        );
        assert_eq!(
            list.route("https://app.example.local/Edge/abc/CMS?Cat=2"),
            Some(Route::Edge)
        );
        assert_eq!(
            list.route("https://app.example.local/trav/PciPal.asp"),
            Some(Route::Edge)
        );
        assert_eq!(list.route("https://app.example.local"), Some(Route::Ie));
    }

    #[test]
    fn a_port_or_a_parent_domain_finds_its_own_entry_and_unlisted_sites_find_none() {
        let list = list();

        assert_eq!(list.route("https://app.example.local:8080/escrow"), Some(Route::Edge));
        assert_eq!(list.route("https://app.example.local:9090/"), Some(Route::Ie));
        assert_eq!(list.route("http://intranet.example.org/home"), Some(Route::Ie));
        assert_eq!(list.route("http://127.0.0.1:5173/"), None);
        assert_eq!(list.route("about:blank"), None);
    }
}
