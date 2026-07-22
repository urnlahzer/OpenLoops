#![forbid(unsafe_code)]

typify::import_types!(schema = "../../contracts/ipc/skeleton-status.schema.json");

#[must_use]
/// Constructs the fixed Phase 0 status from the compile-time schema.
///
/// # Panics
///
/// Panics only if the schema-derived type rejects the fixed synthetic value,
/// which indicates a build-time contract-generator defect.
pub fn synthetic_status() -> SkeletonStatus {
    serde_json::from_value(serde_json::json!({
        "contract_version": 1,
        "companion_state": "skeleton_disabled",
        "enabled_capabilities": [],
        "gates_passed": []
    }))
    .expect("the compile-time schema accepts the fixed synthetic status")
}

#[must_use]
/// Returns whether the schema-derived status carries zero capability claims.
///
/// # Panics
///
/// Panics only if Typify produces a status type that Serde cannot serialize,
/// which indicates a build-time contract-generator defect.
pub fn has_no_claims(status: &SkeletonStatus) -> bool {
    let value = serde_json::to_value(status).expect("generated status is serializable");
    value["companion_state"] == "skeleton_disabled"
        && value["enabled_capabilities"]
            .as_array()
            .is_some_and(Vec::is_empty)
        && value["gates_passed"].as_array().is_some_and(Vec::is_empty)
}

// ---------------------------------------------------------------------------
// ADR-007 `analysis-output-v1`: a strict typed model of
// `contracts/model/analysis-output.schema.json`, plus the byte bound from
// `contracts/model/provider-boundary.json` `response_contract` that the JSON
// Schema itself does not express.
//
// ADR-007's `response_contract.required_validation_order` fixes ten steps.
// This module implements exactly steps 1 through 3: strict UTF-8 and exactly
// one JSON value; duplicate-member and unknown-field rejection; schema,
// numeric, and collection bounds. Steps 4 through 10 need application-
// supplied context this crate does not have (the opaque handles the
// application actually issued, the canonical source text, participant
// membership, timezone data) and remain the caller's responsibility.
// ---------------------------------------------------------------------------

/// `contracts/model/provider-boundary.json` `response_contract.maximum_response_bytes`.
pub const MAXIMUM_RESPONSE_BYTES: usize = 262_144;

/// `analysis-output.schema.json` `properties.claims.maxItems` (equal to
/// `provider-boundary.json` `response_contract.maximum_claims`).
pub const MAXIMUM_CLAIMS: usize = 64;

/// `analysis-output.schema.json` `$defs.claim.properties.evidence.minItems`.
pub const MINIMUM_EVIDENCE_PER_CLAIM: usize = 1;

/// `analysis-output.schema.json` `$defs.claim.properties.evidence.maxItems`.
pub const MAXIMUM_EVIDENCE_PER_CLAIM: usize = 8;

/// `$defs.claim.properties.related_loop_handles.maxItems`.
pub const MAXIMUM_RELATED_LOOP_HANDLES: usize = 8;

/// `$defs.claim.properties.ambiguity_codes.maxItems`.
pub const MAXIMUM_AMBIGUITY_CODES: usize = 8;

/// `$defs.opaque_handle.minLength`.
pub const MINIMUM_OPAQUE_HANDLE_CHARS: usize = 1;

/// `$defs.opaque_handle.maxLength`.
pub const MAXIMUM_OPAQUE_HANDLE_CHARS: usize = 128;

/// `$defs.temporal_hypothesis.properties.value.minLength`.
pub const MINIMUM_TEMPORAL_VALUE_CHARS: usize = 1;

/// `$defs.temporal_hypothesis.properties.value.maxLength`.
pub const MAXIMUM_TEMPORAL_VALUE_CHARS: usize = 256;

/// `$defs.claim.properties.confidence_micros.maximum`.
pub const MAXIMUM_CONFIDENCE_MICROS: u32 = 1_000_000;

/// `$defs.temporal_hypothesis.properties.text_evidence_index.maximum`.
pub const MAXIMUM_TEXT_EVIDENCE_INDEX: u8 = 7;

/// `properties.schema_version.const`.
pub const SCHEMA_VERSION: u32 = 1;

/// `$defs.claim.properties.claim_type.enum`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClaimType {
    Request,
    Promise,
    Attribution,
    Question,
    DeadlineChange,
    PossibleClosure,
    Delegation,
    Modification,
}

/// `$defs.evidence_range.properties.component.enum`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceComponent {
    Subject,
    BodyBlock,
    QuoteBlock,
    Sender,
    To,
    Cc,
    AttachmentName,
    LinkLabel,
}

/// `$defs.temporal_hypothesis.properties.kind.enum`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TemporalKind {
    Date,
    LocalDatetime,
    Relative,
    EventRelative,
    SoftWindow,
}

/// `$defs.claim.properties.ambiguity_codes.items.enum`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AmbiguityCode {
    QuoteScope,
    Identity,
    Delegation,
    Deadline,
    Relation,
    CrossMessage,
    InsufficientContext,
    SemanticConflict,
}

/// A required struct field whose JSON value is either shaped data or
/// explicit `null` (schema `oneOf: [<shape>, {"type": "null"}]`).
///
/// Deliberately not `Option<T>`, and deliberately not implemented by
/// delegating to `Option<T>`'s own `Deserialize` impl. When a struct field's
/// key is absent, serde's derive macro still calls that field type's
/// `Deserialize::deserialize` once more, against an internal placeholder
/// deserializer whose every method fails with "missing field" *except*
/// `deserialize_option`, which reports the field as present-with-`None`.
/// `Option<T>` (and anything that forwards to `deserialize_option`, under
/// any type name) therefore always accepts a missing key. This impl calls
/// only `deserialize_any` on the deserializer it is given, so an absent
/// `waiting_party_handle`/`temporal` key correctly fails with "missing
/// field" instead of silently becoming `Null`, matching the schema's
/// `required` set. An explicit JSON `null` is still accepted via
/// `visit_unit`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Nullable<T> {
    /// The JSON value was `null`.
    Null,
    /// The JSON value matched `T`.
    Value(T),
}

struct NullableVisitor<T>(std::marker::PhantomData<T>);

impl<'de, T> serde::de::Visitor<'de> for NullableVisitor<T>
where
    T: serde::Deserialize<'de>,
{
    type Value = Nullable<T>;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("null or a value")
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(Nullable::Null)
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        let deserializer = serde::de::value::StrDeserializer::new(value);
        T::deserialize(deserializer).map(Nullable::Value)
    }

    fn visit_map<A>(self, map: A) -> Result<Self::Value, A::Error>
    where
        A: serde::de::MapAccess<'de>,
    {
        let deserializer = serde::de::value::MapAccessDeserializer::new(map);
        T::deserialize(deserializer).map(Nullable::Value)
    }
}

impl<'de, T> serde::Deserialize<'de> for Nullable<T>
where
    T: serde::Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_any(NullableVisitor(std::marker::PhantomData))
    }
}

/// `$defs.evidence_range`.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceRange {
    pub source_handle: String,
    pub component: EvidenceComponent,
    pub block_ordinal: u16,
    pub range_start: u32,
    pub range_end: u32,
}

/// `$defs.temporal_hypothesis`.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TemporalHypothesis {
    pub text_evidence_index: u8,
    pub kind: TemporalKind,
    pub value: String,
}

/// `$defs.claim`.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Claim {
    pub claim_type: ClaimType,
    pub evidence: Vec<EvidenceRange>,
    pub waiting_party_handle: Nullable<String>,
    pub related_loop_handles: Vec<String>,
    pub temporal: Nullable<TemporalHypothesis>,
    pub confidence_micros: u32,
    pub ambiguity_codes: Vec<AmbiguityCode>,
}

/// The top-level `analysis-output-v1` document.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnalysisOutput {
    pub schema_version: u32,
    pub claims: Vec<Claim>,
}

/// A value-blind mirror of an arbitrary JSON document, used only to walk
/// every object at every nesting level and reject a repeated member name.
///
/// `serde_json::Value` (and any `#[derive(Deserialize)]` struct) silently
/// keeps the *last* value for a duplicate object key; ADR-007 validation
/// step 2 explicitly prohibits that, so this type turns it into a hard parse
/// error instead. It stores no data: only the pass/fail outcome of the walk
/// matters.
struct StrictJson;

impl<'de> serde::Deserialize<'de> for StrictJson {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_any(StrictJsonVisitor)
    }
}

struct StrictJsonVisitor;

impl<'de> serde::de::Visitor<'de> for StrictJsonVisitor {
    type Value = StrictJson;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("any JSON value")
    }

    fn visit_bool<E>(self, _value: bool) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(StrictJson)
    }

    fn visit_i64<E>(self, _value: i64) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(StrictJson)
    }

    fn visit_u64<E>(self, _value: u64) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(StrictJson)
    }

    fn visit_f64<E>(self, _value: f64) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(StrictJson)
    }

    fn visit_str<E>(self, _value: &str) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(StrictJson)
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(StrictJson)
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: serde::de::SeqAccess<'de>,
    {
        while sequence.next_element::<StrictJson>()?.is_some() {}
        Ok(StrictJson)
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: serde::de::MapAccess<'de>,
    {
        let mut seen_keys: std::collections::HashSet<String> = std::collections::HashSet::new();
        while let Some(key) = map.next_key::<String>()? {
            if !seen_keys.insert(key) {
                return Err(serde::de::Error::custom(
                    "duplicate object member rejected by openloops-contracts",
                ));
            }
            let _: StrictJson = map.next_value()?;
        }
        Ok(StrictJson)
    }
}

fn is_valid_opaque_handle(handle: &str) -> bool {
    let length = handle.chars().count();
    (MINIMUM_OPAQUE_HANDLE_CHARS..=MAXIMUM_OPAQUE_HANDLE_CHARS).contains(&length)
        && handle.chars().all(|character| {
            character.is_ascii_alphanumeric() || character == '_' || character == '-'
        })
}

fn is_bounded_text(value: &str, minimum_chars: usize, maximum_chars: usize) -> bool {
    let length = value.chars().count();
    (minimum_chars..=maximum_chars).contains(&length)
}

fn has_no_duplicate_strings(values: &[String]) -> bool {
    let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
    values.iter().all(|value| seen.insert(value.as_str()))
}

fn has_no_duplicate_ambiguity_codes(values: &[AmbiguityCode]) -> bool {
    for (index, value) in values.iter().enumerate() {
        if values[..index].contains(value) {
            return false;
        }
    }
    true
}

impl EvidenceRange {
    fn satisfies_contract_bounds(&self) -> bool {
        is_valid_opaque_handle(&self.source_handle) && self.range_end != 0
    }
}

impl TemporalHypothesis {
    fn satisfies_contract_bounds(&self) -> bool {
        self.text_evidence_index <= MAXIMUM_TEXT_EVIDENCE_INDEX
            && is_bounded_text(
                &self.value,
                MINIMUM_TEMPORAL_VALUE_CHARS,
                MAXIMUM_TEMPORAL_VALUE_CHARS,
            )
    }
}

impl Claim {
    fn satisfies_contract_bounds(&self) -> bool {
        if self.evidence.len() < MINIMUM_EVIDENCE_PER_CLAIM
            || self.evidence.len() > MAXIMUM_EVIDENCE_PER_CLAIM
            || !self
                .evidence
                .iter()
                .all(EvidenceRange::satisfies_contract_bounds)
        {
            return false;
        }
        if self.related_loop_handles.len() > MAXIMUM_RELATED_LOOP_HANDLES
            || !self
                .related_loop_handles
                .iter()
                .all(|handle| is_valid_opaque_handle(handle))
            || !has_no_duplicate_strings(&self.related_loop_handles)
        {
            return false;
        }
        if let Nullable::Value(handle) = &self.waiting_party_handle
            && !is_valid_opaque_handle(handle)
        {
            return false;
        }
        if let Nullable::Value(temporal) = &self.temporal
            && !temporal.satisfies_contract_bounds()
        {
            return false;
        }
        if self.confidence_micros > MAXIMUM_CONFIDENCE_MICROS {
            return false;
        }
        self.ambiguity_codes.len() <= MAXIMUM_AMBIGUITY_CODES
            && has_no_duplicate_ambiguity_codes(&self.ambiguity_codes)
    }
}

impl AnalysisOutput {
    fn satisfies_contract_bounds(&self) -> bool {
        self.schema_version == SCHEMA_VERSION
            && self.claims.len() <= MAXIMUM_CLAIMS
            && self.claims.iter().all(Claim::satisfies_contract_bounds)
    }
}

/// Rejection reasons for [`parse_analysis_output`].
///
/// Exactly the four ADR-007 diagnostic codes
/// (`contracts/model/provider-boundary.json` `privacy_boundary.diagnostics`)
/// this crate's parsing responsibility can produce: `response_too_large`,
/// `invalid_utf8`, `invalid_json`, `invalid_schema`. No variant carries
/// request/response content, an error message, or a position:
/// `privacy_boundary.diagnostic_values` requires "fixed codes and bounded
/// non-content counters only".
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ParseRejection {
    /// The input exceeded [`MAXIMUM_RESPONSE_BYTES`] before any parsing began.
    ResponseTooLarge,
    /// The input bytes are not strict UTF-8.
    InvalidUtf8,
    /// The input is not exactly one syntactically valid JSON value: bad
    /// tokens, an invalid escape (including an unpaired `\u` surrogate
    /// half), a byte-order mark, or trailing content after the value.
    InvalidJson,
    /// The input is syntactically valid JSON but violates the closed
    /// `analysis-output-v1` contract: a duplicate member at any nesting
    /// level, an unknown field, a wrong type, a non-catalog enum value, an
    /// out-of-range number, an oversized or non-unique collection, or a
    /// malformed opaque handle or bounded string.
    InvalidSchema,
}

/// Parses one provider response into a validated [`AnalysisOutput`].
///
/// Applies `contracts/model/provider-boundary.json`
/// `response_contract.required_validation_order` steps 1 through 3: strict
/// UTF-8 and exactly one JSON value; duplicate-member and unknown-field
/// rejection; schema, numeric, and collection bounds. Steps 4 through 10
/// (opaque-handle membership against the application's own issued set,
/// canonical-text and evidence correspondence, participant membership, date
/// reparsing, cross-claim consistency, and semantic checks) need
/// application-supplied context this crate does not have and remain the
/// caller's responsibility before any result is trusted for automation.
///
/// # Errors
///
/// Returns [`ParseRejection`] for any input this crate cannot accept; see
/// each variant's documentation for the exact condition.
pub fn parse_analysis_output(bytes: &[u8]) -> Result<AnalysisOutput, ParseRejection> {
    if bytes.len() > MAXIMUM_RESPONSE_BYTES {
        return Err(ParseRejection::ResponseTooLarge);
    }
    let text = std::str::from_utf8(bytes).map_err(|_| ParseRejection::InvalidUtf8)?;

    // Pass A: exactly-one-syntactically-valid-JSON-value validation. This
    // also rejects trailing content (`serde_json::from_str` always calls the
    // deserializer's `end()`), invalid escapes, unpaired `\u` surrogate
    // halves, and a leading byte-order mark (not valid JSON whitespace).
    serde_json::from_str::<serde_json::Value>(text).map_err(|_| ParseRejection::InvalidJson)?;

    // Pass B: duplicate-member rejection at every nesting level. Pass A
    // already proved the text is syntactically valid JSON under the same
    // parser and the same default recursion limit, so the only way this
    // pass can fail is the explicit duplicate-member check in
    // `StrictJsonVisitor::visit_map`.
    serde_json::from_str::<StrictJson>(text).map_err(|_| ParseRejection::InvalidSchema)?;

    // Pass C: strict shape (`deny_unknown_fields`, closed enums, required
    // presence of nullable fields via `Nullable<T>`, integer widths that
    // already enforce most numeric bounds).
    let output =
        serde_json::from_str::<AnalysisOutput>(text).map_err(|_| ParseRejection::InvalidSchema)?;

    // Pass D: the remaining numeric, collection, and opaque-handle bounds
    // serde's derive cannot express.
    if output.satisfies_contract_bounds() {
        Ok(output)
    } else {
        Err(ParseRejection::InvalidSchema)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn generated_contract_cannot_claim_capabilities_or_gates() {
        let status = super::synthetic_status();
        assert!(super::has_no_claims(&status));
    }
}

#[cfg(test)]
mod analysis_output_tests {
    fn valid_fixture_json() -> String {
        "{\"schema_version\":1,\"claims\":[{\"claim_type\":\"request\",\"evidence\":[{\"source_handle\":\"src-1\",\"component\":\"subject\",\"block_ordinal\":0,\"range_start\":0,\"range_end\":5}],\"waiting_party_handle\":null,\"related_loop_handles\":[\"loop-1\"],\"temporal\":{\"text_evidence_index\":0,\"kind\":\"date\",\"value\":\"2026-08-01\"},\"confidence_micros\":900000,\"ambiguity_codes\":[\"deadline\"]}]}".to_string()
    }

    fn splitmix64(state: &mut u64) -> u64 {
        *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut value = *state;
        value = (value ^ (value >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        value = (value ^ (value >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        value ^ (value >> 31)
    }

    #[test]
    fn valid_fixture_round_trips() {
        let json = valid_fixture_json();
        let output = super::parse_analysis_output(json.as_bytes())
            .expect("the fixture matches every schema and bound");
        assert_eq!(output.schema_version, super::SCHEMA_VERSION);
        assert_eq!(output.claims.len(), 1);
        let claim = &output.claims[0];
        assert_eq!(claim.claim_type, super::ClaimType::Request);
        assert_eq!(claim.evidence.len(), 1);
        assert_eq!(
            claim.evidence[0].component,
            super::EvidenceComponent::Subject
        );
        assert_eq!(claim.waiting_party_handle, super::Nullable::Null);
        assert_eq!(claim.related_loop_handles, vec!["loop-1".to_string()]);
        match &claim.temporal {
            super::Nullable::Value(temporal) => {
                assert_eq!(temporal.kind, super::TemporalKind::Date);
            }
            super::Nullable::Null => panic!("fixture temporal is not null"),
        }
        assert_eq!(claim.confidence_micros, 900_000);
        assert_eq!(claim.ambiguity_codes, vec![super::AmbiguityCode::Deadline]);
    }

    #[test]
    fn oversize_input_is_rejected_before_any_parsing() {
        let bytes = vec![b' '; super::MAXIMUM_RESPONSE_BYTES + 1];
        assert_eq!(
            super::parse_analysis_output(&bytes),
            Err(super::ParseRejection::ResponseTooLarge)
        );
    }

    #[test]
    fn exact_byte_boundary_is_inclusive_and_falls_through_to_json_validation() {
        let bytes = vec![b' '; super::MAXIMUM_RESPONSE_BYTES];
        // Exactly at the limit: the size gate passes, so whitespace-only
        // content is rejected by the next stage, never by the size gate.
        assert_eq!(
            super::parse_analysis_output(&bytes),
            Err(super::ParseRejection::InvalidJson)
        );
    }

    #[test]
    fn invalid_utf8_bytes_are_rejected() {
        let bytes = [0xFF, 0xFE, 0xFD];
        assert_eq!(
            super::parse_analysis_output(&bytes),
            Err(super::ParseRejection::InvalidUtf8)
        );
    }

    #[test]
    fn overlong_utf8_encoding_is_rejected() {
        // An overlong two-byte encoding of U+0000; never valid UTF-8.
        let bytes = [0xC0, 0x80];
        assert_eq!(
            super::parse_analysis_output(&bytes),
            Err(super::ParseRejection::InvalidUtf8)
        );
    }

    #[test]
    fn trailing_content_after_one_json_value_is_rejected() {
        let text = format!("{}{{}}", valid_fixture_json());
        assert_eq!(
            super::parse_analysis_output(text.as_bytes()),
            Err(super::ParseRejection::InvalidJson)
        );
    }

    #[test]
    fn leading_byte_order_mark_is_rejected() {
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice(valid_fixture_json().as_bytes());
        assert_eq!(
            super::parse_analysis_output(&bytes),
            Err(super::ParseRejection::InvalidJson)
        );
    }

    #[test]
    fn unpaired_high_surrogate_escape_is_rejected() {
        let text = "{\"schema_version\":1,\"claims\":[{\"claim_type\":\"request\",\"evidence\":[{\"source_handle\":\"src-\\uD800\",\"component\":\"subject\",\"block_ordinal\":0,\"range_start\":0,\"range_end\":1}],\"waiting_party_handle\":null,\"related_loop_handles\":[],\"temporal\":null,\"confidence_micros\":0,\"ambiguity_codes\":[]}]}";
        assert_eq!(
            super::parse_analysis_output(text.as_bytes()),
            Err(super::ParseRejection::InvalidJson)
        );
    }

    #[test]
    fn deeply_nested_array_does_not_panic_and_is_rejected() {
        let mut text = String::new();
        for _ in 0..300 {
            text.push('[');
        }
        for _ in 0..300 {
            text.push(']');
        }
        assert_eq!(
            super::parse_analysis_output(text.as_bytes()),
            Err(super::ParseRejection::InvalidJson)
        );
    }

    #[test]
    fn duplicate_member_at_top_level_is_rejected() {
        let text = "{\"schema_version\":1,\"schema_version\":1,\"claims\":[]}";
        assert_eq!(
            super::parse_analysis_output(text.as_bytes()),
            Err(super::ParseRejection::InvalidSchema)
        );
    }

    #[test]
    fn duplicate_member_nested_inside_evidence_is_rejected() {
        let text = "{\"schema_version\":1,\"claims\":[{\"claim_type\":\"request\",\"evidence\":[{\"source_handle\":\"a\",\"source_handle\":\"b\",\"component\":\"subject\",\"block_ordinal\":0,\"range_start\":0,\"range_end\":1}],\"waiting_party_handle\":null,\"related_loop_handles\":[],\"temporal\":null,\"confidence_micros\":0,\"ambiguity_codes\":[]}]}";
        assert_eq!(
            super::parse_analysis_output(text.as_bytes()),
            Err(super::ParseRejection::InvalidSchema)
        );
    }

    #[test]
    fn duplicate_member_nested_inside_temporal_is_rejected() {
        let text = "{\"schema_version\":1,\"claims\":[{\"claim_type\":\"request\",\"evidence\":[{\"source_handle\":\"a\",\"component\":\"subject\",\"block_ordinal\":0,\"range_start\":0,\"range_end\":1}],\"waiting_party_handle\":null,\"related_loop_handles\":[],\"temporal\":{\"text_evidence_index\":0,\"text_evidence_index\":0,\"kind\":\"date\",\"value\":\"x\"},\"confidence_micros\":0,\"ambiguity_codes\":[]}]}";
        assert_eq!(
            super::parse_analysis_output(text.as_bytes()),
            Err(super::ParseRejection::InvalidSchema)
        );
    }

    #[test]
    fn unknown_field_at_top_level_is_rejected() {
        let text = "{\"schema_version\":1,\"claims\":[],\"extra\":true}";
        assert_eq!(
            super::parse_analysis_output(text.as_bytes()),
            Err(super::ParseRejection::InvalidSchema)
        );
    }

    #[test]
    fn unknown_field_on_claim_is_rejected() {
        let text = "{\"schema_version\":1,\"claims\":[{\"claim_type\":\"request\",\"evidence\":[{\"source_handle\":\"a\",\"component\":\"subject\",\"block_ordinal\":0,\"range_start\":0,\"range_end\":1}],\"waiting_party_handle\":null,\"related_loop_handles\":[],\"temporal\":null,\"confidence_micros\":0,\"ambiguity_codes\":[],\"extra\":true}]}";
        assert_eq!(
            super::parse_analysis_output(text.as_bytes()),
            Err(super::ParseRejection::InvalidSchema)
        );
    }

    #[test]
    fn unknown_field_on_evidence_is_rejected() {
        let text = "{\"schema_version\":1,\"claims\":[{\"claim_type\":\"request\",\"evidence\":[{\"source_handle\":\"a\",\"component\":\"subject\",\"block_ordinal\":0,\"range_start\":0,\"range_end\":1,\"extra\":true}],\"waiting_party_handle\":null,\"related_loop_handles\":[],\"temporal\":null,\"confidence_micros\":0,\"ambiguity_codes\":[]}]}";
        assert_eq!(
            super::parse_analysis_output(text.as_bytes()),
            Err(super::ParseRejection::InvalidSchema)
        );
    }

    #[test]
    fn unknown_field_on_temporal_is_rejected() {
        let text = "{\"schema_version\":1,\"claims\":[{\"claim_type\":\"request\",\"evidence\":[{\"source_handle\":\"a\",\"component\":\"subject\",\"block_ordinal\":0,\"range_start\":0,\"range_end\":1}],\"waiting_party_handle\":null,\"related_loop_handles\":[],\"temporal\":{\"text_evidence_index\":0,\"kind\":\"date\",\"value\":\"x\",\"extra\":true},\"confidence_micros\":0,\"ambiguity_codes\":[]}]}";
        assert_eq!(
            super::parse_analysis_output(text.as_bytes()),
            Err(super::ParseRejection::InvalidSchema)
        );
    }

    #[test]
    fn enum_value_differing_only_by_case_is_rejected() {
        let text = "{\"schema_version\":1,\"claims\":[{\"claim_type\":\"Request\",\"evidence\":[{\"source_handle\":\"a\",\"component\":\"subject\",\"block_ordinal\":0,\"range_start\":0,\"range_end\":1}],\"waiting_party_handle\":null,\"related_loop_handles\":[],\"temporal\":null,\"confidence_micros\":0,\"ambiguity_codes\":[]}]}";
        assert_eq!(
            super::parse_analysis_output(text.as_bytes()),
            Err(super::ParseRejection::InvalidSchema)
        );
    }

    #[test]
    fn enum_value_outside_the_catalog_is_rejected() {
        let text = "{\"schema_version\":1,\"claims\":[{\"claim_type\":\"unknown_type\",\"evidence\":[{\"source_handle\":\"a\",\"component\":\"subject\",\"block_ordinal\":0,\"range_start\":0,\"range_end\":1}],\"waiting_party_handle\":null,\"related_loop_handles\":[],\"temporal\":null,\"confidence_micros\":0,\"ambiguity_codes\":[]}]}";
        assert_eq!(
            super::parse_analysis_output(text.as_bytes()),
            Err(super::ParseRejection::InvalidSchema)
        );
    }

    #[test]
    fn claims_over_max_items_is_rejected() {
        let claim = "{\"claim_type\":\"request\",\"evidence\":[{\"source_handle\":\"a\",\"component\":\"subject\",\"block_ordinal\":0,\"range_start\":0,\"range_end\":1}],\"waiting_party_handle\":null,\"related_loop_handles\":[],\"temporal\":null,\"confidence_micros\":0,\"ambiguity_codes\":[]}";
        let claims = [claim; super::MAXIMUM_CLAIMS + 1].join(",");
        let text = format!("{{\"schema_version\":1,\"claims\":[{claims}]}}");
        assert_eq!(
            super::parse_analysis_output(text.as_bytes()),
            Err(super::ParseRejection::InvalidSchema)
        );
    }

    #[test]
    fn evidence_over_max_items_is_rejected() {
        let range = "{\"source_handle\":\"a\",\"component\":\"subject\",\"block_ordinal\":0,\"range_start\":0,\"range_end\":1}";
        let ranges = [range; super::MAXIMUM_EVIDENCE_PER_CLAIM + 1].join(",");
        let text = format!(
            "{{\"schema_version\":1,\"claims\":[{{\"claim_type\":\"request\",\"evidence\":[{ranges}],\"waiting_party_handle\":null,\"related_loop_handles\":[],\"temporal\":null,\"confidence_micros\":0,\"ambiguity_codes\":[]}}]}}"
        );
        assert_eq!(
            super::parse_analysis_output(text.as_bytes()),
            Err(super::ParseRejection::InvalidSchema)
        );
    }

    #[test]
    fn related_loop_handles_duplicate_violates_unique_items() {
        let text = "{\"schema_version\":1,\"claims\":[{\"claim_type\":\"request\",\"evidence\":[{\"source_handle\":\"a\",\"component\":\"subject\",\"block_ordinal\":0,\"range_start\":0,\"range_end\":1}],\"waiting_party_handle\":null,\"related_loop_handles\":[\"loop-1\",\"loop-1\"],\"temporal\":null,\"confidence_micros\":0,\"ambiguity_codes\":[]}]}";
        assert_eq!(
            super::parse_analysis_output(text.as_bytes()),
            Err(super::ParseRejection::InvalidSchema)
        );
    }

    #[test]
    fn ambiguity_codes_duplicate_violates_unique_items() {
        let text = "{\"schema_version\":1,\"claims\":[{\"claim_type\":\"request\",\"evidence\":[{\"source_handle\":\"a\",\"component\":\"subject\",\"block_ordinal\":0,\"range_start\":0,\"range_end\":1}],\"waiting_party_handle\":null,\"related_loop_handles\":[],\"temporal\":null,\"confidence_micros\":0,\"ambiguity_codes\":[\"deadline\",\"deadline\"]}]}";
        assert_eq!(
            super::parse_analysis_output(text.as_bytes()),
            Err(super::ParseRejection::InvalidSchema)
        );
    }

    #[test]
    fn opaque_handle_over_max_length_is_rejected() {
        let handle = "h".repeat(super::MAXIMUM_OPAQUE_HANDLE_CHARS + 1);
        let text = format!(
            "{{\"schema_version\":1,\"claims\":[{{\"claim_type\":\"request\",\"evidence\":[{{\"source_handle\":\"{handle}\",\"component\":\"subject\",\"block_ordinal\":0,\"range_start\":0,\"range_end\":1}}],\"waiting_party_handle\":null,\"related_loop_handles\":[],\"temporal\":null,\"confidence_micros\":0,\"ambiguity_codes\":[]}}]}}"
        );
        assert_eq!(
            super::parse_analysis_output(text.as_bytes()),
            Err(super::ParseRejection::InvalidSchema)
        );
    }

    #[test]
    fn opaque_handle_with_disallowed_character_is_rejected() {
        let text = "{\"schema_version\":1,\"claims\":[{\"claim_type\":\"request\",\"evidence\":[{\"source_handle\":\"has space\",\"component\":\"subject\",\"block_ordinal\":0,\"range_start\":0,\"range_end\":1}],\"waiting_party_handle\":null,\"related_loop_handles\":[],\"temporal\":null,\"confidence_micros\":0,\"ambiguity_codes\":[]}]}";
        assert_eq!(
            super::parse_analysis_output(text.as_bytes()),
            Err(super::ParseRejection::InvalidSchema)
        );
    }

    #[test]
    fn temporal_value_over_max_length_is_rejected() {
        let value = "x".repeat(super::MAXIMUM_TEMPORAL_VALUE_CHARS + 1);
        let text = format!(
            "{{\"schema_version\":1,\"claims\":[{{\"claim_type\":\"request\",\"evidence\":[{{\"source_handle\":\"a\",\"component\":\"subject\",\"block_ordinal\":0,\"range_start\":0,\"range_end\":1}}],\"waiting_party_handle\":null,\"related_loop_handles\":[],\"temporal\":{{\"text_evidence_index\":0,\"kind\":\"date\",\"value\":\"{value}\"}},\"confidence_micros\":0,\"ambiguity_codes\":[]}}]}}"
        );
        assert_eq!(
            super::parse_analysis_output(text.as_bytes()),
            Err(super::ParseRejection::InvalidSchema)
        );
    }

    #[test]
    fn confidence_micros_over_maximum_is_rejected() {
        let text = "{\"schema_version\":1,\"claims\":[{\"claim_type\":\"request\",\"evidence\":[{\"source_handle\":\"a\",\"component\":\"subject\",\"block_ordinal\":0,\"range_start\":0,\"range_end\":1}],\"waiting_party_handle\":null,\"related_loop_handles\":[],\"temporal\":null,\"confidence_micros\":1000001,\"ambiguity_codes\":[]}]}";
        assert_eq!(
            super::parse_analysis_output(text.as_bytes()),
            Err(super::ParseRejection::InvalidSchema)
        );
    }

    #[test]
    fn confidence_micros_far_beyond_u32_is_rejected() {
        let text = "{\"schema_version\":1,\"claims\":[{\"claim_type\":\"request\",\"evidence\":[{\"source_handle\":\"a\",\"component\":\"subject\",\"block_ordinal\":0,\"range_start\":0,\"range_end\":1}],\"waiting_party_handle\":null,\"related_loop_handles\":[],\"temporal\":null,\"confidence_micros\":99999999999999999999999999999,\"ambiguity_codes\":[]}]}";
        assert_eq!(
            super::parse_analysis_output(text.as_bytes()),
            Err(super::ParseRejection::InvalidSchema)
        );
    }

    #[test]
    fn text_evidence_index_over_maximum_is_rejected() {
        let text = "{\"schema_version\":1,\"claims\":[{\"claim_type\":\"request\",\"evidence\":[{\"source_handle\":\"a\",\"component\":\"subject\",\"block_ordinal\":0,\"range_start\":0,\"range_end\":1}],\"waiting_party_handle\":null,\"related_loop_handles\":[],\"temporal\":{\"text_evidence_index\":8,\"kind\":\"date\",\"value\":\"x\"},\"confidence_micros\":0,\"ambiguity_codes\":[]}]}";
        assert_eq!(
            super::parse_analysis_output(text.as_bytes()),
            Err(super::ParseRejection::InvalidSchema)
        );
    }

    #[test]
    fn range_end_of_zero_is_rejected() {
        let text = "{\"schema_version\":1,\"claims\":[{\"claim_type\":\"request\",\"evidence\":[{\"source_handle\":\"a\",\"component\":\"subject\",\"block_ordinal\":0,\"range_start\":0,\"range_end\":0}],\"waiting_party_handle\":null,\"related_loop_handles\":[],\"temporal\":null,\"confidence_micros\":0,\"ambiguity_codes\":[]}]}";
        assert_eq!(
            super::parse_analysis_output(text.as_bytes()),
            Err(super::ParseRejection::InvalidSchema)
        );
    }

    #[test]
    fn wrong_schema_version_is_rejected() {
        let text = "{\"schema_version\":2,\"claims\":[]}";
        assert_eq!(
            super::parse_analysis_output(text.as_bytes()),
            Err(super::ParseRejection::InvalidSchema)
        );
    }

    #[test]
    fn missing_required_nullable_field_is_rejected() {
        // `waiting_party_handle` is omitted entirely, not merely null.
        let text = "{\"schema_version\":1,\"claims\":[{\"claim_type\":\"request\",\"evidence\":[{\"source_handle\":\"a\",\"component\":\"subject\",\"block_ordinal\":0,\"range_start\":0,\"range_end\":1}],\"related_loop_handles\":[],\"temporal\":null,\"confidence_micros\":0,\"ambiguity_codes\":[]}]}";
        assert_eq!(
            super::parse_analysis_output(text.as_bytes()),
            Err(super::ParseRejection::InvalidSchema)
        );
    }

    #[test]
    fn fuzz_loop_over_bounded_pseudorandom_bytes_never_panics() {
        let mut state: u64 = 0x0123_4567_89AB_CDEF;
        for _ in 0..2_000 {
            let length = usize::from(splitmix64(&mut state).to_le_bytes()[0]) * 4;
            let mut buffer = Vec::with_capacity(length);
            for _ in 0..length {
                buffer.push(splitmix64(&mut state).to_le_bytes()[0]);
            }
            let _ = super::parse_analysis_output(&buffer);
        }
    }

    #[test]
    fn fuzz_loop_over_mutated_valid_fixture_never_panics() {
        let fixture = valid_fixture_json().into_bytes();
        let mut state: u64 = 0xFEDC_BA98_7654_3210;
        for _ in 0..2_000 {
            let mut mutated = fixture.clone();
            let mutation_count = 1 + (splitmix64(&mut state) % 6);
            for _ in 0..mutation_count {
                if mutated.is_empty() {
                    break;
                }
                let index =
                    usize::try_from(splitmix64(&mut state)).unwrap_or(usize::MAX) % mutated.len();
                let random_byte = splitmix64(&mut state).to_le_bytes()[0];
                mutated[index] = random_byte;
            }
            let _ = super::parse_analysis_output(&mutated);
        }
    }
}
