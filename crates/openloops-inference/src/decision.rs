//! Typed decision questions, strict answers, and the `OpenRouter` Jev adapter.

use std::collections::{BTreeMap, HashSet};
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use serde_json::Value;

use crate::analysis::valid_handle;
use crate::provider::{MAX_REQUEST, ProviderError};

/// Whole-request deadline for decision calls.
pub const DECISION_DEADLINE: Duration = Duration::from_secs(20);
/// Maximum number of choices accepted by the Jev API.
pub const MAX_CHOICE_OPTIONS: usize = 255;

#[derive(Clone, Debug, PartialEq)]
pub enum Question {
    Noul {
        instructions: String,
    },
    Choice {
        instructions: String,
        options: Vec<(String, String)>,
    },
    Score {
        instructions: String,
        levels: Vec<String>,
    },
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Questions {
    items: Vec<(String, Question)>,
}

impl Questions {
    #[must_use]
    pub const fn new() -> Self {
        Self { items: Vec::new() }
    }

    /// Adds a probability question.
    /// # Errors
    /// Returns [`ProviderError::InvalidQuestion`] for malformed or duplicate input.
    pub fn noul(mut self, id: &str, instructions: &str) -> Result<Self, ProviderError> {
        self.validate_common(id, instructions)?;
        self.items.push((
            id.to_owned(),
            Question::Noul {
                instructions: instructions.to_owned(),
            },
        ));
        Ok(self)
    }

    /// Adds an ordered choice question.
    /// # Errors
    /// Returns [`ProviderError::InvalidQuestion`] for malformed or duplicate input.
    pub fn choice(
        mut self,
        id: &str,
        instructions: &str,
        options: &[(&str, &str)],
    ) -> Result<Self, ProviderError> {
        self.validate_common(id, instructions)?;
        if options.is_empty() || options.len() > MAX_CHOICE_OPTIONS {
            return Err(ProviderError::InvalidQuestion);
        }
        let mut keys = HashSet::new();
        if options
            .iter()
            .any(|(key, _)| !valid_handle(key) || !keys.insert(*key))
        {
            return Err(ProviderError::InvalidQuestion);
        }
        self.items.push((
            id.to_owned(),
            Question::Choice {
                instructions: instructions.to_owned(),
                options: options
                    .iter()
                    .map(|(key, description)| ((*key).to_owned(), (*description).to_owned()))
                    .collect(),
            },
        ));
        Ok(self)
    }

    /// Adds an ordered score question.
    /// # Errors
    /// Returns [`ProviderError::InvalidQuestion`] for malformed or duplicate input.
    pub fn score(
        mut self,
        id: &str,
        instructions: &str,
        levels: &[&str],
    ) -> Result<Self, ProviderError> {
        self.validate_common(id, instructions)?;
        if levels.len() < 2 {
            return Err(ProviderError::InvalidQuestion);
        }
        self.items.push((
            id.to_owned(),
            Question::Score {
                instructions: instructions.to_owned(),
                levels: levels.iter().map(|level| (*level).to_owned()).collect(),
            },
        ));
        Ok(self)
    }

    fn validate_common(&self, id: &str, instructions: &str) -> Result<(), ProviderError> {
        if !valid_question_id(id)
            || instructions.is_empty()
            || self.items.iter().any(|(existing, _)| existing == id)
        {
            return Err(ProviderError::InvalidQuestion);
        }
        Ok(())
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Builds the named questions from the pinned registry.
    /// # Errors
    /// Returns [`ProviderError::InvalidQuestion`] for an unknown id or invalid registry entry.
    pub fn from_registry(ids: &[&str]) -> Result<Self, ProviderError> {
        let mapped: Vec<(&str, &str)> = ids.iter().map(|id| (*id, *id)).collect();
        Self::from_registry_with_ids(&mapped)
    }

    /// Builds named questions from the pinned registry under request-local ids.
    ///
    /// The first member of each pair is the registry id and the second is the
    /// id emitted in the request. This keeps tuned instructions in the
    /// registry while allowing callers to identify repeated questions.
    /// # Errors
    /// Returns [`ProviderError::InvalidQuestion`] for an unknown id or invalid
    /// registry entry or request-local id.
    pub fn from_registry_with_ids(ids: &[(&str, &str)]) -> Result<Self, ProviderError> {
        let mut questions = Self::new();
        for (registry_id, request_id) in ids {
            let registered = registry::Registry::get()
                .question(registry_id)
                .ok_or(ProviderError::InvalidQuestion)?;
            questions = match registered.kind {
                registry::QuestionKind::Noul => {
                    questions.noul(request_id, &registered.instructions)?
                }
                registry::QuestionKind::Choice => {
                    let options: Vec<_> = registered
                        .options
                        .iter()
                        .map(|(key, value)| (key.as_str(), value.as_str()))
                        .collect();
                    questions.choice(request_id, &registered.instructions, &options)?
                }
                registry::QuestionKind::Score => {
                    let levels: Vec<_> = registered.levels.iter().map(String::as_str).collect();
                    questions.score(request_id, &registered.instructions, &levels)?
                }
            };
        }
        Ok(questions)
    }
}

fn valid_question_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 128 && id.split('.').all(valid_handle)
}

#[derive(Clone, Debug, PartialEq)]
pub enum Answer {
    Noul {
        probability: f64,
    },
    Choice {
        choice: String,
        probabilities: BTreeMap<String, f64>,
        confidence: f64,
    },
    Score {
        score: f64,
        probabilities: BTreeMap<String, f64>,
        confidence: f64,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Answers {
    answers: BTreeMap<String, Answer>,
    pub input_tokens: u64,
}

impl Answers {
    #[must_use]
    pub fn get(&self, id: &str) -> Option<&Answer> {
        self.answers.get(id)
    }
}

pub trait DecisionClient: Sync {
    fn model(&self) -> &str;
    fn max_parallel(&self) -> usize;
    /// Sends `state` with the typed questions.
    /// # Errors
    /// Returns a fixed provider error; no upstream text escapes.
    fn decide(
        &self,
        state: &Value,
        questions: &Questions,
        cancel: Option<&AtomicBool>,
        deadline: Duration,
    ) -> Result<Answers, ProviderError>;
}

fn append_json<T: serde::Serialize + ?Sized>(
    body: &mut Vec<u8>,
    value: &T,
) -> Result<(), ProviderError> {
    serde_json::to_writer(body, value).map_err(|_| ProviderError::InvalidResponse)
}

fn append_question(body: &mut Vec<u8>, question: &Question) -> Result<(), ProviderError> {
    match question {
        Question::Noul { instructions } => {
            body.extend_from_slice(b"{\"type\":\"noul\",\"instructions\":");
            append_json(body, instructions)?;
        }
        Question::Choice {
            instructions,
            options,
        } => {
            body.extend_from_slice(b"{\"type\":\"choice\",\"instructions\":");
            append_json(body, instructions)?;
            body.extend_from_slice(b",\"criteria\":{");
            for (index, (key, description)) in options.iter().enumerate() {
                if index != 0 {
                    body.push(b',');
                }
                append_json(body, key)?;
                body.push(b':');
                append_json(body, description)?;
            }
            body.push(b'}');
        }
        Question::Score {
            instructions,
            levels,
        } => {
            body.extend_from_slice(b"{\"type\":\"score\",\"instructions\":");
            append_json(body, instructions)?;
            body.extend_from_slice(b",\"criteria\":");
            append_json(body, levels)?;
        }
    }
    body.push(b'}');
    Ok(())
}

/// Builds a byte-deterministic decision request.
/// # Errors
/// Returns [`ProviderError::InputTooLarge`] when the request exceeds the shared cap.
pub fn request_body(
    model: &str,
    state: &Value,
    questions: &Questions,
    zdr_member: bool,
) -> Result<Vec<u8>, ProviderError> {
    let mut body = Vec::new();
    body.extend_from_slice(b"{\"model\":");
    append_json(&mut body, model)?;
    body.extend_from_slice(b",\"state\":");
    append_json(&mut body, state)?;
    body.extend_from_slice(b",\"questions\":{");
    for (index, (id, question)) in questions.items.iter().enumerate() {
        if index != 0 {
            body.push(b',');
        }
        append_json(&mut body, id)?;
        body.push(b':');
        append_question(&mut body, question)?;
    }
    body.push(b'}');
    if zdr_member {
        body.extend_from_slice(b",\"provider\":{\"zdr\":true}");
    }
    body.push(b'}');
    if body.len() > MAX_REQUEST {
        return Err(ProviderError::InputTooLarge);
    }
    Ok(body)
}

fn object_with_members<'a>(
    value: &'a Value,
    allowed: &[&str],
) -> Result<&'a serde_json::Map<String, Value>, ProviderError> {
    let object = value.as_object().ok_or(ProviderError::InvalidResponse)?;
    if object.keys().any(|key| !allowed.contains(&key.as_str())) {
        return Err(ProviderError::InvalidResponse);
    }
    Ok(object)
}

fn probability(value: Option<&Value>) -> Result<f64, ProviderError> {
    value
        .and_then(Value::as_f64)
        .filter(|value| (0.0..=1.0).contains(value))
        .ok_or(ProviderError::InvalidResponse)
}

fn probabilities(value: Option<&Value>) -> Result<BTreeMap<String, f64>, ProviderError> {
    value
        .and_then(Value::as_object)
        .ok_or(ProviderError::InvalidResponse)?
        .iter()
        .map(|(key, value)| Ok((key.clone(), probability(Some(value))?)))
        .collect()
}

fn parse_answer(value: &Value, question: &Question) -> Result<Answer, ProviderError> {
    match question {
        Question::Noul { .. } => {
            let object = object_with_members(value, &["type", "noul"])?;
            if object.get("type").and_then(Value::as_str) != Some("noul") {
                return Err(ProviderError::InvalidResponse);
            }
            Ok(Answer::Noul {
                probability: probability(object.get("noul"))?,
            })
        }
        Question::Choice { options, .. } => parse_choice(value, options),
        Question::Score { levels, .. } => parse_score(value, levels.len()),
    }
}

fn parse_choice(value: &Value, options: &[(String, String)]) -> Result<Answer, ProviderError> {
    let object = object_with_members(value, &["type", "choice", "probabilities", "confidence"])?;
    if object.get("type").and_then(Value::as_str) != Some("choice") {
        return Err(ProviderError::InvalidResponse);
    }
    let issued = |key: &str| options.iter().any(|(issued, _)| issued == key);
    let choice = object
        .get("choice")
        .and_then(Value::as_str)
        .filter(|choice| issued(choice))
        .ok_or(ProviderError::InvalidResponse)?;
    let probabilities = probabilities(object.get("probabilities"))?;
    if probabilities.keys().any(|key| !issued(key)) {
        return Err(ProviderError::InvalidResponse);
    }
    Ok(Answer::Choice {
        choice: choice.to_owned(),
        probabilities,
        confidence: probability(object.get("confidence"))?,
    })
}

fn parse_score(value: &Value, level_count: usize) -> Result<Answer, ProviderError> {
    let object = object_with_members(
        value,
        &["type", "score", "probabilities", "legend", "confidence"],
    )?;
    if object.get("type").and_then(Value::as_str) != Some("score")
        || !object.get("legend").is_some_and(Value::is_object)
    {
        return Err(ProviderError::InvalidResponse);
    }
    let max_score = u32::try_from(level_count - 1)
        .map(f64::from)
        .map_err(|_| ProviderError::InvalidResponse)?;
    let score = object
        .get("score")
        .and_then(Value::as_f64)
        .filter(|score| (0.0..=max_score).contains(score))
        .ok_or(ProviderError::InvalidResponse)?;
    Ok(Answer::Score {
        score,
        probabilities: probabilities(object.get("probabilities"))?,
        confidence: probability(object.get("confidence"))?,
    })
}

/// Strictly validates one decision response against the issued questions.
/// # Errors
/// Returns [`ProviderError::InvalidResponse`] for every malformed response.
pub fn parse_answers(
    bytes: &[u8],
    selected_model: &str,
    questions: &Questions,
) -> Result<Answers, ProviderError> {
    let value = openloops_contracts::parse_strict_json(bytes)
        .map_err(|_| ProviderError::InvalidResponse)?;
    let object = object_with_members(
        &value,
        &[
            "model", "answers", "usage", "id", "object", "created", "provider",
        ],
    )?;
    if object.get("error").is_some()
        || !object
            .get("model")
            .and_then(Value::as_str)
            .is_some_and(|model| selected_model_matches(model, selected_model))
    {
        return Err(ProviderError::InvalidResponse);
    }
    let supplied = object
        .get("answers")
        .and_then(Value::as_object)
        .ok_or(ProviderError::InvalidResponse)?;
    if supplied.len() != questions.len() {
        return Err(ProviderError::InvalidResponse);
    }
    let mut answers = BTreeMap::new();
    for (id, question) in &questions.items {
        answers.insert(
            id.clone(),
            parse_answer(
                supplied.get(id).ok_or(ProviderError::InvalidResponse)?,
                question,
            )?,
        );
    }
    let input_tokens = parse_usage(object.get("usage"))?;
    Ok(Answers {
        answers,
        input_tokens,
    })
}

/// `usage` is provider accounting metadata: `OpenRouter` adds members such as
/// `cost` beside `TypeSafe`'s token counts, so only `input_tokens` is read and
/// the rest of the object is ignored. It must still be an object with a
/// non-negative integer `input_tokens`.
fn parse_usage(value: Option<&Value>) -> Result<u64, ProviderError> {
    let Some(value) = value else { return Ok(0) };
    value
        .as_object()
        .and_then(|usage| usage.get("input_tokens"))
        .and_then(Value::as_u64)
        .ok_or(ProviderError::InvalidResponse)
}

/// The exact selected label, or that label followed by `:` or `-` and a
/// variant or dated build such as `typesafe/jev-1.13-20260917`, which is what
/// the decisions endpoint reports for the build it served. Any other label
/// rejects.
fn selected_model_matches(reported: &str, selected: &str) -> bool {
    reported.strip_prefix(selected).is_some_and(|suffix| {
        suffix.is_empty()
            || suffix.strip_prefix([':', '-']).is_some_and(|rest| {
                !rest.is_empty()
                    && rest
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
            })
    })
}

pub mod registry {
    use std::collections::BTreeMap;
    use std::sync::OnceLock;

    use serde_json::Value;

    use super::MAX_CHOICE_OPTIONS;
    use crate::analysis::valid_handle;
    use crate::provider::ProviderError;

    pub const REQUIRED_IDS: &[&str] = &[
        "check.asks_recipient",
        "check.kind",
        "closure.fulfilled",
        "closure.withdrawn",
        "closure.deadline_changed",
        "closure.modified",
        "triage.asks_recipient",
        "triage.commits_sender",
        "triage.asks_question",
        "triage.names_time",
        "triage.boilerplate",
        "triage.automated_notification",
        "rules.recap",
        "rules.scoped_event",
        "rules.event_match",
        "rules.duplicate_action",
        "rules.thread_merge",
        "rules.deadline_kind",
    ];
    pub const CHECK_IDS: &[&str] = &["check.asks_recipient", "check.kind"];

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum QuestionKind {
        Noul,
        Choice,
        Score,
    }

    #[derive(Clone, Debug, PartialEq)]
    pub struct Registered {
        pub kind: QuestionKind,
        pub instructions: String,
        pub options: Vec<(String, String)>,
        pub levels: Vec<String>,
        pub accept: f64,
        pub escalate: f64,
    }

    #[derive(Debug)]
    pub struct Registry {
        pub model: String,
        questions: BTreeMap<String, Registered>,
    }

    impl Registry {
        #[must_use]
        /// Returns the validated embedded registry.
        /// # Panics
        /// Panics when the source-controlled embedded registry is invalid.
        pub fn get() -> &'static Self {
            static REGISTRY: OnceLock<Registry> = OnceLock::new();
            REGISTRY.get_or_init(|| {
                parse(include_str!(
                    "../../../contracts/model/decision-questions.json"
                ))
                .expect("the embedded decision question registry must be valid")
            })
        }

        #[must_use]
        pub fn question(&self, id: &str) -> Option<&Registered> {
            self.questions.get(id)
        }
    }

    fn parse(source: &str) -> Result<Registry, ProviderError> {
        let value = openloops_contracts::parse_strict_json(source.as_bytes())
            .map_err(|_| ProviderError::InvalidQuestion)?;
        let root = value.as_object().ok_or(ProviderError::InvalidQuestion)?;
        if root.keys().any(|key| {
            ![
                "schema_version",
                "model",
                "tuned_at",
                "questions",
                "metrics",
            ]
            .contains(&key.as_str())
        }) || root.get("schema_version").and_then(Value::as_u64) != Some(1)
        {
            return Err(ProviderError::InvalidQuestion);
        }
        let model = root
            .get("model")
            .and_then(Value::as_str)
            .ok_or(ProviderError::InvalidQuestion)?;
        let entries = root
            .get("questions")
            .and_then(Value::as_object)
            .ok_or(ProviderError::InvalidQuestion)?;
        let questions = entries
            .iter()
            .map(|(id, value)| Ok((id.clone(), parse_registered(value)?)))
            .collect::<Result<_, ProviderError>>()?;
        Ok(Registry {
            model: model.to_owned(),
            questions,
        })
    }

    fn parse_registered(value: &Value) -> Result<Registered, ProviderError> {
        let entry = value.as_object().ok_or(ProviderError::InvalidQuestion)?;
        let instructions = entry
            .get("instructions")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or(ProviderError::InvalidQuestion)?;
        let accept = entry
            .get("accept")
            .and_then(Value::as_f64)
            .filter(|value| (0.0..=1.0).contains(value))
            .ok_or(ProviderError::InvalidQuestion)?;
        let escalate = entry
            .get("escalate")
            .and_then(Value::as_f64)
            .filter(|value| (0.0..=1.0).contains(value) && *value < accept)
            .ok_or(ProviderError::InvalidQuestion)?;
        let kind = entry
            .get("type")
            .and_then(Value::as_str)
            .ok_or(ProviderError::InvalidQuestion)?;
        let (kind, options, levels) = match kind {
            "noul" if entry.get("options").is_none() && entry.get("levels").is_none() => {
                (QuestionKind::Noul, vec![], vec![])
            }
            "choice" if entry.get("levels").is_none() => {
                let options = entry
                    .get("options")
                    .and_then(Value::as_object)
                    .filter(|options| !options.is_empty() && options.len() <= MAX_CHOICE_OPTIONS)
                    .ok_or(ProviderError::InvalidQuestion)?
                    .iter()
                    .map(|(key, value)| {
                        value
                            .as_str()
                            .filter(|_| valid_handle(key))
                            .map(|description| (key.clone(), description.to_owned()))
                            .ok_or(ProviderError::InvalidQuestion)
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                (QuestionKind::Choice, options, vec![])
            }
            "score" if entry.get("options").is_none() => {
                let levels = entry
                    .get("levels")
                    .and_then(Value::as_array)
                    .filter(|levels| levels.len() >= 2)
                    .ok_or(ProviderError::InvalidQuestion)?
                    .iter()
                    .map(|level| {
                        level
                            .as_str()
                            .map(str::to_owned)
                            .ok_or(ProviderError::InvalidQuestion)
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                (QuestionKind::Score, vec![], levels)
            }
            _ => return Err(ProviderError::InvalidQuestion),
        };
        Ok(Registered {
            kind,
            instructions: instructions.to_owned(),
            options,
            levels,
            accept,
            escalate,
        })
    }

    #[cfg(test)]
    pub(super) fn parse_for_test(source: &str) -> Result<Registry, ProviderError> {
        parse(source)
    }
}

#[cfg(feature = "openrouter")]
mod openrouter_adapter {
    use std::sync::atomic::AtomicBool;
    use std::time::Duration;
    use std::time::Instant;

    use reqwest::blocking::Client;
    use serde_json::Value;
    use zeroize::Zeroizing;

    use super::{
        Answer, Answers, DECISION_DEADLINE, DecisionClient, Questions, parse_answers, registry,
        request_body,
    };
    use crate::openrouter::{AUTHORITY, fetch_zdr, read_response};
    use crate::provider::{
        MAX_PARALLEL_REQUESTS, ProviderError, RequestControl, https_client, send_with_control,
        valid_key, valid_model_name,
    };

    const DECISIONS: &str = "https://openrouter.ai/api/alpha/decisions";
    const ZDR_ENDPOINTS: &str = "https://openrouter.ai/api/v1/endpoints/zdr";

    pub struct OpenRouterDecisions {
        client: Client,
        key: Zeroizing<String>,
        model: String,
        parallel: usize,
        zdr_member: bool,
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct DecisionCheckReport {
        pub zdr_member_supported: bool,
        pub latency_ms: u64,
    }

    impl OpenRouterDecisions {
        /// Connects only when the pinned model is in the public ZDR listing.
        /// # Errors
        /// Returns a fixed validation, transport, or availability error.
        pub fn connect(key: String, model: &str) -> Result<Self, ProviderError> {
            let key = Zeroizing::new(key);
            if !valid_key(&key) {
                return Err(ProviderError::InvalidKey);
            }
            if !valid_model_name(model) {
                return Err(ProviderError::ModelUnavailable);
            }
            let client = https_client()?;
            if !fetch_zdr(&client, ZDR_ENDPOINTS)?
                .iter()
                .any(|choice| choice.id == model)
            {
                return Err(ProviderError::ModelUnavailable);
            }
            Ok(Self {
                client,
                key,
                model: model.to_owned(),
                parallel: 1,
                zdr_member: true,
            })
        }

        #[must_use]
        pub fn with_max_parallel(mut self, parallel: usize) -> Self {
            self.parallel = parallel.clamp(1, MAX_PARALLEL_REQUESTS);
            self
        }

        /// Runs the synthetic decision endpoint check.
        /// # Errors
        /// Returns a fixed provider error or an invalid-response result when the answers fail the registry thresholds.
        pub fn check(&mut self) -> Result<DecisionCheckReport, ProviderError> {
            self.check_at(DECISIONS)
        }

        pub(super) fn check_at(&mut self, url: &str) -> Result<DecisionCheckReport, ProviderError> {
            let started = Instant::now();
            let state = serde_json::json!({"text":"Please send the signed form by Friday."});
            let questions = Questions::from_registry(registry::CHECK_IDS)?;
            let first = self.decide_at(url, &state, &questions, None, DECISION_DEADLINE);
            let answers = match first {
                Err(ProviderError::RequestRejected(400 | 422)) if self.zdr_member => {
                    self.zdr_member = false;
                    let remaining = DECISION_DEADLINE.saturating_sub(started.elapsed());
                    self.decide_at(url, &state, &questions, None, remaining)?
                }
                result => result?,
            };
            let ask = matches!(answers.get("check.asks_recipient"), Some(Answer::Noul { probability }) if *probability >= registry::Registry::get().question("check.asks_recipient").expect("required registry id").accept);
            let kind = matches!(answers.get("check.kind"), Some(Answer::Choice { choice, .. }) if choice == "request");
            if !ask || !kind {
                return Err(ProviderError::InvalidResponse);
            }
            Ok(DecisionCheckReport {
                zdr_member_supported: self.zdr_member,
                latency_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
            })
        }

        fn decide_at(
            &self,
            url: &str,
            state: &Value,
            questions: &Questions,
            cancel: Option<&AtomicBool>,
            deadline: Duration,
        ) -> Result<Answers, ProviderError> {
            debug_assert!(cfg!(test) || url.starts_with(AUTHORITY));
            let started = Instant::now();
            let control = RequestControl::with_cancel_and_deadline(started, cancel, deadline);
            let response = send_with_control(
                self.client
                    .post(url)
                    .bearer_auth(self.key.as_str())
                    .header(reqwest::header::CONTENT_TYPE, "application/json")
                    .body(request_body(
                        &self.model,
                        state,
                        questions,
                        self.zdr_member,
                    )?),
                &control,
            )?;
            parse_answers(&read_response(response, &control)?, &self.model, questions)
        }

        #[cfg(test)]
        pub(super) fn for_test(model: &str) -> Self {
            Self {
                client: Client::builder()
                    .no_proxy()
                    .build()
                    .expect("loopback client"),
                key: Zeroizing::new("synthetic-key".into()),
                model: model.to_owned(),
                parallel: 1,
                zdr_member: true,
            }
        }
    }

    impl DecisionClient for OpenRouterDecisions {
        fn model(&self) -> &str {
            &self.model
        }
        fn max_parallel(&self) -> usize {
            self.parallel
        }
        fn decide(
            &self,
            state: &Value,
            questions: &Questions,
            cancel: Option<&AtomicBool>,
            deadline: Duration,
        ) -> Result<Answers, ProviderError> {
            self.decide_at(DECISIONS, state, questions, cancel, deadline)
        }
    }
}

#[cfg(feature = "openrouter")]
pub use openrouter_adapter::{DecisionCheckReport, OpenRouterDecisions};

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const MODEL: &str = "typesafe/jev-1.13";

    fn questions() -> Questions {
        Questions::new()
            .noul("ask", "The text asks for an action.")
            .unwrap()
            .choice(
                "kind",
                "Choose the statement kind.",
                &[("request", "A request."), ("none", "Neither.")],
            )
            .unwrap()
            .score("urgency", "Rate urgency.", &["low", "high"])
            .unwrap()
    }

    fn documented_response() -> Value {
        json!({
            "id": "decision-synthetic",
            "object": "decision",
            "created": 1,
            "provider": "synthetic-provider",
            "model": MODEL,
            "answers": {
                "ask": {"type":"noul","noul":0.8},
                "kind": {"type":"choice","choice":"request","probabilities":{"request":0.8,"none":0.2},"confidence":0.9},
                "urgency": {"type":"score","score":1.0,"probabilities":{"0":0.1,"1":0.9},"legend":{"0":"low","1":"high"},"confidence":0.8}
            },
            "usage": {"input_tokens":12,"output_tokens":0}
        })
    }

    fn parse(value: &Value) -> Result<Answers, ProviderError> {
        parse_answers(&serde_json::to_vec(value).unwrap(), MODEL, &questions())
    }

    #[test]
    fn request_body_has_model_state_questions_in_order_and_provider_last() {
        let questions = Questions::new()
            .noul("ask", "Act?")
            .unwrap()
            .choice(
                "kind",
                "Kind?",
                &[("request", "Ask."), ("none", "Neither.")],
            )
            .unwrap()
            .score("score", "Score?", &["low", "high"])
            .unwrap();
        let without = br#"{"model":"typesafe/jev-1.13","state":{"text":"Synthetic text."},"questions":{"ask":{"type":"noul","instructions":"Act?"},"kind":{"type":"choice","instructions":"Kind?","criteria":{"request":"Ask.","none":"Neither."}},"score":{"type":"score","instructions":"Score?","criteria":["low","high"]}}}"#;
        let with = br#"{"model":"typesafe/jev-1.13","state":{"text":"Synthetic text."},"questions":{"ask":{"type":"noul","instructions":"Act?"},"kind":{"type":"choice","instructions":"Kind?","criteria":{"request":"Ask.","none":"Neither."}},"score":{"type":"score","instructions":"Score?","criteria":["low","high"]}},"provider":{"zdr":true}}"#;
        assert_eq!(
            request_body(MODEL, &json!({"text":"Synthetic text."}), &questions, false).unwrap(),
            without
        );
        assert_eq!(
            request_body(MODEL, &json!({"text":"Synthetic text."}), &questions, true).unwrap(),
            with
        );
    }

    #[test]
    fn question_builder_rejects_bad_ids_duplicates_empty_options_and_short_scores() {
        assert_eq!(
            Questions::new().noul("bad id", "x").err(),
            Some(ProviderError::InvalidQuestion)
        );
        assert_eq!(
            Questions::new().noul("id", "").err(),
            Some(ProviderError::InvalidQuestion)
        );
        assert_eq!(
            Questions::new()
                .noul("id", "x")
                .unwrap()
                .noul("id", "y")
                .err(),
            Some(ProviderError::InvalidQuestion)
        );
        assert_eq!(
            Questions::new().choice("id", "x", &[]).err(),
            Some(ProviderError::InvalidQuestion)
        );
        assert_eq!(
            Questions::new()
                .choice("id", "x", &[("same", "a"), ("same", "b")])
                .err(),
            Some(ProviderError::InvalidQuestion)
        );
        assert_eq!(
            Questions::new()
                .choice("id", "x", &[("bad key", "a")])
                .err(),
            Some(ProviderError::InvalidQuestion)
        );
        let too_many = (0..=MAX_CHOICE_OPTIONS)
            .map(|index| (format!("key{index}"), String::from("value")))
            .collect::<Vec<_>>();
        let too_many_refs = too_many
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
            .collect::<Vec<_>>();
        assert_eq!(
            Questions::new().choice("id", "x", &too_many_refs).err(),
            Some(ProviderError::InvalidQuestion)
        );
        assert_eq!(
            Questions::new().score("id", "x", &["one"]).err(),
            Some(ProviderError::InvalidQuestion)
        );
    }

    #[test]
    fn request_body_over_max_request_is_input_too_large() {
        let state = json!({"text":"x".repeat(MAX_REQUEST)});
        assert_eq!(
            request_body(MODEL, &state, &Questions::new(), false),
            Err(ProviderError::InputTooLarge)
        );
    }

    #[test]
    fn parse_answers_accepts_a_documented_response_and_ignores_the_envelope() {
        let answers = parse(&documented_response()).unwrap();
        assert_eq!(answers.input_tokens, 12);
        assert!(
            matches!(answers.get("ask"), Some(Answer::Noul { probability }) if (*probability - 0.8).abs() < f64::EPSILON)
        );
        assert!(
            matches!(answers.get("kind"), Some(Answer::Choice { choice, .. }) if choice == "request")
        );
        assert!(
            matches!(answers.get("urgency"), Some(Answer::Score { score, .. }) if (*score - 1.0).abs() < f64::EPSILON)
        );
    }

    #[test]
    fn parse_answers_rejects_unknown_top_level_member() {
        let mut value = documented_response();
        value["unknown"] = json!(true);
        assert_eq!(parse(&value), Err(ProviderError::InvalidResponse));
    }

    #[test]
    fn parse_answers_rejects_error_member() {
        let mut value = documented_response();
        value["error"] = json!({"message":"synthetic"});
        assert_eq!(parse(&value), Err(ProviderError::InvalidResponse));
    }

    #[test]
    fn parse_answers_rejects_model_mismatch() {
        let mut value = documented_response();
        value["model"] = json!("typesafe/other");
        assert_eq!(parse(&value), Err(ProviderError::InvalidResponse));
    }

    #[test]
    fn parse_answers_accepts_a_dated_build_of_the_selected_model() {
        let mut value = documented_response();
        value["model"] = json!("typesafe/jev-1.13-20260917");
        assert!(parse(&value).is_ok());
        value["model"] = json!("typesafe/jev-1.13:beta");
        assert!(parse(&value).is_ok());
        value["model"] = json!("typesafe/jev-1.130");
        assert_eq!(parse(&value), Err(ProviderError::InvalidResponse));
        value["model"] = json!("typesafe/jev-1.13-");
        assert_eq!(parse(&value), Err(ProviderError::InvalidResponse));
    }

    #[test]
    fn parse_answers_rejects_missing_question() {
        let mut value = documented_response();
        value["answers"]
            .as_object_mut()
            .unwrap()
            .shift_remove("ask");
        assert_eq!(parse(&value), Err(ProviderError::InvalidResponse));
    }

    #[test]
    fn parse_answers_rejects_extra_question() {
        let mut value = documented_response();
        value["answers"]["extra"] = json!({"type":"noul","noul":0.5});
        assert_eq!(parse(&value), Err(ProviderError::InvalidResponse));
    }

    #[test]
    fn parse_answers_rejects_wrong_type() {
        let mut value = documented_response();
        value["answers"]["ask"]["type"] = json!("choice");
        assert_eq!(parse(&value), Err(ProviderError::InvalidResponse));
    }

    #[test]
    fn parse_answers_rejects_probability_out_of_range() {
        let mut value = documented_response();
        value["answers"]["ask"]["noul"] = json!(1.1);
        assert_eq!(parse(&value), Err(ProviderError::InvalidResponse));
    }

    #[test]
    fn parse_answers_rejects_unissued_choice_key() {
        let mut value = documented_response();
        value["answers"]["kind"]["choice"] = json!("other");
        assert_eq!(parse(&value), Err(ProviderError::InvalidResponse));
        value = documented_response();
        value["answers"]["kind"]["probabilities"]["other"] = json!(0.1);
        assert_eq!(parse(&value), Err(ProviderError::InvalidResponse));
    }

    #[test]
    fn parse_answers_rejects_extra_member_inside_answer() {
        let mut value = documented_response();
        value["answers"]["ask"]["text"] = json!("synthetic");
        assert_eq!(parse(&value), Err(ProviderError::InvalidResponse));
    }

    #[test]
    fn parse_answers_rejects_duplicate_members() {
        let bytes = br#"{"model":"typesafe/jev-1.13","model":"typesafe/jev-1.13","answers":{}}"#;
        assert_eq!(
            parse_answers(bytes, MODEL, &Questions::new()),
            Err(ProviderError::InvalidResponse)
        );
    }

    #[test]
    fn registry_loads_and_contains_every_required_id() {
        let registry = registry::Registry::get();
        assert_eq!(registry.model, MODEL);
        for id in registry::REQUIRED_IDS {
            assert!(registry.question(id).is_some(), "missing {id}");
        }
        assert_eq!(
            Questions::from_registry(registry::REQUIRED_IDS)
                .unwrap()
                .len(),
            registry::REQUIRED_IDS.len()
        );
    }

    #[test]
    fn registry_fixture_contains_all_p1_through_p3_ids() {
        let source = include_str!("../../../contracts/model/decision-questions.json");
        let fixture = registry::parse_for_test(source).unwrap();
        for id in &registry::REQUIRED_IDS[2..] {
            assert!(fixture.question(id).is_some(), "missing fixture id {id}");
        }
    }

    #[test]
    fn registry_rejects_escalate_at_or_above_accept() {
        let source = r#"{"schema_version":1,"model":"typesafe/jev-1.13","tuned_at":null,"questions":{"test":{"type":"noul","instructions":"Synthetic.","accept":0.7,"escalate":0.7}}}"#;
        assert!(registry::parse_for_test(source).is_err());
    }

    #[test]
    fn decide_uses_the_20_second_default_deadline_constant() {
        assert_eq!(DECISION_DEADLINE, Duration::from_secs(20));
    }

    #[cfg(feature = "openrouter")]
    mod openrouter_tests {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::time::Duration;

        use super::*;

        fn http(status: u16, body: &[u8]) -> Vec<u8> {
            let mut response = format!("HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).into_bytes();
            response.extend_from_slice(body);
            response
        }

        fn check_answer() -> Vec<u8> {
            serde_json::to_vec(&json!({"model":MODEL,"answers":{"check.asks_recipient":{"type":"noul","noul":0.9},"check.kind":{"type":"choice","choice":"request","probabilities":{"request":0.9,"promise":0.05,"none":0.05},"confidence":0.9}},"usage":{"input_tokens":8,"output_tokens":0}})).unwrap()
        }

        fn loopback(responses: Vec<Vec<u8>>) -> (String, std::sync::mpsc::Receiver<Vec<u8>>) {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let (sender, receiver) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                for response in responses {
                    let (mut socket, _) = listener.accept().unwrap();
                    socket
                        .set_read_timeout(Some(Duration::from_secs(5)))
                        .unwrap();
                    let mut request = Vec::new();
                    let mut buffer = [0u8; 4096];
                    while let Ok(read) = socket.read(&mut buffer) {
                        if read == 0 {
                            break;
                        }
                        request.extend_from_slice(&buffer[..read]);
                        let Some(start) = request
                            .windows(4)
                            .position(|part| part == b"\r\n\r\n")
                            .map(|value| value + 4)
                        else {
                            continue;
                        };
                        let length = String::from_utf8_lossy(&request[..start])
                            .lines()
                            .find_map(|line| {
                                line.split_once(':')
                                    .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                                    .and_then(|(_, value)| value.trim().parse::<usize>().ok())
                            })
                            .unwrap_or(0);
                        if request.len() >= start + length {
                            break;
                        }
                    }
                    socket.write_all(&response).unwrap();
                    sender.send(request).unwrap();
                }
            });
            (format!("http://{address}/"), receiver)
        }

        fn body(request: &[u8]) -> Value {
            let start = request
                .windows(4)
                .position(|part| part == b"\r\n\r\n")
                .unwrap()
                + 4;
            serde_json::from_slice(&request[start..]).unwrap()
        }

        #[test]
        fn check_records_provider_member_support_from_a_422_then_200() {
            let (url, captured) = loopback(vec![http(422, b"{}"), http(200, &check_answer())]);
            let mut client = OpenRouterDecisions::for_test(MODEL);
            let report = client.check_at(&url).unwrap();
            assert!(!report.zdr_member_supported);
            assert!(
                body(&captured.recv_timeout(Duration::from_secs(5)).unwrap())
                    .get("provider")
                    .is_some()
            );
            assert!(
                body(&captured.recv_timeout(Duration::from_secs(5)).unwrap())
                    .get("provider")
                    .is_none()
            );
        }

        #[test]
        fn check_keeps_provider_member_on_first_200() {
            let (url, captured) = loopback(vec![http(200, &check_answer())]);
            let mut client = OpenRouterDecisions::for_test(MODEL);
            let report = client.check_at(&url).unwrap();
            assert!(report.zdr_member_supported);
            assert!(
                body(&captured.recv_timeout(Duration::from_secs(5)).unwrap())
                    .get("provider")
                    .is_some()
            );
        }
    }
}
