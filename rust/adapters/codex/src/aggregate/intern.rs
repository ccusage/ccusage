use std::num::NonZeroU32;

use compact_str::CompactString;

use crate::{Result, fast::FxHashMap};

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
pub(super) struct StringId(NonZeroU32);

impl StringId {
    pub(super) fn index(self) -> usize {
        self.0.get() as usize - 1
    }
}

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
            u32::try_from(self.values.len())
                .ok()
                .and_then(|len| len.checked_add(1))
                .and_then(NonZeroU32::new)
                .ok_or_else(|| crate::cli_error("too many interned Codex strings"))?,
        );
        self.values.push(create());
        self.ids.insert(CompactString::new(value), id);
        Ok(id)
    }
}
