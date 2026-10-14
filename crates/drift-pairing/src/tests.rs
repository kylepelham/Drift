use super::*;

fn sample() -> Pairing {
    Pairing {
        name: "Kyle's PC".into(),
        fingerprint: std::array::from_fn(|index| index as u8),
        addresses: vec!["192.168.1.20".parse().unwrap(), "100.101.2.3".parse().unwrap()],
        port: 41718,
        mac: Some([0xaa, 0xbb, 0xcc, 0x01, 0x02, 0x03]),
        ticket: "ab".repeat(32),
    }
}

#[test]
fn a_pairing_survives_the_round_trip_through_its_uri() {
    let pairing = sample();
    let uri = pairing.to_uri();

    assert!(uri.starts_with("drift://pair?v=1&"), "{uri}");
    assert!(uri.len() < 300, "small enough for a quick scan: {}", uri.len());
    assert_eq!(Pairing::parse(&uri), Ok(pairing.clone()));

    let without_mac = Pairing { mac: None, ..pairing };
    assert_eq!(Pairing::parse(&without_mac.to_uri()), Ok(without_mac));
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
}

#[test]
fn words_are_six_from_the_list_and_follow_every_bit_they_cover() {
    let fingerprint = sample().fingerprint;
    let spoken = words(&fingerprint);

    assert_eq!(spoken.len(), 6);
    assert_eq!(WORDS.lines().count(), 2048);
    assert_eq!(words(&[0; 32]), ["abandon"; 6]);
    assert_eq!(words(&[0xff; 32]), ["zoo"; 6]);

    // The 66th bit is the last one the words cover; flipping it changes the sixth word.
    let mut changed = fingerprint;
    changed[8] ^= 0b0100_0000;
    assert_ne!(words(&changed)[5], spoken[5]);
    assert_eq!(words(&changed)[..5], spoken[..5]);
}

#[test]
fn the_desktops_fingerprint_text_parses_with_or_without_colons() {
    let fingerprint = sample().fingerprint;
    let colons: Vec<String> = fingerprint.iter().map(|byte| format!("{byte:02X}")).collect();

    assert_eq!(parse_fingerprint(&colons.join(":")), Some(fingerprint));
    assert_eq!(parse_fingerprint(&hex(&fingerprint)), Some(fingerprint));
    assert_eq!(parse_fingerprint("00:11"), None);
}
