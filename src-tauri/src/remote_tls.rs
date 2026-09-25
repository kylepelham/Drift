use rcgen::{
    BasicConstraints, CertificateParams, CidrSubnet, DnType, ExtendedKeyUsagePurpose, GeneralSubtree, IsCa, Issuer,
    KeyPair, KeyUsagePurpose, NameConstraints, SanType,
};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivatePkcs8KeyDer};
use rustls::ServerConfig;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::net::IpAddr;
use std::path::Path;
use std::sync::{Arc, Mutex};
use time::{Duration, OffsetDateTime};

const CA_FILE: &str = "remote-access-ca.pem";
const CA_KEY_FILE: &str = "remote-access-ca-key.pem";
const CA_NAME: &str = "Drift Remote Access";
const CA_YEARS: i64 = 10;
/// Apple rejects TLS leaf certificates valid for more than 825 days, even from user-trusted roots.
const LEAF_DAYS: i64 = 397;
/// The CA may only vouch for private-network addresses, so trusting it cannot expose public sites.
const PERMITTED_V4: [([u8; 4], [u8; 4]); 6] = [
    ([10, 0, 0, 0], [255, 0, 0, 0]),
    ([172, 16, 0, 0], [255, 240, 0, 0]),
    ([192, 168, 0, 0], [255, 255, 0, 0]),
    ([100, 64, 0, 0], [255, 192, 0, 0]),
    ([169, 254, 0, 0], [255, 255, 0, 0]),
    ([127, 0, 0, 0], [255, 0, 0, 0]),
];

/// A per-install certificate authority and the leaf configurations it issues per local address.
pub(crate) struct Tls {
    ca: CertificateDer<'static>,
    issuer: Issuer<'static, KeyPair>,
    fingerprint: String,
    configs: Mutex<HashMap<IpAddr, Arc<ServerConfig>>>,
}

impl Tls {
    pub(crate) fn load_or_create(directory: &Path) -> Result<Self, String> {
        std::fs::create_dir_all(directory).map_err(|error| error.to_string())?;
        let (ca_pem, key_pem) = match read_pair(directory) {
            Some(pair) => pair,
            None => create_pair(directory)?,
        };
        let key = KeyPair::from_pem(&key_pem).map_err(|error| error.to_string())?;
        let ca = CertificateDer::from_pem_slice(ca_pem.as_bytes()).map_err(|error| error.to_string())?;
        Ok(Self {
            fingerprint: fingerprint(&ca),
            ca,
            issuer: Issuer::new(ca_params(OffsetDateTime::now_utc()), key),
            configs: Mutex::new(HashMap::new()),
        })
    }

    /// SHA-256 of the CA certificate, as colon-separated hex, for manual verification and pinning.
    pub(crate) fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    pub(crate) fn ca_der(&self) -> &[u8] {
        &self.ca
    }

    /// Server configuration whose leaf names the address the client connected to.
    pub(crate) fn config_for(&self, address: IpAddr) -> Result<Arc<ServerConfig>, String> {
        if let Some(config) = self.configs.lock().unwrap().get(&address) {
            return Ok(config.clone());
        }
        let config = Arc::new(self.server_config(address)?);
        self.configs.lock().unwrap().insert(address, config.clone());
        Ok(config)
    }

    fn server_config(&self, address: IpAddr) -> Result<ServerConfig, String> {
        let (chain, private) = self.leaf(address)?;
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let mut config = ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .map_err(|error| error.to_string())?
            .with_no_client_auth()
            .with_single_cert(chain, private.into())
            .map_err(|error| error.to_string())?;
        config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
        Ok(config)
    }

    fn leaf(&self, address: IpAddr) -> Result<(Vec<CertificateDer<'static>>, PrivatePkcs8KeyDer<'static>), String> {
        let now = OffsetDateTime::now_utc();
        let mut params = CertificateParams::default();
        params.distinguished_name.push(DnType::CommonName, address.to_string());
        params.subject_alt_names = vec![SanType::IpAddress(address), SanType::DnsName("localhost".try_into().unwrap())];
        params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        params.use_authority_key_identifier_extension = true;
        params.not_before = now - Duration::days(1);
        params.not_after = now + Duration::days(LEAF_DAYS);
        let key = KeyPair::generate().map_err(|error| error.to_string())?;
        let leaf = params.signed_by(&key, &self.issuer).map_err(|error| error.to_string())?;
        let chain = vec![leaf.der().clone(), self.ca.clone()];
        Ok((chain, PrivatePkcs8KeyDer::from(key.serialize_der())))
    }
}

/// Issuer parameters must be reproducible: leaves are signed long after the CA file was written.
fn ca_params(now: OffsetDateTime) -> CertificateParams {
    let mut params = CertificateParams::default();
    params.distinguished_name.push(DnType::CommonName, CA_NAME);
    params.distinguished_name.push(DnType::OrganizationName, "Drift");
    params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
    params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    params.name_constraints = Some(NameConstraints {
        permitted_subtrees: PERMITTED_V4
            .iter()
            .map(|(address, mask)| GeneralSubtree::IpAddress(CidrSubnet::V4(*address, *mask)))
            .chain([GeneralSubtree::DnsName("localhost".into()), GeneralSubtree::DnsName("local".into())])
            .collect(),
        excluded_subtrees: Vec::new(),
    });
    params.not_before = now - Duration::days(1);
    params.not_after = now + Duration::days(365 * CA_YEARS);
    params
}

fn read_pair(directory: &Path) -> Option<(String, String)> {
    let ca = std::fs::read_to_string(directory.join(CA_FILE)).ok()?;
    let key = std::fs::read_to_string(directory.join(CA_KEY_FILE)).ok()?;
    KeyPair::from_pem(&key).ok()?;
    Some((ca, key))
}

fn create_pair(directory: &Path) -> Result<(String, String), String> {
    let key = KeyPair::generate().map_err(|error| error.to_string())?;
    let ca = ca_params(OffsetDateTime::now_utc()).self_signed(&key).map_err(|error| error.to_string())?;
    let (ca_pem, key_pem) = (ca.pem(), key.serialize_pem());
    crate::mcp::write_raw(&directory.join(CA_KEY_FILE), key_pem.as_bytes())?;
    crate::mcp::write_raw(&directory.join(CA_FILE), ca_pem.as_bytes())?;
    Ok((ca_pem, key_pem))
}

fn fingerprint(certificate: &[u8]) -> String {
    Sha256::digest(certificate).iter().map(|byte| format!("{byte:02X}")).collect::<Vec<_>>().join(":")
}

#[cfg(test)]
#[path = "remote_tls_tests.rs"]
mod tests;
