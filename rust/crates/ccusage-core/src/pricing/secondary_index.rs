use super::{
    PricingRows,
    keys::{is_qualifier_end, starts_price_neutral_suffix},
    rows::folded_identity,
};

const FILTER_BYTES: usize = 8192;
pub(super) const INDEX_BYTES: usize = FILTER_BYTES + 1;

#[derive(Clone, Copy, Debug)]
pub(super) struct SecondaryIndex(&'static [u8]);

impl SecondaryIndex {
    pub(super) fn new(bytes: &'static [u8]) -> Self {
        assert_eq!(bytes.len(), INDEX_BYTES);
        Self(bytes)
    }

    pub(super) fn encode(rows: &PricingRows) -> Vec<u8> {
        let mut index = vec![0; INDEX_BYTES];
        for (_, key) in rows.keys() {
            index[FILTER_BYTES] |= u8::from(key.is_empty());
            // NOTE(2026-11-21, #2465): A qualified catalogue key can match a bare query. Index every qualified suffix so a negative result can safely skip the inflate.
            for start in qualifier_boundaries(key) {
                for bit in bits(&key[start..]) {
                    index[bit / 8] |= 1 << (bit % 8);
                }
            }
        }
        index
    }

    pub(super) fn may_contain(self, key: &str) -> bool {
        bits(key).all(|bit| self.0[bit / 8] & (1 << (bit % 8)) != 0)
    }

    pub(super) fn may_match_fuzzy(self, key: &str) -> bool {
        self.0[FILTER_BYTES] != 0
            || qualifier_boundaries(key).any(|start| {
                let suffix = &key[start..];
                self.may_contain(suffix)
                    || suffix
                        .char_indices()
                        .skip(1)
                        .filter(|(end, _)| starts_price_neutral_suffix(suffix.as_bytes()[*end]))
                        .any(|(end, _)| self.may_contain(&suffix[..end]))
            })
    }
}

fn qualifier_boundaries(key: &str) -> impl Iterator<Item = usize> + '_ {
    std::iter::once(0).chain(
        key.char_indices()
            .map(|(start, _)| start)
            .skip(1)
            .chain((!key.is_empty()).then_some(key.len()))
            .filter(|&start| is_qualifier_end(key.as_bytes()[start - 1])),
    )
}

fn bits(key: &str) -> impl Iterator<Item = usize> {
    let hash = folded_identity(key).fold(0xcbf29ce484222325u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
    });
    (0..4).map(move |index| (hash >> (16 * index)) as usize & (FILTER_BYTES * 8 - 1))
}
