//! CREATE2 address derivation.
//!
//! Per EIP-1014:
//! ```text
//! address = keccak256( 0xff ++ deployer ++ salt ++ keccak256(init_code) )[12:]
//! ```
//! The preimage is exactly 85 bytes: 1 (0xff) + 20 (deployer) + 32 (salt) + 32 (initCodeHash).

use tiny_keccak::{Hasher, Keccak};

pub const PREIMAGE_LEN: usize = 1 + 20 + 32 + 32; // 85

/// keccak256 of arbitrary bytes.
pub fn keccak256(data: &[u8]) -> [u8; 32] {
    let mut k = Keccak::v256();
    k.update(data);
    let mut out = [0u8; 32];
    k.finalize(&mut out);
    out
}

/// Build the 85-byte CREATE2 preimage with the salt slot zeroed.
/// Salt occupies bytes [21, 53).
pub fn preimage_template(deployer: &[u8; 20], init_code_hash: &[u8; 32]) -> [u8; PREIMAGE_LEN] {
    let mut buf = [0u8; PREIMAGE_LEN];
    buf[0] = 0xff;
    buf[1..21].copy_from_slice(deployer);
    // [21..53] salt — left zero, filled per attempt
    buf[53..85].copy_from_slice(init_code_hash);
    buf
}

/// Absorb the padded 85-byte preimage into a 25-lane Keccak state, WITHOUT permuting.
/// The GPU kernel copies this, overwrites lane 4 with its salt counter, then permutes —
/// so the expensive per-byte absorb happens once on the host instead of per attempt.
/// Requires the salt counter region (preimage bytes [32..40] = lane 4) to be zero here.
pub fn keccak_absorb_state(template: &[u8; PREIMAGE_LEN]) -> [u64; 25] {
    let mut block = [0u8; 136]; // keccak256 rate
    block[..PREIMAGE_LEN].copy_from_slice(template);
    block[PREIMAGE_LEN] ^= 0x01; // padding start (byte 85)
    block[135] ^= 0x80; // padding end (last rate byte)
    let mut st = [0u64; 25];
    for pos in 0..136 {
        st[pos / 8] ^= (block[pos] as u64) << (8 * (pos % 8));
    }
    st
}

/// Compute the deployed address for a fully-formed preimage (salt already written).
pub fn address_from_preimage(preimage: &[u8; PREIMAGE_LEN]) -> [u8; 20] {
    let h = keccak256(preimage);
    let mut addr = [0u8; 20];
    addr.copy_from_slice(&h[12..32]);
    addr
}

/// Convenience: full computation from parts. (Used by tests / as a library helper.)
#[allow(dead_code)]
pub fn create2_address(
    deployer: &[u8; 20],
    salt: &[u8; 32],
    init_code_hash: &[u8; 32],
) -> [u8; 20] {
    let mut buf = preimage_template(deployer, init_code_hash);
    buf[21..53].copy_from_slice(salt);
    address_from_preimage(&buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    // EIP-1014 example #1: deployer 0x0000...0000, salt 0, code 0x00
    #[test]
    fn eip1014_vector_1() {
        let deployer = [0u8; 20];
        let salt = [0u8; 32];
        let init_code_hash = keccak256(&[0x00]);
        let addr = create2_address(&deployer, &salt, &init_code_hash);
        assert_eq!(
            hex::encode(addr),
            "4d1a2e2bb4f88f0250f26ffff098b0b30b26bf38"
        );
    }
}
