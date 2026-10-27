use super::*;

/// Cheap stretching so tests run fast; the default's cost is checked on its own.
const QUICK: Stretch = Stretch {
    memory: 64,
    passes: 1,
    lanes: 1,
};

fn at(sequence: u64) -> Context<'static> {
    Context {
        group: "g1",
        device: "work",
        sequence,
    }
}

#[test]
fn the_password_opens_the_group_key_and_nothing_else_does() {
    let salt = new_salt().unwrap();
    let key = GroupKey::generate().unwrap();
    let unlock = Unlock::from_password("correct horse", &salt, QUICK).unwrap();
    let wrapped = unlock.wrap(&key, "g1").unwrap();

    let again = Unlock::from_password("correct horse", &salt, QUICK).unwrap();
    assert_eq!(again.unwrap(&wrapped, "g1").unwrap().bytes(), key.bytes());
    assert_eq!(again.login, unlock.login, "the same password logs in the same way");

    let wrong = Unlock::from_password("correct horsf", &salt, QUICK).unwrap();
    assert_eq!(wrong.unwrap(&wrapped, "g1").err(), Some(CryptoError::Tampered));
    assert_ne!(wrong.login, unlock.login);
    assert_eq!(
        again.unwrap(&wrapped, "g2").err(),
        Some(CryptoError::Tampered),
        "bound to its group"
    );

    let other_salt = Unlock::from_password("correct horse", &new_salt().unwrap(), QUICK).unwrap();
    assert_ne!(other_salt.login, unlock.login);
    assert_ne!(
        unlock.login, unlock.wrap,
        "what the server sees is not what unwraps the key"
    );
}

#[test]
fn an_object_opens_only_unchanged_from_its_own_computer_and_place() {
    let key = GroupKey::generate().unwrap();
    let work = DeviceKey::generate().unwrap();
    let home = DeviceKey::generate().unwrap();
    let plain = br#"{"parts":["the agent kept reversing the protocol"]}"#.repeat(50);

    let object = key.seal(&work, &at(7), &plain).unwrap();
    assert_eq!(key.open(work.public(), &at(7), &object).unwrap(), plain);
    assert_eq!(
        object.len() % PAD,
        1 + ID_LEN + 16 + SIGNATURE_LEN,
        "padded to hide its size"
    );
    assert!(
        !object.windows(9).any(|window| window == b"reversing"),
        "nothing readable is sent"
    );

    assert_eq!(
        key.open(home.public(), &at(7), &object).err(),
        Some(CryptoError::Signature)
    );
    assert_eq!(
        key.open(work.public(), &at(8), &object).err(),
        Some(CryptoError::Tampered),
        "replayed elsewhere"
    );
    let other = GroupKey::generate().unwrap();
    assert_eq!(
        other.open(work.public(), &at(7), &object).err(),
        Some(CryptoError::Tampered)
    );

    let mut flipped = object.clone();
    flipped[40] ^= 1;
    assert_eq!(
        key.open(work.public(), &at(7), &flipped).err(),
        Some(CryptoError::Signature)
    );
    assert_ne!(key.seal(&work, &at(7), &plain).unwrap(), object, "each seal is fresh");
}

#[test]
fn a_separator_in_a_name_cannot_move_an_object_to_another_place() {
    let key = GroupKey::generate().unwrap();
    let work = DeviceKey::generate().unwrap();
    let sealed_at = Context {
        group: "a|b",
        device: "c",
        sequence: 1,
    };
    let moved = Context {
        group: "a",
        device: "b|c",
        sequence: 1,
    };
    let object = key.seal(&work, &sealed_at, b"x").unwrap();

    assert_eq!(
        key.open(work.public(), &moved, &object).err(),
        Some(CryptoError::Tampered)
    );
}

#[test]
fn a_device_list_entry_is_vouched_for_only_by_the_group_key() {
    let key = GroupKey::generate().unwrap();
    let device = DeviceKey::generate().unwrap();
    let tag = key.vouch("work", device.public());

    assert!(key.vouched_for("work", device.public(), &tag));
    assert!(
        !key.vouched_for("home", device.public(), &tag),
        "a key moved to another name"
    );
    let planted = DeviceKey::generate().unwrap();
    assert!(!key.vouched_for("work", planted.public(), &tag), "a server's own key");
    assert!(!GroupKey::generate().unwrap().vouched_for("work", device.public(), &tag));
}

#[test]
fn keys_survive_the_keychain_and_blinded_names_agree_across_computers() {
    let device = DeviceKey::generate().unwrap();
    let restored = DeviceKey::from_pkcs8(device.pkcs8()).unwrap();
    assert_eq!(restored.public(), device.public());

    let key = GroupKey::generate().unwrap();
    let copy = GroupKey::from_bytes(*key.bytes());
    assert_eq!(copy.blind("sha256:abc"), key.blind("sha256:abc"));
    assert_ne!(key.blind("sha256:abc"), key.blind("sha256:abe"));
    assert_ne!(
        GroupKey::generate().unwrap().blind("sha256:abc"),
        key.blind("sha256:abc")
    );
}

#[test]
fn a_forged_object_cannot_claim_more_than_it_holds() {
    let key = GroupKey::generate().unwrap();
    let work = DeviceKey::generate().unwrap();
    let object = key.seal(&work, &at(1), b"").unwrap();
    assert_eq!(key.open(work.public(), &at(1), &object).unwrap(), b"");

    assert!(unpadded(&[0, 0, 1, 0, 1, 2]).is_err(), "a length past the padding");
    assert_eq!(
        key.open(work.public(), &at(1), &[1; 10]).err(),
        Some(CryptoError::Tampered)
    );
}

#[test]
fn the_default_stretch_is_accepted() {
    let unlock = Unlock::from_password("pw", &new_salt().unwrap(), Stretch::DEFAULT).unwrap();
    assert_ne!(unlock.login, [0; KEY_LEN]);
}
