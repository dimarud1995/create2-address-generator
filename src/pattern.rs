//! Address pattern matching and difficulty estimation.
//!
//! Patterns operate on the 20-byte address (40 hex nibbles). Supported, combinable with AND:
//!   - prefix  : leading hex nibbles
//!   - suffix  : trailing hex nibbles
//!   - leading-zeros : N leading 0x00 bytes (gas-golf)
//!   - regex   : match anywhere against the 40-char lowercase hex
//!   - checksum: require EIP-55 mixed-case to match the cased prefix/suffix
//!
//! Difficulty is reported in bits: log2(expected attempts) = -log2(p).

use anyhow::{bail, Result};
use regex::Regex;
use tiny_keccak::{Hasher, Keccak};

#[derive(Clone)]
pub struct PatternSet {
    /// (nibble_index 0..40, required value 0..16) — the fast byte/nibble filter.
    constraints: Vec<(usize, u8)>,
    regex: Option<Regex>,
    /// Cased prefix/suffix for EIP-55 checksum verification, as (nibble_index, ascii_char).
    checksum_chars: Vec<(usize, u8)>,
    checksum: bool,
    /// Human-readable description of the active pattern.
    pub describe: String,
}

fn hex_to_nibbles(s: &str) -> Result<Vec<u8>> {
    let s = s.strip_prefix("0x").unwrap_or(s);
    let mut out = Vec::with_capacity(s.len());
    for c in s.chars() {
        let v = c
            .to_digit(16)
            .ok_or_else(|| anyhow::anyhow!("invalid hex char '{c}' in pattern"))?;
        out.push(v as u8);
    }
    Ok(out)
}

impl PatternSet {
    pub fn build(
        prefix: Option<&str>,
        suffix: Option<&str>,
        leading_zeros: Option<usize>,
        regex: Option<&str>,
        checksum: bool,
    ) -> Result<Self> {
        // Per-nibble constraint, deduped: index -> value. Detect contradictions.
        let mut map: std::collections::BTreeMap<usize, u8> = std::collections::BTreeMap::new();
        let put = |idx: usize, val: u8, map: &mut std::collections::BTreeMap<usize, u8>| -> Result<()> {
            if idx >= 40 {
                bail!("pattern exceeds address length (40 hex nibbles)");
            }
            if let Some(&prev) = map.get(&idx) {
                if prev != val {
                    bail!("contradictory pattern at nibble {idx}: {prev:x} vs {val:x} (impossible to satisfy)");
                }
            }
            map.insert(idx, val);
            Ok(())
        };

        let mut checksum_chars = Vec::new();
        let mut parts = Vec::new();

        if let Some(n) = leading_zeros {
            if n > 20 {
                bail!("leading-zeros cannot exceed 20 bytes");
            }
            for i in 0..(n * 2) {
                put(i, 0, &mut map)?;
            }
            if n > 0 {
                parts.push(format!("leading-zeros={n}"));
            }
        }

        if let Some(p) = prefix {
            let nibs = hex_to_nibbles(p)?;
            let cased: Vec<u8> = p.strip_prefix("0x").unwrap_or(p).bytes().collect();
            for (i, v) in nibs.iter().enumerate() {
                put(i, *v, &mut map)?;
                if checksum {
                    checksum_chars.push((i, cased[i]));
                }
            }
            parts.push(format!("prefix=0x{}", p.strip_prefix("0x").unwrap_or(p)));
        }

        if let Some(s) = suffix {
            let nibs = hex_to_nibbles(s)?;
            let cased: Vec<u8> = s.strip_prefix("0x").unwrap_or(s).bytes().collect();
            let start = 40 - nibs.len();
            for (i, v) in nibs.iter().enumerate() {
                put(start + i, *v, &mut map)?;
                if checksum {
                    checksum_chars.push((start + i, cased[i]));
                }
            }
            parts.push(format!("suffix=...{}", s.strip_prefix("0x").unwrap_or(s)));
        }

        let regex = match regex {
            Some(r) => {
                parts.push(format!("regex=/{r}/"));
                Some(Regex::new(r)?)
            }
            None => None,
        };

        if checksum {
            parts.push("checksum".to_string());
        }

        if map.is_empty() && regex.is_none() {
            bail!("no pattern specified — provide at least one of --starts-with/--ends-with/--leading-zeros/--regex");
        }

        Ok(PatternSet {
            constraints: map.into_iter().collect(),
            regex,
            checksum_chars,
            checksum,
            describe: if parts.is_empty() { "any".into() } else { parts.join(" ") },
        })
    }

    #[inline]
    fn nibble(addr: &[u8; 20], idx: usize) -> u8 {
        let byte = addr[idx / 2];
        if idx % 2 == 0 {
            byte >> 4
        } else {
            byte & 0x0f
        }
    }

    /// Fast nibble/byte filter — the part the GPU can do.
    #[inline]
    pub fn matches_fast(&self, addr: &[u8; 20]) -> bool {
        for &(idx, val) in &self.constraints {
            if Self::nibble(addr, idx) != val {
                return false;
            }
        }
        true
    }

    /// Full match including regex + EIP-55 checksum (CPU-side refinement).
    pub fn matches(&self, addr: &[u8; 20]) -> bool {
        if !self.matches_fast(addr) {
            return false;
        }
        if self.regex.is_some() || self.checksum {
            let lower = hex_lower(addr);
            if let Some(re) = &self.regex {
                if !re.is_match(&lower) {
                    return false;
                }
            }
            if self.checksum {
                let cs = eip55(&lower);
                for &(idx, want) in &self.checksum_chars {
                    if cs[idx] != want {
                        return false;
                    }
                }
            }
        }
        true
    }

    /// Difficulty in bits = -log2(probability a random address matches).
    /// Returns None for regex (estimated separately via sampling).
    pub fn difficulty_bits(&self) -> Option<f64> {
        if self.regex.is_some() {
            return None;
        }
        // Each constrained nibble: 4 bits. Each cased letter under checksum: +1 bit.
        let mut bits = self.constraints.len() as f64 * 4.0;
        if self.checksum {
            for &(_, c) in &self.checksum_chars {
                if c.is_ascii_alphabetic() {
                    bits += 1.0; // letters carry a case bit; digits 0-9 do not
                }
            }
        }
        Some(bits)
    }

    /// Per-nibble EIP-55 case constraints for the GPU. Returns (check, want, any):
    /// `check[i]==1` means nibble i's letter-case is constrained; `want[i]==1` means it
    /// must be uppercase (hash nibble >= 8), 0 means lowercase. `any` is false when there's
    /// nothing to check (no `--checksum`, or only digit positions which have no case).
    pub fn case_filter(&self) -> ([u8; 40], [u8; 40], bool) {
        let mut check = [0u8; 40];
        let mut want = [0u8; 40];
        let mut any = false;
        if self.checksum {
            for &(idx, c) in &self.checksum_chars {
                if c.is_ascii_alphabetic() {
                    check[idx] = 1;
                    want[idx] = if c.is_ascii_uppercase() { 1 } else { 0 };
                    any = true;
                }
            }
        }
        (check, want, any)
    }

    /// Express the nibble constraints as a byte-level (mask, want) pair for the GPU:
    /// a match requires `(addr[i] & mask[i]) == want[i]` for all i.
    pub fn byte_filter(&self) -> ([u8; 20], [u8; 20]) {
        let mut mask = [0u8; 20];
        let mut want = [0u8; 20];
        for &(idx, val) in &self.constraints {
            let b = idx / 2;
            if idx % 2 == 0 {
                mask[b] |= 0xf0;
                want[b] |= val << 4;
            } else {
                mask[b] |= 0x0f;
                want[b] |= val;
            }
        }
        (mask, want)
    }
}

pub fn hex_lower(addr: &[u8; 20]) -> String {
    hex::encode(addr)
}

/// EIP-55 checksummed hex (no 0x), as 40 ascii bytes.
pub fn eip55(lower_hex: &str) -> Vec<u8> {
    let mut k = Keccak::v256();
    k.update(lower_hex.as_bytes());
    let mut hash = [0u8; 32];
    k.finalize(&mut hash);
    lower_hex
        .bytes()
        .enumerate()
        .map(|(i, c)| {
            if c.is_ascii_digit() {
                c
            } else {
                let nib = if i % 2 == 0 { hash[i / 2] >> 4 } else { hash[i / 2] & 0x0f };
                if nib >= 8 {
                    c.to_ascii_uppercase()
                } else {
                    c
                }
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suffix_match_and_difficulty() {
        let p = PatternSet::build(None, Some("00123"), None, None, false).unwrap();
        assert_eq!(p.difficulty_bits(), Some(20.0)); // 5 nibbles * 4
        let mut addr = [0u8; 20];
        addr[17] = 0x00;
        addr[18] = 0x01;
        addr[19] = 0x23;
        assert!(p.matches(&addr));
        addr[19] = 0x24;
        assert!(!p.matches(&addr));
    }

    #[test]
    fn leading_zeros() {
        let p = PatternSet::build(None, None, Some(2), None, false).unwrap();
        assert_eq!(p.difficulty_bits(), Some(16.0)); // 2 bytes = 4 nibbles
        let mut addr = [0xffu8; 20];
        assert!(!p.matches(&addr));
        addr[0] = 0;
        addr[1] = 0;
        assert!(p.matches(&addr));
    }

    #[test]
    fn checksum_case_filter_and_difficulty() {
        // "Ab" prefix with checksum: 2 raw nibbles (8 bits) + 2 cased letters (2 bits) = 10.
        let p = PatternSet::build(Some("Ab"), None, None, None, true).unwrap();
        assert_eq!(p.difficulty_bits(), Some(10.0));
        let (check, want, any) = p.case_filter();
        assert!(any);
        assert_eq!((check[0], want[0]), (1, 1)); // 'A' must be uppercase
        assert_eq!((check[1], want[1]), (1, 0)); // 'b' must be lowercase
        assert_eq!(check[2], 0); // unconstrained
    }

    #[test]
    fn checksum_matches_respects_case() {
        // EIP-55: 0x5aaeb6...  -> checksum 0x5aAeb6053F3E...  (pos1='a' lower, pos2='A' upper)
        let addr_bytes = hex::decode("5aaeb6053f3e94c9b9a09f33669435e7ef1beaed").unwrap();
        let mut addr = [0u8; 20];
        addr.copy_from_slice(&addr_bytes);

        // "5aAe" matches the real checksum case at positions 0-3.
        let good = PatternSet::build(Some("5aAe"), None, None, None, true).unwrap();
        assert!(good.matches(&addr));

        // "5aAE" demands uppercase 'E' at pos 3, but checksum has lowercase 'e' -> no match.
        let bad = PatternSet::build(Some("5aAE"), None, None, None, true).unwrap();
        assert!(!bad.matches(&addr));

        // Without --checksum, case is ignored: raw hex prefix still matches.
        let raw = PatternSet::build(Some("5AAE"), None, None, None, false).unwrap();
        assert!(raw.matches(&addr));
    }

    #[test]
    fn eip55_known() {
        // Vitalik's checksum example fragment
        let lower = "5aaeb6053f3e94c9b9a09f33669435e7ef1beaed";
        let cs = String::from_utf8(eip55(lower)).unwrap();
        assert_eq!(cs, "5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAed");
    }
}
