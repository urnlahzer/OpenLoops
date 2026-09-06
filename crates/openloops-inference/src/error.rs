//! Typed, content-free errors for the ADR-006 canonicalizer.
//!
//! Every variant here identifies *which rule* rejected an input and never
//! carries source text, a participant value, a subject, a URL, a filename,
//! or any other message content: `privacy_boundary.
//! human_readable_mailbox_or_generated_text` is "prohibited", and
//! `unavailable_projection.prohibited_projection` lists the same content
//! classes, so a caller can log/route on the error variant alone.

/// A rejected scalar-level decode or validation step.
///
/// `contracts/evidence/identity-boundary.json`
/// `canonical_anchor_contract.normalization_steps_in_order[0]` and
/// `canonical_projection_contract.scalar_policy`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScalarError {
    /// A UTF-16 code unit did not pair with a matching surrogate
    /// (`scalar_policy.json_decode`: "reject any unpaired UTF-16 surrogate
    /// before scalar conversion").
    UnpairedSurrogate,
    /// A decoded scalar is on `scalar_policy.rejected_scalars`: a forbidden
    /// control character, the `U+FDD0..=U+FDEF` noncharacter block, or any
    /// scalar whose low 16 bits are `FFFE`/`FFFF`.
    ForbiddenScalar,
}

/// A rejected block/segmentation-level bound.
///
/// `canonical_anchor_contract.maximum_blocks_per_component` /
/// `.maximum_scalars_per_block`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BlockError {
    /// More than `maximum_blocks_per_component` blocks were produced for one
    /// component's ordered block list.
    TooManyBlocks,
    /// One block exceeded `maximum_scalars_per_block` Unicode scalars.
    TooManyScalars,
}

/// A rejected `(block_ordinal, range_start, range_end)` addressing request.
///
/// `canonical_anchor_contract.range_unit` / `.range_end` / `.empty_ranges`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RangeError {
    /// `range_start >= range_end`; `empty_ranges` is "prohibited".
    EmptyRange,
    /// `range_end` exceeds the addressed block's Unicode scalar count.
    OutOfBounds,
}

/// A rejected `CanonicalMessage` assembly.
///
/// `contracts/model/provider-boundary.json` `request_contract` bounds.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MessageError {
    /// Scalar-level rejection while canonicalizing one component.
    Scalar(ScalarError),
    /// Block-level rejection while canonicalizing one component.
    Block(BlockError),
    /// A single block exceeded `maximum_scalars_per_block` (8192) under the
    /// *provider* bound, which is stricter than the evidence-anchor bound
    /// enforced inside [`crate::blocks::CanonicalBlock`].
    TooManyScalarsInBlock,
    /// More than `maximum_blocks_per_message` subject/body/quote blocks.
    TooManyBlocks,
    /// More than `maximum_participants_per_message` sender/to/cc entries.
    TooManyParticipants,
    /// More than `maximum_attachment_names_per_message` attachments.
    TooManyAttachmentNames,
    /// More than `maximum_link_labels_per_message` link labels.
    TooManyLinkLabels,
    /// Two attachments in the same enumeration shared a case-sensitive id.
    DuplicateAttachmentId,
    /// Attachment-metadata paging did not reach a terminal page;
    /// `CANON-INCOMPLETE-ATTACHMENTS-001` requires `reject_anchor_creation`.
    IncompleteAttachmentPaging,
}

impl From<ScalarError> for MessageError {
    fn from(error: ScalarError) -> Self {
        Self::Scalar(error)
    }
}

impl From<BlockError> for MessageError {
    fn from(error: BlockError) -> Self {
        Self::Block(error)
    }
}
