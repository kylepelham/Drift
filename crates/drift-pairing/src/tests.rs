use super::*;

#[test]
fn both_sides_reach_the_same_code_and_any_other_authority_or_secret_changes_it() {
    let authority = [7u8; 32];
    let device = [1u8; 32];
    let computer = [2u8; 32];
    let code = matching_code(&authority, &device, &computer);

    assert_eq!(code, matching_code(&authority, &device, &computer));
    assert_eq!(code.len(), 7, "{code}");
    assert!(code.chars().enumerate().all(|(index, character)| if index == 3 {
        character == ' '
    } else {
        character.is_ascii_digit()
    }));

    assert_ne!(
        code,
        matching_code(&[8u8; 32], &device, &computer),
        "a relay's own authority"
    );
    assert_ne!(code, matching_code(&authority, &[3u8; 32], &computer));
    assert_ne!(code, matching_code(&authority, &device, &[3u8; 32]));
}

#[test]
fn a_commitment_names_one_secret() {
    let secret = [9u8; 32];
    assert_eq!(commitment(&secret), commitment(&secret));
    assert_ne!(commitment(&secret), commitment(&[8u8; 32]));
    assert_ne!(commitment(&secret), secret, "the secret itself is not given away");
}

#[test]
fn a_first_link_comes_from_this_network_only() {
    for address in [
        "192.168.1.20",
        "10.0.0.5",
        "172.20.1.1",
        "100.101.2.3",
        "127.0.0.1",
        "169.254.3.4",
        "::1",
        "fd7a:115c:a1e0::1",
        "fe80::1",
        "::ffff:192.168.1.20",
    ] {
        assert!(on_local_network(address.parse().unwrap()), "{address}");
    }
    for address in ["8.8.8.8", "100.200.1.1", "2001:4860::8888", "::ffff:8.8.8.8"] {
        assert!(!on_local_network(address.parse().unwrap()), "{address}");
    }
}

#[test]
fn an_announcement_survives_its_round_trip() {
    let announcement = Announcement {
        name: "Kyle's PC".into(),
        id: "ab".repeat(32),
        port: PORT,
        api: API_VERSION,
    };
    let bytes = serde_json::to_vec(&announcement).unwrap();

    assert_eq!(serde_json::from_slice::<Announcement>(&bytes).unwrap(), announcement);
}

#[test]
fn a_tunnel_host_is_a_plain_name() {
    for host in [
        "evil.example/steal",
        "evil.example:8443",
        "user@evil.example",
        "localhost",
        ".example.com",
    ] {
        assert!(!valid_host(host), "{host}");
    }
    assert!(valid_host("k2v7qxmd4hzt6wnc.x3fp8rk2mq7a.opentunnel.xyz"));
}

#[test]
fn the_desktops_fingerprint_text_parses_with_or_without_colons() {
    let fingerprint: [u8; 32] = std::array::from_fn(|index| index as u8);
    let colons: Vec<String> = fingerprint.iter().map(|byte| format!("{byte:02X}")).collect();

    assert_eq!(parse_fingerprint(&colons.join(":")), Some(fingerprint));
    assert_eq!(parse_fingerprint(&hex(&fingerprint)), Some(fingerprint));
    assert_eq!(parse_fingerprint("00:11"), None);
}
