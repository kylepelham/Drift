//! AWS credentials as the AWS CLI finds them (environment, then the shared profile files) and SigV4 request signing.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::PathBuf;

use ring::{digest, hmac};

#[derive(Clone, Debug, PartialEq)]
pub struct Keys {
    pub access_key: String,
    pub secret_key: String,
    pub session_token: Option<String>,
}

/// How a Bedrock request authenticates: an API key as a bearer token, or signed with account keys.
#[derive(Clone, Debug, PartialEq)]
pub enum Auth {
    Bearer(String),
    Signed(Keys),
}

/// What the environment offers, described for the provider list; `None` when there is nothing to sign with.
pub fn detect() -> Option<String> {
    if env("AWS_BEARER_TOKEN_BEDROCK").is_some() {
        return Some("AWS_BEARER_TOKEN_BEDROCK".into());
    }
    if env("AWS_ACCESS_KEY_ID").is_some() && env("AWS_SECRET_ACCESS_KEY").is_some() {
        return Some("AWS_ACCESS_KEY_ID".into());
    }

    let profile = profile_name();
    profile_keys(&profile).map(|_| format!("AWS profile {profile}"))
}

/// The bearer token or keys to use now, read afresh so a rotated profile is picked up.
pub fn auth() -> Option<Auth> {
    if let Some(token) = env("AWS_BEARER_TOKEN_BEDROCK") {
        return Some(Auth::Bearer(token));
    }
    if let (Some(access_key), Some(secret_key)) = (env("AWS_ACCESS_KEY_ID"), env("AWS_SECRET_ACCESS_KEY")) {
        return Some(Auth::Signed(Keys {
            access_key,
            secret_key,
            session_token: env("AWS_SESSION_TOKEN"),
        }));
    }

    profile_keys(&profile_name()).map(Auth::Signed)
}

/// `AWS_REGION`, then `AWS_DEFAULT_REGION`, then the profile's `region`, then us-east-1.
pub fn region() -> String {
    env("AWS_REGION")
        .or_else(|| env("AWS_DEFAULT_REGION"))
        .or_else(|| {
            let profile = profile_name();
            let section = if profile == "default" {
                profile
            } else {
                format!("profile {profile}")
            };
            ini(&aws_file("AWS_CONFIG_FILE", "config"))
                .remove(&section)?
                .remove("region")
        })
        .unwrap_or_else(|| "us-east-1".into())
}

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.trim().is_empty())
}

fn profile_name() -> String {
    env("AWS_PROFILE").unwrap_or_else(|| "default".into())
}

fn aws_file(variable: &str, name: &str) -> PathBuf {
    env(variable).map_or_else(|| home().join(".aws").join(name), PathBuf::from)
}

fn home() -> PathBuf {
    env("USERPROFILE")
        .or_else(|| env("HOME"))
        .map(PathBuf::from)
        .unwrap_or_default()
}

fn profile_keys(profile: &str) -> Option<Keys> {
    let mut section = ini(&aws_file("AWS_SHARED_CREDENTIALS_FILE", "credentials")).remove(profile)?;

    Some(Keys {
        access_key: section.remove("aws_access_key_id")?,
        secret_key: section.remove("aws_secret_access_key")?,
        session_token: section.remove("aws_session_token"),
    })
}

/// The shared files' INI: `[section]` headers and `key = value` lines; comments and anything else skipped.
fn ini(path: &std::path::Path) -> HashMap<String, HashMap<String, String>> {
    let mut sections: HashMap<String, HashMap<String, String>> = HashMap::new();
    let mut current = String::new();
    let text = std::fs::read_to_string(path).unwrap_or_default();

    for line in text.lines().map(str::trim) {
        if let Some(name) = line.strip_prefix('[').and_then(|line| line.strip_suffix(']')) {
            current = name.trim().to_string();
        } else if let Some((key, value)) = line.split_once('=').filter(|_| !line.starts_with(['#', ';'])) {
            sections
                .entry(current.clone())
                .or_default()
                .insert(key.trim().to_lowercase(), value.trim().to_string());
        }
    }

    sections
}

/// One request to sign: its pieces as they will go on the wire, the path already percent-encoded once.
pub struct Signing<'a> {
    pub method: &'a str,
    pub host: &'a str,
    pub path: &'a str,
    pub body: &'a [u8],
    pub region: &'a str,
    pub service: &'a str,
    /// `YYYYMMDD'T'HHMMSS'Z'`.
    pub amz_date: &'a str,
}

/// The headers that make a signed request: `x-amz-date`, `x-amz-content-sha256`, the session token if any, and `authorization`.
pub fn sign(request: &Signing, keys: &Keys) -> Vec<(String, String)> {
    let payload = hex(digest::digest(&digest::SHA256, request.body).as_ref());
    let mut headers = vec![
        ("host", request.host.to_string()),
        ("x-amz-content-sha256", payload.clone()),
        ("x-amz-date", request.amz_date.to_string()),
    ];
    if let Some(token) = &keys.session_token {
        headers.push(("x-amz-security-token", token.clone()));
    }

    let signed = headers.iter().map(|(name, _)| *name).collect::<Vec<_>>().join(";");
    let mut canonical_headers = String::new();
    for (name, value) in &headers {
        writeln!(canonical_headers, "{name}:{}", value.trim()).unwrap();
    }

    // Services other than S3 take each path segment encoded twice: once on the wire, once here.
    let canonical_path = request.path.split('/').map(encode).collect::<Vec<_>>().join("/");
    let canonical = format!(
        "{}\n{canonical_path}\n\n{canonical_headers}\n{signed}\n{payload}",
        request.method
    );
    let day = &request.amz_date[..8];
    let scope = format!("{day}/{}/{}/aws4_request", request.region, request.service);
    let canonical_hash = hex(digest::digest(&digest::SHA256, canonical.as_bytes()).as_ref());
    let to_sign = format!("AWS4-HMAC-SHA256\n{}\n{scope}\n{canonical_hash}", request.amz_date);

    let key = [day, request.region, request.service, "aws4_request"]
        .iter()
        .fold(format!("AWS4{}", keys.secret_key).into_bytes(), |key, part| {
            mac(&key, part.as_bytes())
        });
    let signature = hex(&mac(&key, to_sign.as_bytes()));

    let mut out: Vec<(String, String)> = headers
        .into_iter()
        .filter(|(name, _)| *name != "host")
        .map(|(name, value)| (name.to_string(), value))
        .collect();
    out.push((
        "authorization".into(),
        format!(
            "AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={signed}, Signature={signature}",
            keys.access_key
        ),
    ));

    out
}

/// SigV4's encoding: everything but unreserved characters as upper-case `%XX`.
pub fn encode(text: &str) -> String {
    text.bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (byte as char).to_string(),
            other => format!("%{other:02X}"),
        })
        .collect()
}

fn mac(key: &[u8], data: &[u8]) -> Vec<u8> {
    hmac::sign(&hmac::Key::new(hmac::HMAC_SHA256, key), data)
        .as_ref()
        .to_vec()
}

fn hex(bytes: &[u8]) -> String {
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(encoded, "{byte:02x}").unwrap();
    }

    encoded
}

/// Now as SigV4 writes it.
pub fn amz_date() -> String {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs());
    let (days, rest) = (seconds / 86_400, seconds % 86_400);
    let (year, month, day) = civil(days as i64);

    format!(
        "{year:04}{month:02}{day:02}T{:02}{:02}{:02}Z",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

/// Days since 1970-01-01 to a calendar date (Howard Hinnant's algorithm).
fn civil(days: i64) -> (i64, u32, u32) {
    let shifted_days = days + 719_468;
    let era = shifted_days.div_euclid(146_097);
    let day_of_era = shifted_days - era * 146_097;
    let year_of_era = (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);

    // Hinnant's calendar starts in March so leap days fall at the end of each year.
    let march_month = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * march_month + 2) / 5 + 1) as u32;
    let month = if march_month < 10 {
        march_month + 3
    } else {
        march_month - 9
    } as u32;
    let year = year_of_era + era * 400 + i64::from(month <= 2);

    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn example() -> Keys {
        Keys {
            access_key: "AKIDEXAMPLE".into(),
            secret_key: "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY".into(),
            session_token: None,
        }
    }

    /// The signing key and string-to-sign steps from AWS's published SigV4 example.
    #[test]
    fn the_derived_key_matches_the_published_example() {
        let key = ["20150830", "us-east-1", "iam", "aws4_request"]
            .iter()
            .fold(format!("AWS4{}", example().secret_key).into_bytes(), |key, part| {
                mac(&key, part.as_bytes())
            });
        assert_eq!(
            hex(&key),
            "c4afb1cc5771d871763a393e44b703571b55cc28424d1a5e86da6ed3c154a4b9"
        );
        let to_sign = concat!(
            "AWS4-HMAC-SHA256\n20150830T123600Z\n20150830/us-east-1/iam/aws4_request\n",
            "f536975d06c0309214f805bb90ccff089219ecd68b2577efef23edd43b7e1a59",
        );
        assert_eq!(
            hex(&mac(&key, to_sign.as_bytes())),
            "5d672d79c15b13162d9279b0855cfba6789a8edb4c82c400e06b5924a6f2b5d7"
        );
    }

    #[test]
    fn a_request_is_signed_over_its_doubly_encoded_path_and_body() {
        let path = format!(
            "/model/{}/invoke-with-response-stream",
            encode("anthropic.claude-sonnet-4-5-v1:0")
        );
        assert_eq!(
            path,
            "/model/anthropic.claude-sonnet-4-5-v1%3A0/invoke-with-response-stream"
        );
        let request = Signing {
            method: "POST",
            host: "bedrock-runtime.us-east-1.amazonaws.com",
            path: &path,
            body: b"{}",
            region: "us-east-1",
            service: "bedrock",
            amz_date: "20260101T000000Z",
        };
        let headers: HashMap<String, String> = sign(&request, &example()).into_iter().collect();
        assert_eq!(
            headers["x-amz-content-sha256"],
            "44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a"
        );
        let authorization = &headers["authorization"];
        assert!(authorization.starts_with(concat!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-east-1/bedrock/aws4_request, ",
            "SignedHeaders=host;x-amz-content-sha256;x-amz-date, Signature=",
        )));
        let with_token = sign(
            &request,
            &Keys {
                session_token: Some("tok".into()),
                ..example()
            },
        );
        assert!(
            with_token
                .iter()
                .any(|(name, value)| name == "x-amz-security-token" && value == "tok")
        );
        assert!(
            with_token
                .iter()
                .any(|(name, value)| name == "authorization" && value.contains("x-amz-date;x-amz-security-token"))
        );
    }

    #[test]
    fn dates_are_written_as_sigv4_wants() {
        assert_eq!(civil(0), (1970, 1, 1));
        assert_eq!(civil(20_454), (2026, 1, 1));
        assert_eq!(civil(11_016), (2000, 2, 29));
        assert_eq!(amz_date().len(), 16);
    }

    #[test]
    fn profiles_are_read_from_the_shared_files() {
        let dir = std::env::temp_dir().join(format!("drift-aws-{}", crate::random_hex(4)));
        std::fs::create_dir_all(&dir).unwrap();
        let credentials = dir.join("credentials");
        let contents = concat!(
            "# comment\n[default]\naws_access_key_id = AKIA1\naws_secret_access_key = s1\n\n",
            "[work]\naws_access_key_id=AKIA2\naws_secret_access_key=s2\naws_session_token=t2\n",
        );
        std::fs::write(&credentials, contents).unwrap();

        let sections = ini(&credentials);
        assert_eq!(sections["work"]["aws_session_token"], "t2");
        assert_eq!(sections["default"]["aws_access_key_id"], "AKIA1");
        let _ = std::fs::remove_dir_all(dir);
    }
}
