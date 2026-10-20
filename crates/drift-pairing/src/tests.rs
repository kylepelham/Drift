use super::*;

fn sample() -> Pairing {
    Pairing {
        name: "Kyle's PC".into(),
        fingerprint: std::array::from_fn(|index| index as u8),
        addresses: vec!["192.168.1.20".parse().unwrap(), "100.101.2.3".parse().unwrap()],
        port: 41718,
        mac: Some([0xaa, 0xbb, 0xcc, 0x01, 0x02, 0x03]),
        tunnel: Some(Tunnel {
            host: "k2v7qxmd4hzt6wnc.x3fp8rk2mq7a.opentunnel.xyz".into(),
            key: [0x5a; 32],
        }),
        ticket: "ab".repeat(32),
    }
}

#[test]
fn a_pairing_survives_the_round_trip_through_its_uri() {
    let pairing = sample();
    let uri = pairing.to_uri();

    assert!(uri.starts_with("drift://pair?v=1&"), "{uri}");
    assert!(uri.len() < 420, "small enough for a quick scan: {}", uri.len());
    assert_eq!(Pairing::parse(&uri), Ok(pairing.clone()));

    let lan_only = Pairing {
        mac: None,
        tunnel: None,
        ..pairing.clone()
    };
    assert_eq!(Pairing::parse(&lan_only.to_uri()), Ok(lan_only));

    let away_from_home = Pairing {
        addresses: Vec::new(),
        ..pairing
    };
    assert_eq!(Pairing::parse(&away_from_home.to_uri()), Ok(away_from_home));
}

#[test]
fn anything_else_is_refused_with_a_reason() {
    assert_eq!(Pairing::parse("https://example.com"), Err(PairingError::NotPairing));
    assert_eq!(Pairing::parse("drift://pair?v=9&t=x"), Err(PairingError::Version));
    assert_eq!(Pairing::parse("not a uri"), Err(PairingError::NotPairing));

    let uri = sample().to_uri();
    let short_fingerprint = uri.replace(&hex(&sample().fingerprint), "abcd");
    assert_eq!(
        Pairing::parse(&short_fingerprint),
        Err(PairingError::Invalid("fingerprint"))
    );
    let bad_address = uri.replace("192.168.1.20", "nowhere");
    assert_eq!(Pairing::parse(&bad_address), Err(PairingError::Invalid("address")));

    let nowhere = Pairing {
        addresses: Vec::new(),
        tunnel: None,
        ..sample()
    };
    assert_eq!(
        Pairing::parse(&nowhere.to_uri()),
        Err(PairingError::Missing("address or ticket"))
    );
}

#[test]
fn a_tunnel_needs_a_plain_host_and_its_key() {
    let uri = sample().to_uri();
    let without_key = uri.replace(&format!("&k={}", hex(&[0x5a; 32])), "");
    assert_eq!(Pairing::parse(&without_key), Err(PairingError::Invalid("tunnel")));

    let short_key = uri.replace(&hex(&[0x5a; 32]), "5a5a");
    assert_eq!(Pairing::parse(&short_key), Err(PairingError::Invalid("tunnel key")));

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
    let fingerprint = sample().fingerprint;
    let colons: Vec<String> = fingerprint.iter().map(|byte| format!("{byte:02X}")).collect();

    assert_eq!(parse_fingerprint(&colons.join(":")), Some(fingerprint));
    assert_eq!(parse_fingerprint(&hex(&fingerprint)), Some(fingerprint));
    assert_eq!(parse_fingerprint("00:11"), None);
}
