//! Bytes as lowercase hex, the way keys and hashes travel in JSON and on
//! screen.

pub fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub fn hex_decode(value: &str) -> Result<Vec<u8>, String> {
    if !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("non-hex digit".into());
    }
    if !value.len().is_multiple_of(2) {
        return Err("odd-length hex".into());
    }
    (0..value.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(&value[at..at + 2], 16).map_err(|error| error.to_string()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::fresh_counter;

    #[test]
    fn sequences_climb_and_hex_round_trips() {
        let first = fresh_counter();
        let second = fresh_counter();
        assert!(second > first);
        assert_eq!(
            hex_decode(&hex_encode(&[0, 255, 16])).unwrap(),
            vec![0, 255, 16]
        );
        assert!(hex_decode("abc").is_err());
        assert!(
            hex_decode("a\u{e9}a").is_err(),
            "a byte pair splits the \u{e9}"
        );
        assert!(
            hex_decode("\u{e9}\u{e9}").is_err(),
            "whole characters, not hex"
        );
    }
}
