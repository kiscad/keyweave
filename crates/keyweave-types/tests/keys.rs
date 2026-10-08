#![allow(clippy::unwrap_used)]
use keyweave_types::*;

#[test]
fn aliases_are_physical_and_special_keys_stay_distinct() {
    assert_eq!(
        "left_command".parse::<KeyId>().unwrap(),
        KeyId::new(7, 0xe3)
    );
    assert_eq!(
        "japanese_kana".parse::<KeyId>().unwrap(),
        KeyId::new(7, 0x90)
    );
    assert_ne!(
        "return_or_enter".parse::<KeyId>().unwrap(),
        "keypad_enter".parse::<KeyId>().unwrap()
    );
    assert_ne!(
        "delete_forward".parse::<KeyId>().unwrap(),
        "delete_or_backspace".parse::<KeyId>().unwrap()
    );
    assert!("A".parse::<KeyId>().is_err());
}

#[test]
fn all_generated_names_round_trip_to_the_same_usage() {
    for &(page, name, usage) in KeyId::standard_names() {
        let key = KeyId::named(page, name).unwrap();
        assert_eq!(key, KeyId::new(page, usage));
        assert_eq!(key.name().parse::<KeyId>().unwrap(), key);
    }
    assert!(KeyId::named(KEYBOARD_PAGE, "al_calculator").is_err());
    assert_ne!(
        KeyId::named(KEYBOARD_PAGE, "mute").unwrap(),
        "consumer:mute".parse::<KeyId>().unwrap()
    );
    assert_eq!(
        "consumer:mute".parse::<KeyId>().unwrap(),
        KeyId::new(12, 0xe2)
    );
}
