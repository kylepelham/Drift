use rmcp::RoleClient;
use rmcp::model::{ClientCapabilities, ClientConfig, Implementation, ProtocolVersion};
use rmcp::service::ClientLifecycleMode;
use std::path::Path;

use super::{DriftClient, Era};

/// Probes an unknown era, otherwise starts directly in the server's remembered protocol.
pub(super) fn lifecycle(known: Option<Era>) -> ClientLifecycleMode {
    match known {
        None => ClientLifecycleMode::Auto {
            preferred_versions: vec![ProtocolVersion::V_2026_07_28],
            legacy_version: Some(ProtocolVersion::V_2025_11_25),
        },
        Some(Era::Stateless) => ClientLifecycleMode::Discover {
            preferred_versions: vec![ProtocolVersion::V_2026_07_28],
        },
        Some(Era::Legacy) => ClientLifecycleMode::Initialize,
    }
}

/// Drift offers no sampling or elicitation, so servers requesting them are declined.
/// A workspace connection offers that directory as its root.
/// The legacy handshake offers protocol version 2025-11-25.
fn client_info(roots: bool) -> ClientConfig {
    let mut capabilities = ClientCapabilities::default();
    if roots {
        capabilities.roots = Some(rmcp::model::RootsCapabilities::default());
    }

    ClientConfig::new(capabilities, Implementation::new("Drift", env!("CARGO_PKG_VERSION")))
        .with_protocol_version(ProtocolVersion::V_2025_11_25)
}

impl DriftClient {
    pub(super) fn rooted(workspace: Option<&Path>) -> Self {
        let root = workspace.map(|path| {
            reqwest::Url::from_directory_path(path).map_or_else(|()| path.to_string_lossy().into_owned(), String::from)
        });

        Self { root }
    }
}

#[expect(deprecated, reason = "legacy servers still request workspace roots")]
impl rmcp::ClientHandler for DriftClient {
    fn get_info(&self) -> ClientConfig {
        client_info(self.root.is_some())
    }

    async fn list_roots(
        &self,
        _context: rmcp::service::RequestContext<RoleClient>,
    ) -> Result<rmcp::model::ListRootsResult, rmcp::ErrorData> {
        let roots = self.root.iter().map(rmcp::model::Root::new).collect();

        Ok(rmcp::model::ListRootsResult::new(roots))
    }
}
