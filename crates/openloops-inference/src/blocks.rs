//! The closed block-kind catalog, per-block Unicode-scalar bounds, and the
//! transient block/range addressing that a future ADR-006 anchor
//! computation reads.
//!
//! `contracts/evidence/identity-boundary.json`
//! `email_record_schema.field_contracts[component_code].catalog`,
//! `canonical_anchor_contract.component_code_map`, and
//! `component_block_order`.

use crate::error::{BlockError, RangeError};

/// The closed `component_code_map` catalog this crate can produce blocks
/// for. `calendar_response` (code 9) is excluded: `component_block_order.
/// calendar_response` is "unavailable pending G-CAL"; this crate never
/// constructs a block or code for it.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
#[repr(u32)]
pub enum ComponentCode {
    Subject = 1,
    BodyBlock = 2,
    QuoteBlock = 3,
    Sender = 4,
    To = 5,
    Cc = 6,
    AttachmentName = 7,
    LinkLabel = 8,
}

impl ComponentCode {
    /// The exact `component_code_map[].code` value.
    #[must_use]
    pub const fn code(self) -> u32 {
        self as u32
    }
}

/// `canonical_anchor_contract.maximum_blocks_per_component`.
pub const MAX_BLOCKS_PER_COMPONENT: usize = 4096;
/// `canonical_anchor_contract.maximum_scalars_per_block`.
pub const MAX_SCALARS_PER_BLOCK: usize = 1_048_576;
/// `canonical_anchor_contract.prefix_context_scalars` /
/// `.suffix_context_scalars`.
pub const ANCHOR_CONTEXT_SCALARS: usize = 64;

/// One already-canonicalized (NFC, forbidden-scalar-checked) block of text.
///
/// Stores Unicode scalars (`char`), not bytes: `canonical_anchor_contract.
/// range_unit` is "zero-based Unicode scalar index", and a byte offset
/// would not address the same position as a scalar index for any
/// non-ASCII text.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CanonicalBlock {
    scalars: Vec<char>,
}

impl CanonicalBlock {
    /// Wraps already-canonicalized text, enforcing
    /// [`MAX_SCALARS_PER_BLOCK`].
    ///
    /// # Errors
    ///
    /// Returns [`BlockError::TooManyScalars`] over the bound.
    pub fn new(text: &str) -> Result<Self, BlockError> {
        let scalars: Vec<char> = text.chars().collect();
        if scalars.len() > MAX_SCALARS_PER_BLOCK {
            return Err(BlockError::TooManyScalars);
        }
        Ok(Self { scalars })
    }

    /// The block's Unicode scalar count.
    #[must_use]
    pub fn scalar_len(&self) -> usize {
        self.scalars.len()
    }

    /// Whether the block has zero scalars.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.scalars.is_empty()
    }

    /// Returns the exact canonical scalar text.
    #[must_use]
    pub fn as_string(&self) -> String {
        self.scalars.iter().collect()
    }

    /// Extracts one zero-based, exclusive-end Unicode scalar range.
    ///
    /// `canonical_anchor_contract.range_unit` / `.range_end` /
    /// `.empty_ranges`.
    ///
    /// # Errors
    ///
    /// [`RangeError::EmptyRange`] when `start >= end`;
    /// [`RangeError::OutOfBounds`] when `end` exceeds
    /// [`Self::scalar_len`].
    pub fn range_text(&self, start: u32, end: u32) -> Result<String, RangeError> {
        if start >= end {
            return Err(RangeError::EmptyRange);
        }
        let end_usize = end as usize;
        if end_usize > self.scalars.len() {
            return Err(RangeError::OutOfBounds);
        }
        Ok(self.scalars[start as usize..end_usize].iter().collect())
    }

    /// The available prefix immediately before `start`, up to
    /// `max_scalars`, never crossing this block's start.
    ///
    /// `canonical_anchor_contract.context_edges`: "never cross a block
    /// boundary".
    #[must_use]
    pub fn prefix_context(&self, start: u32, max_scalars: usize) -> String {
        let start_usize = (start as usize).min(self.scalars.len());
        let from = start_usize.saturating_sub(max_scalars);
        self.scalars[from..start_usize].iter().collect()
    }

    /// The available suffix immediately after `end`, up to `max_scalars`,
    /// never crossing this block's end.
    #[must_use]
    pub fn suffix_context(&self, end: u32, max_scalars: usize) -> String {
        let end_usize = (end as usize).min(self.scalars.len());
        let to = end_usize
            .saturating_add(max_scalars)
            .min(self.scalars.len());
        self.scalars[end_usize..to].iter().collect()
    }
}

/// A bounded, ordered catalog of [`CanonicalBlock`]s for one
/// [`ComponentCode`].
#[derive(Clone, Debug, Eq, PartialEq, Default)]
pub struct BlockList {
    blocks: Vec<CanonicalBlock>,
}

impl BlockList {
    /// An empty catalog.
    #[must_use]
    pub fn new() -> Self {
        Self { blocks: Vec::new() }
    }

    /// Appends one block, enforcing [`MAX_BLOCKS_PER_COMPONENT`].
    ///
    /// # Errors
    ///
    /// Returns [`BlockError::TooManyBlocks`] once length would exceed the
    /// bound.
    pub fn push(&mut self, block: CanonicalBlock) -> Result<(), BlockError> {
        if self.blocks.len() >= MAX_BLOCKS_PER_COMPONENT {
            return Err(BlockError::TooManyBlocks);
        }
        self.blocks.push(block);
        Ok(())
    }

    /// The number of blocks in this catalog.
    #[must_use]
    pub fn len(&self) -> usize {
        self.blocks.len()
    }

    /// Whether this catalog has zero blocks.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }

    /// Returns the block at `ordinal`, the zero-based `block_ordinal`
    /// addressing scheme used throughout the contract.
    #[must_use]
    pub fn get(&self, ordinal: usize) -> Option<&CanonicalBlock> {
        self.blocks.get(ordinal)
    }

    /// Iterates blocks in document order.
    pub fn iter(&self) -> core::slice::Iter<'_, CanonicalBlock> {
        self.blocks.iter()
    }
}

impl<'a> IntoIterator for &'a BlockList {
    type Item = &'a CanonicalBlock;
    type IntoIter = core::slice::Iter<'a, CanonicalBlock>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::{BlockList, CanonicalBlock, MAX_SCALARS_PER_BLOCK};
    use crate::error::{BlockError, RangeError};

    #[test]
    fn range_text_rejects_empty_range() {
        let block = CanonicalBlock::new("hello").unwrap();
        assert_eq!(block.range_text(2, 2), Err(RangeError::EmptyRange));
        assert_eq!(block.range_text(3, 1), Err(RangeError::EmptyRange));
    }

    #[test]
    fn range_text_rejects_out_of_bounds() {
        let block = CanonicalBlock::new("hello").unwrap();
        assert_eq!(block.range_text(0, 6), Err(RangeError::OutOfBounds));
    }

    #[test]
    fn range_text_extracts_exact_scalars() {
        let block = CanonicalBlock::new("hello").unwrap();
        assert_eq!(block.range_text(1, 3).unwrap(), "el");
    }

    #[test]
    fn range_text_uses_scalar_not_byte_indices() {
        let block = CanonicalBlock::new("a\u{1F600}b").unwrap();
        assert_eq!(block.scalar_len(), 3);
        assert_eq!(block.range_text(1, 2).unwrap(), "\u{1F600}");
    }

    #[test]
    fn prefix_and_suffix_context_never_cross_block_start_or_end() {
        let block = CanonicalBlock::new("abcdef").unwrap();
        assert_eq!(block.prefix_context(2, 64), "ab");
        assert_eq!(block.suffix_context(4, 64), "ef");
        assert_eq!(block.prefix_context(0, 64), "");
        assert_eq!(block.suffix_context(6, 64), "");
    }

    #[test]
    fn block_rejects_over_scalar_bound() {
        let text: String = "a".repeat(MAX_SCALARS_PER_BLOCK + 1);
        assert_eq!(CanonicalBlock::new(&text), Err(BlockError::TooManyScalars));
    }

    #[test]
    fn block_list_bounds_and_preserves_order() {
        let mut list = BlockList::new();
        list.push(CanonicalBlock::new("a").unwrap()).unwrap();
        list.push(CanonicalBlock::new("b").unwrap()).unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list.get(0).unwrap().as_string(), "a");
        assert_eq!(list.get(1).unwrap().as_string(), "b");
        let collected: Vec<String> = list.iter().map(CanonicalBlock::as_string).collect();
        assert_eq!(collected, vec!["a".to_string(), "b".to_string()]);
    }
}
