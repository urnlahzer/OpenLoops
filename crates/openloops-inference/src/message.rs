//! `CanonicalMessage` assembly: participant slots, subject/body/quote/
//! attachment-name/link-label blocks, and the bounded counts a model-
//! provider payload must respect.
//!
//! `contracts/model/provider-boundary.json` `request_contract`:
//! `maximum_blocks_per_message` (64), `maximum_scalars_per_block` (8192),
//! `maximum_participants_per_message` (500),
//! `maximum_attachment_names_per_message` (256), and
//! `maximum_link_labels_per_message` (256) are five independent maxima on
//! five independent catalogs (content blocks; the per-block scalar count;
//! participants; attachment names; link labels) — not one summed total —
//! so this module checks each independently rather than against one
//! combined counter.
//!
//! This is a second, *stricter* re-validation layer above
//! [`crate::blocks`]: `blocks::CanonicalBlock` enforces the larger
//! evidence-anchor bounds (`canonical_anchor_contract`:
//! `maximum_blocks_per_component` 4096, `maximum_scalars_per_block`
//! 1,048,576), because those blocks also back full-body anchor addressing.
//! A `CanonicalMessage` assembled here is the smaller, provider-payload-
//! shaped projection of that same canonical data.

use crate::blocks::CanonicalBlock;
use crate::canonical;
use crate::error::MessageError;
use crate::walker;

/// `maximum_blocks_per_message`: bounds `subject` (always exactly one) plus
/// `body_block` plus `quote_block` combined.
pub const MAX_BLOCKS_PER_MESSAGE: usize = 64;
/// `maximum_scalars_per_block`, re-checked here at the stricter
/// provider-payload bound.
pub const MAX_SCALARS_PER_BLOCK: usize = 8192;
/// `maximum_participants_per_message`: bounds `sender` (0 or 1) plus `to`
/// plus `cc` combined.
pub const MAX_PARTICIPANTS_PER_MESSAGE: usize = 500;
/// `maximum_attachment_names_per_message`.
pub const MAX_ATTACHMENT_NAMES_PER_MESSAGE: usize = 256;
/// `maximum_link_labels_per_message`.
pub const MAX_LINK_LABELS_PER_MESSAGE: usize = 256;

/// One raw (not yet canonicalized) `name`/`address` pair.
///
/// `canonical_projection_contract.graph_shape_rules.recipient`: "project
/// exactly `emailAddress.name` and `emailAddress.address`".
#[derive(Clone, Copy, Debug)]
pub struct RawRecipient<'a> {
    pub name: &'a str,
    pub address: &'a str,
}

/// One raw attachment's `id` (used only for ordering/dedup, then
/// discarded) and `name` (the only field ever surfaced as a block).
#[derive(Clone, Copy, Debug)]
pub struct RawAttachment<'a> {
    pub id: &'a str,
    pub name: &'a str,
}

/// One page of the bounded attachment-metadata enumeration.
///
/// `graph_request_contract.attachment_enumeration_rule`: `hasAttachments`
/// is deliberately not part of this shape — it is "a non-authoritative
/// optimization hint" the caller must never use to skip enumeration.
#[derive(Clone, Copy, Debug, Default)]
pub struct AttachmentPage<'a> {
    /// Whether this was the terminal page of the enumeration.
    pub terminal: bool,
    pub attachments: &'a [RawAttachment<'a>],
}

/// The complete raw input for one message canonicalization.
#[derive(Clone, Copy, Debug)]
pub struct RawMessageInput<'a> {
    pub subject: &'a str,
    /// `graph_shape_rules.body`: the `itemBody.content` value, requested
    /// with `contentType` already validated as `"html"` by the caller (this
    /// crate performs no Graph-shape/JSON validation; see
    /// `contracts/model/provider-boundary.json` for the upstream
    /// projection this input assumes).
    pub body_html: &'a str,
    pub sender: Option<RawRecipient<'a>>,
    pub from: Option<RawRecipient<'a>>,
    pub to: &'a [RawRecipient<'a>],
    pub cc: &'a [RawRecipient<'a>],
    pub attachment_pages: &'a [AttachmentPage<'a>],
}

/// The bounded, provider-payload-shaped canonical projection of one
/// message.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CanonicalMessage {
    pub subject: CanonicalBlock,
    pub body_blocks: Vec<CanonicalBlock>,
    pub quote_blocks: Vec<CanonicalBlock>,
    pub sender: Option<CanonicalBlock>,
    pub to: Vec<CanonicalBlock>,
    pub cc: Vec<CanonicalBlock>,
    pub attachment_names: Vec<CanonicalBlock>,
    pub link_labels: Vec<CanonicalBlock>,
}

fn bounded_block(text: &str) -> Result<CanonicalBlock, MessageError> {
    let block = CanonicalBlock::new(text)?;
    if block.scalar_len() > MAX_SCALARS_PER_BLOCK {
        return Err(MessageError::TooManyScalarsInBlock);
    }
    Ok(block)
}

fn canonicalize_recipient(recipient: &RawRecipient<'_>) -> Result<CanonicalBlock, MessageError> {
    let text = canonical::canonicalize_participant(recipient.name, recipient.address)?;
    bounded_block(&text)
}

/// `graph_shape_rules.sender_and_from`: "sender is used and from is
/// fallback only when sender is absent".
fn select_sender<'a>(
    sender: Option<&'a RawRecipient<'a>>,
    from: Option<&'a RawRecipient<'a>>,
) -> Option<&'a RawRecipient<'a>> {
    sender.or(from)
}

/// `graph_shape_rules.attachments` / `component_block_order.attachment_name`.
///
/// # Errors
///
/// [`MessageError::IncompleteAttachmentPaging`] unless the last supplied
/// page is terminal; [`MessageError::TooManyAttachmentNames`] over
/// [`MAX_ATTACHMENT_NAMES_PER_MESSAGE`]; [`MessageError::
/// DuplicateAttachmentId`] on a repeated case-sensitive id.
fn assemble_attachment_names(
    pages: &[AttachmentPage<'_>],
) -> Result<Vec<CanonicalBlock>, MessageError> {
    let Some(last) = pages.last() else {
        return Err(MessageError::IncompleteAttachmentPaging);
    };
    if !last.terminal {
        return Err(MessageError::IncompleteAttachmentPaging);
    }

    let mut all: Vec<(&str, &str)> = Vec::new();
    for page in pages {
        for attachment in page.attachments {
            all.push((attachment.id, attachment.name));
        }
    }
    if all.len() > MAX_ATTACHMENT_NAMES_PER_MESSAGE {
        return Err(MessageError::TooManyAttachmentNames);
    }

    let mut ids: Vec<&str> = all.iter().map(|(id, _)| *id).collect();
    ids.sort_unstable();
    let unique_count = {
        let mut deduped = ids.clone();
        deduped.dedup();
        deduped.len()
    };
    if unique_count != ids.len() {
        return Err(MessageError::DuplicateAttachmentId);
    }

    all.sort_by(|a, b| a.0.cmp(b.0));
    all.into_iter()
        .map(|(_, name)| {
            let text = canonical::canonicalize_plain(name)?;
            bounded_block(&text)
        })
        .collect()
}

/// Assembles one bounded [`CanonicalMessage`] from raw Graph-shaped input.
///
/// # Errors
///
/// See [`MessageError`] for every rejection this can return.
pub fn canonicalize_message(input: &RawMessageInput<'_>) -> Result<CanonicalMessage, MessageError> {
    let subject_text = canonical::canonicalize_plain(input.subject)?;
    let subject = bounded_block(&subject_text)?;

    let walked = walker::canonicalize_html(input.body_html)?;
    let body_blocks = walked
        .body_blocks
        .iter()
        .map(|text| bounded_block(text))
        .collect::<Result<Vec<_>, _>>()?;
    let quote_blocks = walked
        .quote_blocks
        .iter()
        .map(|text| bounded_block(text))
        .collect::<Result<Vec<_>, _>>()?;
    let link_labels = walked
        .link_labels
        .iter()
        .map(|text| bounded_block(text))
        .collect::<Result<Vec<_>, _>>()?;
    if link_labels.len() > MAX_LINK_LABELS_PER_MESSAGE {
        return Err(MessageError::TooManyLinkLabels);
    }

    let content_block_count = 1 + body_blocks.len() + quote_blocks.len();
    if content_block_count > MAX_BLOCKS_PER_MESSAGE {
        return Err(MessageError::TooManyBlocks);
    }

    let sender = select_sender(input.sender.as_ref(), input.from.as_ref())
        .map(canonicalize_recipient)
        .transpose()?;
    let to = input
        .to
        .iter()
        .map(canonicalize_recipient)
        .collect::<Result<Vec<_>, _>>()?;
    let cc = input
        .cc
        .iter()
        .map(canonicalize_recipient)
        .collect::<Result<Vec<_>, _>>()?;
    let participant_count = to.len() + cc.len() + usize::from(sender.is_some());
    if participant_count > MAX_PARTICIPANTS_PER_MESSAGE {
        return Err(MessageError::TooManyParticipants);
    }

    let attachment_names = assemble_attachment_names(input.attachment_pages)?;

    Ok(CanonicalMessage {
        subject,
        body_blocks,
        quote_blocks,
        sender,
        to,
        cc,
        attachment_names,
        link_labels,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        AttachmentPage, MAX_PARTICIPANTS_PER_MESSAGE, MAX_SCALARS_PER_BLOCK, RawAttachment,
        RawMessageInput, RawRecipient, canonicalize_message,
    };
    use crate::error::MessageError;
    use core::fmt::Write as _;

    fn one_terminal_page<'a>(attachments: &'a [RawAttachment<'a>]) -> Vec<AttachmentPage<'a>> {
        vec![AttachmentPage {
            terminal: true,
            attachments,
        }]
    }

    fn base_input<'a>(
        attachment_pages: &'a [AttachmentPage<'a>],
        to: &'a [RawRecipient<'a>],
        cc: &'a [RawRecipient<'a>],
    ) -> RawMessageInput<'a> {
        RawMessageInput {
            subject: "Hello",
            body_html: "<p>Body text</p>",
            sender: Some(RawRecipient {
                name: "Sender Name",
                address: "sender@example.invalid",
            }),
            from: None,
            to,
            cc,
            attachment_pages,
        }
    }

    #[test]
    fn assembles_a_complete_message() {
        let pages = one_terminal_page(&[]);
        let to = [RawRecipient {
            name: "To Person",
            address: "to@example.invalid",
        }];
        let cc: [RawRecipient<'_>; 0] = [];
        let input = base_input(&pages, &to, &cc);
        let message = canonicalize_message(&input).unwrap();
        assert_eq!(message.subject.as_string(), "Hello");
        assert_eq!(message.body_blocks.len(), 1);
        assert_eq!(message.body_blocks[0].as_string(), "Body text");
        assert_eq!(
            message.sender.unwrap().as_string(),
            "Sender Name <sender@example.invalid>"
        );
        assert_eq!(message.to.len(), 1);
        assert!(message.attachment_names.is_empty());
    }

    #[test]
    fn inline_only_attachment_vector_ignores_has_attachments_hint() {
        let attachments = [RawAttachment {
            id: "synthetic-inline-1",
            name: "synthetic-inline.png",
        }];
        let pages = one_terminal_page(&attachments);
        let to: [RawRecipient<'_>; 0] = [];
        let cc: [RawRecipient<'_>; 0] = [];
        let input = base_input(&pages, &to, &cc);
        let message = canonicalize_message(&input).unwrap();
        assert_eq!(message.attachment_names.len(), 1);
        assert_eq!(
            message.attachment_names[0].as_string(),
            "synthetic-inline.png"
        );
    }

    #[test]
    fn incomplete_attachment_paging_rejects_anchor_creation() {
        let attachments = [RawAttachment {
            id: "a",
            name: "a.txt",
        }];
        let pages = vec![AttachmentPage {
            terminal: false,
            attachments: &attachments,
        }];
        let to: [RawRecipient<'_>; 0] = [];
        let cc: [RawRecipient<'_>; 0] = [];
        let input = base_input(&pages, &to, &cc);
        assert_eq!(
            canonicalize_message(&input),
            Err(MessageError::IncompleteAttachmentPaging)
        );
    }

    #[test]
    fn duplicate_attachment_id_rejects() {
        let attachments = [
            RawAttachment {
                id: "dup",
                name: "one.txt",
            },
            RawAttachment {
                id: "dup",
                name: "two.txt",
            },
        ];
        let pages = one_terminal_page(&attachments);
        let to: [RawRecipient<'_>; 0] = [];
        let cc: [RawRecipient<'_>; 0] = [];
        let input = base_input(&pages, &to, &cc);
        assert_eq!(
            canonicalize_message(&input),
            Err(MessageError::DuplicateAttachmentId)
        );
    }

    #[test]
    fn attachments_sort_by_unsigned_utf8_id_bytes_and_discard_id() {
        let attachments = [
            RawAttachment {
                id: "b",
                name: "second.txt",
            },
            RawAttachment {
                id: "a",
                name: "first.txt",
            },
        ];
        let pages = one_terminal_page(&attachments);
        let to: [RawRecipient<'_>; 0] = [];
        let cc: [RawRecipient<'_>; 0] = [];
        let input = base_input(&pages, &to, &cc);
        let message = canonicalize_message(&input).unwrap();
        let names: Vec<String> = message
            .attachment_names
            .iter()
            .map(crate::blocks::CanonicalBlock::as_string)
            .collect();
        assert_eq!(
            names,
            vec!["first.txt".to_string(), "second.txt".to_string()]
        );
    }

    #[test]
    fn sender_absent_falls_back_to_from() {
        let pages = one_terminal_page(&[]);
        let to: [RawRecipient<'_>; 0] = [];
        let cc: [RawRecipient<'_>; 0] = [];
        let mut input = base_input(&pages, &to, &cc);
        input.sender = None;
        input.from = Some(RawRecipient {
            name: "From Person",
            address: "from@example.invalid",
        });
        let message = canonicalize_message(&input).unwrap();
        assert_eq!(
            message.sender.unwrap().as_string(),
            "From Person <from@example.invalid>"
        );
    }

    #[test]
    fn too_many_participants_rejects() {
        let pages = one_terminal_page(&[]);
        let recipients: Vec<RawRecipient<'_>> = (0..MAX_PARTICIPANTS_PER_MESSAGE)
            .map(|_| RawRecipient {
                name: "N",
                address: "n@example.invalid",
            })
            .collect();
        let cc: [RawRecipient<'_>; 0] = [];
        let input = base_input(&pages, &recipients, &cc);
        // `sender` (1) + `to` (500) already exceeds the 500 bound.
        assert_eq!(
            canonicalize_message(&input),
            Err(MessageError::TooManyParticipants)
        );
    }

    #[test]
    fn too_many_content_blocks_rejects() {
        let pages = one_terminal_page(&[]);
        let to: [RawRecipient<'_>; 0] = [];
        let cc: [RawRecipient<'_>; 0] = [];
        // Sibling blocks with no intervening blockquote merge into one
        // flush, so forcing enough *separate* body/quote flushes to exceed
        // `MAX_BLOCKS_PER_MESSAGE` requires interspersed blockquotes: each
        // `<div>X</div><blockquote>Q</blockquote>` pair yields one
        // body_block plus one quote_block. 32 pairs plus the mandatory
        // subject block is 65, one over the 64 bound.
        let mut body = String::new();
        for i in 0..32 {
            let _ = write!(body, "<div>{i}</div><blockquote>Q{i}</blockquote>");
        }
        let mut input = base_input(&pages, &to, &cc);
        input.body_html = &body;
        assert_eq!(
            canonicalize_message(&input),
            Err(MessageError::TooManyBlocks)
        );
    }

    #[test]
    fn block_over_provider_scalar_bound_rejects() {
        let pages = one_terminal_page(&[]);
        let to: [RawRecipient<'_>; 0] = [];
        let cc: [RawRecipient<'_>; 0] = [];
        let oversized: String = "a".repeat(MAX_SCALARS_PER_BLOCK + 1);
        let mut input = base_input(&pages, &to, &cc);
        input.subject = &oversized;
        assert_eq!(
            canonicalize_message(&input),
            Err(MessageError::TooManyScalarsInBlock)
        );
    }
}
