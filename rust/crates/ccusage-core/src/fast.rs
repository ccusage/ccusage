use memchr::memchr_iter;

mod line_reader;
mod prefilter;

pub use line_reader::LineReader;
pub use prefilter::LinePrefilter;

pub type FxHashMap<K, V> = rustc_hash::FxHashMap<K, V>;
pub type FxHashSet<T> = rustc_hash::FxHashSet<T>;

pub fn newline_offsets(bytes: &[u8]) -> impl DoubleEndedIterator<Item = usize> + '_ {
    memchr_iter(b'\n', bytes)
}

pub fn suffix_string(value: &str, suffix: &str) -> String {
    let mut output = String::with_capacity(value.len() + suffix.len());
    output.push_str(value);
    output.push_str(suffix);
    output
}

#[cfg(test)]
mod tests;
