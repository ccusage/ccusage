use std::num::NonZeroUsize;

use compact_str::CompactString;

use crate::{Result, fast::FxHashMap};

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
pub(super) struct StringId(pub(super) NonZeroUsize);

pub(super) struct StringInterner<T> {
    ids: FxHashMap<CompactString, StringId>,
    pub(super) values: Vec<T>,
}

impl<T> Default for StringInterner<T> {
    fn default() -> Self {
        Self {
            ids: FxHashMap::default(),
            values: Vec::new(),
        }
    }
}

impl<T> StringInterner<T> {
    pub(super) fn intern(&mut self, value: &str, create: impl FnOnce() -> T) -> Result<StringId> {
        if let Some(id) = self.ids.get(value) {
            return Ok(*id);
        }
        let id = StringId(
            self.values
                .len()
                .checked_add(1)
                .and_then(NonZeroUsize::new)
                .ok_or_else(|| crate::cli_error("too many interned Codex strings"))?,
        );
        self.values.push(create());
        self.ids.insert(CompactString::new(value), id);
        Ok(id)
    }
}
