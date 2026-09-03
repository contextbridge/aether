use sha2::{Digest, Sha256};

pub(crate) fn sha256_hex(content: &str) -> String {
    base16ct::lower::encode_string(&Sha256::digest(content.as_bytes()))
}
