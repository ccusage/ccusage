use std::borrow::Cow;

use super::data;

const MODEL_DATE_SUFFIX_DIGITS: usize = 8;

pub(super) fn with_names<T>(model: &str, mut choose: impl FnMut(&str) -> Option<T>) -> Option<T> {
    let mut spelled = |name: &str| {
        choose(name).or_else(|| alternate_spellings(name).find_map(|spelling| choose(&spelling)))
    };
    spelled(model).or_else(|| {
        let alias = crate::model_aliases::resolve_model_name(model);
        if alias == model {
            return None;
        }
        spelled(alias.as_ref())
    })
}

// NOTE(2026-11-21, #2226): Every lookup, for a logged name and for a configured alias's target alike, tries the same other spellings in the same order: the vendor's own spelling, then the built-in alias. Keeping them in one place stops the override-base and resolver paths from drifting apart.
pub(super) fn alternate_spellings(model: &str) -> impl Iterator<Item = Cow<'static, str>> {
    vendor_spelling(model)
        .map(|vendor| Cow::Owned(vendor.into_string()))
        .into_iter()
        .chain(pricing_alias(model).map(Cow::Borrowed))
}

pub(super) fn unlabeled_model(label: &str) -> &str {
    label
        .strip_prefix('[')
        .and_then(|label| label.split_once("] "))
        .map_or(label, |(_, model)| model)
}

pub(super) fn last_segment(key: &str) -> &str {
    key.rsplit_once('/').map_or(key, |(_, segment)| segment)
}

pub(super) fn pricing_key_matches(candidate: &str, model: &str, normalized_model: &str) -> bool {
    names_same_model::<false>(model, model, candidate)
        || names_same_model::<true>(normalized_model, model, candidate)
}

pub(super) fn names_pricing_key(candidate: &str, model: &str, normalized_model: &str) -> bool {
    contains_pricing_key::<false>(model, model, candidate)
        || contains_pricing_key::<true>(normalized_model, model, candidate)
}

// NOTE(2026-11-20, #2236): Either id may carry only a host or provider qualifier in front of the other, so a word in front, as in `chat-gpt-5.4` for `gpt-5.4`, makes it another model. The qualifier is read from the raw id because the identity spells `openai.` as `openai-`. Only the logged id may add a price-neutral suffix: a catalogue lists a dated, `[1m]` or effort-level id because it prices that variant itself.
fn names_same_model<const NORMALIZE: bool>(model: &str, raw_model: &str, key: &str) -> bool {
    if model.len() >= key.len() {
        contains_pricing_key::<NORMALIZE>(model, raw_model, key)
    } else {
        let start = key.len() - model.len();
        is_qualifier_end(key.as_bytes()[start - 1])
            && key
                .get(start..)
                .is_some_and(|suffix| key_bytes_match::<NORMALIZE>(suffix, model))
    }
}

fn contains_pricing_key<const NORMALIZE: bool>(value: &str, raw_value: &str, key: &str) -> bool {
    key_positions::<NORMALIZE>(value, key).any(|index| {
        let before = index
            .checked_sub(1)
            .and_then(|before| raw_value.as_bytes().get(before))
            .copied();
        before.is_none_or(is_qualifier_end)
            && is_price_neutral_suffix(&value[index + key.len()..], family_words(key))
    })
}

fn key_bytes_match<const NORMALIZE: bool>(key: &str, identity: &str) -> bool {
    if NORMALIZE {
        normalized_key_bytes(key).eq(identity.bytes())
    } else {
        key == identity
    }
}

// NOTE(2026-11-21, #2226): The fuzzy pass tests every catalogue key against a short model id, and building a substring searcher per pair cost more than the search. Scanning for the key's first byte and comparing from there finds the same non-overlapping positions as `str::match_indices`.
fn key_positions<'a, const NORMALIZE: bool>(
    text: &'a str,
    key: &'a str,
) -> impl Iterator<Item = usize> + 'a {
    let value = text.as_bytes();
    let mut start = 0;
    std::iter::from_fn(move || {
        let Some((&first, _)) = key.as_bytes().split_first() else {
            let index = start;
            let next = text.get(index..)?.chars().next();
            start = next.map_or(text.len() + 1, |char| index + char.len_utf8());
            return Some(index);
        };
        let first = if NORMALIZE {
            normalize_key_byte(first)
        } else {
            first
        };
        while start + key.len() <= value.len() {
            let index = start + memchr::memchr(first, &value[start..=value.len() - key.len()])?;
            let matched = if NORMALIZE {
                normalized_key_bytes(key).eq(value[index..index + key.len()].iter().copied())
            } else {
                value[index..].starts_with(key.as_bytes())
            };
            if matched {
                start = index + key.len();
                return Some(index);
            }
            start = index + 1;
        }
        None
    })
}

pub(super) fn is_qualifier_end(byte: u8) -> bool {
    !byte.is_ascii_alphanumeric() && !matches!(byte, b'-' | b'_')
}

pub(super) fn starts_price_neutral_suffix(byte: u8) -> bool {
    matches!(byte, b'[' | b'-' | b'.' | b'_' | b':' | b'@')
}

fn is_price_neutral_suffix(suffix: &str, family: &[&str]) -> bool {
    let mut rest = suffix;
    while let Some(&separator) = rest.as_bytes().first() {
        if separator == b'[' {
            let Some(end) = rest.find(']') else {
                return false;
            };
            rest = &rest[end + 1..];
            continue;
        }
        if !starts_price_neutral_suffix(separator) {
            return false;
        }
        let word_len = rest[1..]
            .bytes()
            .take_while(u8::is_ascii_alphanumeric)
            .count();
        let (word, after) = rest[1..].split_at(word_len);
        rest = if let Some(after) = date_suffix_end(word, after) {
            after
        } else if let Some(after) = deployment_revision_end(word, after) {
            after
        } else if let Some(after) = price_neutral_word_end(&rest[1..], family) {
            after
        } else {
            return false;
        };
    }
    true
}

fn family_words(key: &str) -> &'static [&'static str] {
    let segment = last_segment(key);
    let names = std::iter::once(segment).chain(
        segment
            .match_indices('.')
            .map(|(index, _)| &segment[index + 1..]),
    );
    data::FAMILY_PRICE_NEUTRAL_SUFFIX_WORDS
        .iter()
        .find(|(family, _)| names.clone().any(|name| names_family(name, family)))
        .map_or(&[], |(_, words)| words)
}

// NOTE(2026-11-20): Lookup treats `.` and `@` in a key as `-`, so a `.` in a family name such as `qwen3.5-plus` must also match `qwen3-5-plus`. A `-` in a family name stays literal, so a Bedrock qualifier such as `mistral.devstral-…` never names the `mistral-` family.
fn names_family(name: &str, family: &str) -> bool {
    name.len() >= family.len()
        && name.bytes().zip(family.bytes()).all(|(byte, expected)| {
            byte == expected
                || (expected == b'.' && (byte == b'-' || is_pricing_key_separator(byte)))
        })
}

fn price_neutral_word_end<'a>(rest: &'a str, family: &[&str]) -> Option<&'a str> {
    data::PRICE_NEUTRAL_SUFFIX_WORDS
        .iter()
        .chain(family)
        .find_map(|word| {
            let after = rest.strip_prefix(word)?;
            (!after
                .bytes()
                .next()
                .is_some_and(|byte| byte.is_ascii_alphanumeric()))
            .then_some(after)
        })
}

fn date_suffix_end<'a>(word: &str, after: &'a str) -> Option<&'a str> {
    if is_digits(word, MODEL_DATE_SUFFIX_DIGITS) {
        return Some(after);
    }
    let month = after.strip_prefix('-')?.get(..2)?;
    let day = after.get(3..)?.strip_prefix('-')?.get(..2)?;
    let rest = &after[6..];
    (is_digits(word, 4)
        && is_digits(month, 2)
        && is_digits(day, 2)
        && !rest
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_alphanumeric()))
    .then_some(rest)
}

fn deployment_revision_end<'a>(word: &str, after: &'a str) -> Option<&'a str> {
    let revision = word.strip_prefix('v')?;
    let minor = after.strip_prefix(':')?;
    let minor_len = minor.bytes().take_while(u8::is_ascii_digit).count();
    (is_digits(revision, revision.len().max(1)) && minor_len > 0).then(|| &minor[minor_len..])
}

fn is_digits(value: &str, len: usize) -> bool {
    value.len() == len && value.bytes().all(|byte| byte.is_ascii_digit())
}

pub(super) fn is_pricing_key_separator(byte: u8) -> bool {
    matches!(byte, b'.' | b'@')
}

fn normalize_key_byte(byte: u8) -> u8 {
    if is_pricing_key_separator(byte) {
        b'-'
    } else {
        byte
    }
}

pub(super) fn normalized_key_bytes(value: &str) -> impl Iterator<Item = u8> + '_ {
    value.bytes().map(normalize_key_byte)
}

pub(super) fn dash_pricing_key_separators(chars: impl Iterator<Item = char>) -> String {
    chars
        .map(|char| if matches!(char, '.' | '@') { '-' } else { char })
        .collect()
}

pub(super) fn normalized_pricing_key(value: &str) -> Cow<'_, str> {
    if value.bytes().any(is_pricing_key_separator) {
        let dashed = normalized_key_bytes(value).collect();
        Cow::Owned(String::from_utf8(dashed).expect("replacing ASCII bytes keeps UTF-8"))
    } else {
        Cow::Borrowed(value)
    }
}

pub(super) fn model_without_date_suffix(model: &str) -> &str {
    let bytes = model.as_bytes();
    if model.len() > 11 {
        let suffix = &bytes[model.len() - 11..];
        if suffix[0] == b'-'
            && suffix[1..5].iter().all(u8::is_ascii_digit)
            && suffix[5] == b'-'
            && suffix[6..8].iter().all(u8::is_ascii_digit)
            && suffix[8] == b'-'
            && suffix[9..].iter().all(u8::is_ascii_digit)
        {
            return &model[..model.len() - 11];
        }
    }
    if model.len() > 9 {
        let suffix = &bytes[model.len() - 9..];
        if suffix[0] == b'-' && suffix[1..].iter().all(u8::is_ascii_digit) {
            return &model[..model.len() - 9];
        }
    }
    model
}

fn vendor_spelling(model: &str) -> Option<Box<str>> {
    data::VENDOR_SPELLINGS
        .iter()
        .find_map(|(spelling, vendor)| {
            let id = model.strip_prefix(spelling)?.strip_prefix('/')?;
            Some(format!("{vendor}/{id}").into_boxed_str())
        })
}

pub(super) fn pricing_alias(model: &str) -> Option<&'static str> {
    let fold = |byte| {
        if is_pricing_key_separator(byte) {
            b'-'
        } else {
            byte
        }
    };
    data::ALIASES.iter().find_map(|(key, alias)| {
        (key.len() == model.len() && key.bytes().map(fold).eq(model.bytes().map(fold)))
            .then_some(*alias)
    })
}

pub(super) fn matches_model_suffix(part: &str, base: &str) -> bool {
    if base.is_empty() {
        return true;
    }
    let Some(index) = part.rfind(base) else {
        return false;
    };
    part[..index]
        .chars()
        .next_back()
        .is_none_or(|previous| !previous.is_alphanumeric())
        && part
            .as_bytes()
            .get(index + base.len())
            .is_none_or(|&next| next == b'-')
}

#[cfg(test)]
mod tests {
    #[test]
    fn key_positions_match_str_match_indices() {
        for (value, key) in [
            ("gemini-mystery-1", "gemini-"),
            ("openrouter/google/gemini-2-5-flash", "gemini-2-5-flash"),
            ("aaaa", "aa"),
            ("abc", "abcd"),
            ("gpt-5-4-codex", "gpt-5-4"),
            ("x", "x"),
            ("unlisted[1m]", ""),
            ("é1", ""),
            ("", ""),
        ] {
            let expected: Vec<_> = value.match_indices(key).map(|(index, _)| index).collect();
            assert_eq!(
                super::key_positions::<false>(value, key).collect::<Vec<_>>(),
                expected,
                "{value} {key}"
            );
        }
    }

    #[test]
    fn normalized_key_positions_match_materialized_identity() {
        for (value, key) in [
            ("x-é-5-é-5", "é@5"),
            ("model-1-2[1m]", "model.1@2"),
            ("------", ".@"),
            ("foo-bar", "@baz"),
            ("é1", ""),
        ] {
            let identity = super::normalized_pricing_key(key);
            let expected: Vec<_> = value
                .match_indices(identity.as_ref())
                .map(|(index, _)| index)
                .collect();
            assert_eq!(
                super::key_positions::<true>(value, key).collect::<Vec<_>>(),
                expected,
                "{value} {key}"
            );
        }
    }
}
