//! Hashes, shingles and MinHash signatures. Everything here is a pure function of token hashes,
//! and the hash functions are fixed, so a fingerprint can be stored and compared later.

/// Tokens in one shingle.
pub const SHINGLE: usize = 4;
pub const BANDS: usize = 32;
pub const ROWS: usize = 4;
pub const SIGNATURE: usize = BANDS * ROWS;

const SEED: u64 = 0xcbf2_9ce4_8422_2325;

/// The splitmix64 finalizer.
pub fn mix(value: u64) -> u64 {
    let mut z = value.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// FNV-1a over the bytes, then mixed.
pub fn hash_bytes(bytes: &[u8]) -> u64 {
    mix(bytes.iter().fold(SEED, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01b3)
    }))
}

/// An order-sensitive hash of a sequence of hashes.
pub fn hash_sequence(hashes: &[u64]) -> u64 {
    hashes.iter().fold(SEED, |hash, item| mix(hash ^ item))
}

/// The set of shingle hashes, sorted and without duplicates. A sequence shorter than one shingle
/// gives one shingle of the whole sequence.
pub fn shingles(tokens: &[u64]) -> Vec<u64> {
    let mut set: Vec<u64> = if tokens.len() < SHINGLE {
        vec![hash_sequence(tokens)]
    } else {
        tokens.windows(SHINGLE).map(hash_sequence).collect()
    };
    set.sort_unstable();
    set.dedup();
    set
}

pub fn minhash(shingles: &[u64]) -> Vec<u64> {
    (0..SIGNATURE as u64)
        .map(|index| {
            let seed = mix(index + 1);
            shingles
                .iter()
                .map(|shingle| mix(shingle ^ seed))
                .min()
                .unwrap_or(u64::MAX)
        })
        .collect()
}

/// The Jaccard similarity of two sorted sets.
pub fn jaccard(a: &[u64], b: &[u64]) -> f64 {
    let (mut i, mut j, mut shared) = (0, 0, 0usize);
    while i < a.len() && j < b.len() {
        match a[i].cmp(&b[j]) {
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
            std::cmp::Ordering::Equal => {
                shared += 1;
                i += 1;
                j += 1;
            }
        }
    }
    let union = a.len() + b.len() - shared;
    if union == 0 {
        0.0
    } else {
        shared as f64 / union as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sequence_hash_depends_on_the_order() {
        assert_ne!(hash_sequence(&[1, 2, 3]), hash_sequence(&[3, 2, 1]));
    }

    #[test]
    fn a_short_sequence_gives_one_shingle() {
        assert_eq!(shingles(&[1, 2]), vec![hash_sequence(&[1, 2])]);
    }

    #[test]
    fn shingles_are_a_sorted_set() {
        let set = shingles(&[7, 7, 7, 7, 7, 7]);
        assert_eq!(set, vec![hash_sequence(&[7, 7, 7, 7])]);
        let set = shingles(&[1, 2, 3, 4, 5, 6]);
        assert_eq!(set.len(), 3);
        assert!(set.windows(2).all(|pair| pair[0] < pair[1]));
    }

    #[test]
    fn jaccard_counts_shared_over_union() {
        assert_eq!(jaccard(&[1, 2, 3], &[2, 3, 4]), 0.5);
        assert_eq!(jaccard(&[1, 2], &[1, 2]), 1.0);
        assert_eq!(jaccard(&[1], &[2]), 0.0);
        assert_eq!(jaccard(&[], &[]), 0.0);
    }

    #[test]
    fn equal_sets_have_equal_signatures() {
        let signature = minhash(&[5, 9, 11]);
        assert_eq!(signature.len(), SIGNATURE);
        assert_eq!(signature, minhash(&[5, 9, 11]));
        assert_ne!(signature, minhash(&[5, 9, 12]));
    }
}
