//! Optional output: Keccak256(domain || VK id || field count || fields).
//! VK id is SHA256 of the exact embedded VK JSON. Fields and u64 count are
//! little endian. This is a versioned Mizu format, not OpenVM/Ethereum ABI.
use ark_ff::PrimeField;
use sha3::{Digest, Keccak256};
use crate::types::StepField;

pub fn statement_digest(vk_id: &[u8; 32], statement: &[StepField]) -> [u8; 32] {
    let mut hash = Keccak256::new();
    hash.update(b"mizu/verified-statement/v1");
    hash.update(vk_id);
    hash.update((statement.len() as u64).to_le_bytes());
    for field in statement {
        for limb in field.into_bigint().0 { hash.update(limb.to_le_bytes()); }
    }
    hash.finalize().into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ark_ff::{One, Zero};
    #[test]
    fn digest_matches_wire_and_binds_key_length_order_and_fields() {
        let key = [7; 32];
        let fields = [StepField::one(), StepField::from(256u64)];
        let mut wire = b"mizu/verified-statement/v1".to_vec();
        wire.extend(key); wire.extend(2u64.to_le_bytes());
        let mut a = [0; 32]; a[0] = 1;
        let mut b = [0; 32]; b[1] = 1;
        wire.extend(a); wire.extend(b);
        let expected: [u8; 32] = Keccak256::digest(wire).into();
        assert_eq!(statement_digest(&key, &fields), expected);
        assert_ne!(expected, statement_digest(&[8; 32], &fields));
        assert_ne!(expected, statement_digest(&key, &[fields[1], fields[0]]));
        assert_ne!(expected, statement_digest(&key, &fields[..1]));
        assert_ne!(expected, statement_digest(&key, &[fields[0], StepField::zero()]));
        assert_ne!(statement_digest(&key, &[]), statement_digest(&key, &[StepField::zero()]));
    }
}
