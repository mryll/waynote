use sha2::{Digest, Sha256};

/// Returns the lowercase hex SHA-256 of `bytes`.
pub fn content_hash(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input_produces_known_sha256() {
        // SHA-256("") = e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
        assert_eq!(
            content_hash(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn different_inputs_produce_different_hashes() {
        assert_ne!(content_hash(b"a"), content_hash(b"b"));
    }

    #[test]
    fn same_input_produces_same_hash_across_calls() {
        assert_eq!(content_hash(b"hello"), content_hash(b"hello"));
    }
}
