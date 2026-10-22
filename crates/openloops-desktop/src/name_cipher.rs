use openloops_inference::decision::{Answers, DecisionClient, Questions};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::AtomicBool;
use std::time::Duration;

const GIVEN_NAMES: [&str; 24] = [
    "Priya", "Tomas", "Amara", "Mateo", "Linnea", "Devon", "Soraya", "Kenji", "Ingrid", "Rafael",
    "Noor", "Callum", "Yesenia", "Anselm", "Talia", "Oskar", "Imani", "Lorcan", "Mireille",
    "Dashiell", "Saoirse", "Bartek", "Odalys", "Tobiah",
];
const FAMILY_NAMES: [&str; 24] = [
    "Venkataraman",
    "Lindqvist",
    "Okafor",
    "Villanueva",
    "Bergstrom",
    "Fairchild",
    "Nwosu",
    "Takamura",
    "Halvorsen",
    "Quintero",
    "Abernathy",
    "Szymanski",
    "Ferreira",
    "Achterberg",
    "Castellanos",
    "Oyelaran",
    "Whitcombe",
    "Marchetti",
    "Kowalczyk",
    "Thorsby",
    "Ebbesen",
    "Arceneaux",
    "Lindgren",
    "Pemberton",
];
const SYSTEM_WORDS: &[&str] = &[
    "inc",
    "llc",
    "ltd",
    "team",
    "support",
    "billing",
    "notifications",
    "service",
    "admin",
    "sales",
    "help",
    "desk",
    "calendar",
    "bot",
    "reply",
];
const SYSTEM_LOCALS: &[&str] = &[
    "noreply",
    "no-reply",
    "donotreply",
    "notifications",
    "mailer-daemon",
    "postmaster",
];
const NAME_PARTICLES: &[&str] = &[
    "al", "bin", "da", "das", "de", "del", "della", "der", "di", "dos", "du", "ibn", "la", "le",
    "ten", "ter", "van", "von",
];
const NAME_SUFFIXES: &[&str] = &[
    "jr", "sr", "ii", "iii", "iv", "phd", "md", "esq", "cpa", "jd", "mba",
];
const ENGLISH_GIVEN_NAMES: &[&str] = &[
    "will", "may", "mark", "bill", "grant", "hope", "june", "april", "august", "rose", "chase",
    "drew", "dawn", "art", "pat", "sue", "rob", "joy", "faith", "summer", "frank", "gene", "jack",
    "ray", "guy", "dean", "penny", "ruby", "grace", "holly", "iris", "hunter", "carter", "parker",
    "mason",
];

#[derive(Default)]
pub(crate) struct NameCipher {
    people: Vec<Person>,
    by_address: BTreeMap<String, usize>,
    protected_addresses: BTreeSet<String>,
    name_forms: BTreeMap<char, Vec<NameLookup>>,
}

#[derive(Default)]
struct Person {
    addresses: BTreeSet<String>,
    names: Vec<PersonName>,
}

#[derive(Clone)]
struct PersonName {
    canonical_full: String,
    full_forms: Vec<String>,
    given: String,
    family: Option<String>,
}

#[derive(Clone)]
struct Pseudonym {
    given: String,
    family: Option<String>,
    full: String,
    address: String,
}

struct AssignedPerson {
    pseudonym: Pseudonym,
    real_name: PersonName,
    real_address: String,
}

#[derive(Clone)]
struct DecodeForm {
    pseudo: String,
    real: String,
    priority: u8,
}

#[derive(Clone, Copy, Debug)]
struct NameLookup {
    person_index: usize,
    name_index: usize,
    part: NamePart,
}

#[derive(Clone, Copy, Debug)]
enum NamePart {
    Full(usize),
    Family,
    Given,
}

impl NamePart {
    fn match_kind(self) -> MatchKind {
        match self {
            Self::Full(_) => MatchKind::Full,
            Self::Family => MatchKind::Family,
            Self::Given => MatchKind::Given,
        }
    }
}

pub(crate) struct EncodedRequest {
    pub(crate) state: Value,
    pub(crate) questions: Questions,
    pub(crate) assignment: RequestAssignment,
}

pub(crate) struct RequestAssignment {
    assigned: BTreeMap<usize, AssignedPerson>,
    skipped_tokens: BTreeSet<String>,
    next_pool_index: usize,
    overflow_index: usize,
}

impl NameCipher {
    pub(crate) fn from_messages(messages: &[crate::review_model::ReviewMessage]) -> Self {
        let mut cipher = Self::default();
        for message in messages {
            let participants = message
                .input
                .message
                .sender
                .iter()
                .chain(&message.input.message.to)
                .chain(&message.input.message.cc);
            for participant in participants {
                cipher.add_participant(&participant.as_string(), None, None);
            }
            for address in &message.own_addresses {
                cipher.add_participant(
                    message.own_display_name.as_deref().unwrap_or_default(),
                    Some(address),
                    message.own_given_name.as_deref(),
                );
            }
        }
        cipher.rebuild_name_forms();
        cipher
    }

    fn rebuild_name_forms(&mut self) {
        self.name_forms.clear();
        for (person_index, person) in self.people.iter().enumerate() {
            for (name_index, name) in person.names.iter().enumerate() {
                for (form_index, form) in name.full_forms.iter().enumerate() {
                    if let Some(first) = form.chars().next() {
                        self.name_forms.entry(first).or_default().push(NameLookup {
                            person_index,
                            name_index,
                            part: NamePart::Full(form_index),
                        });
                    }
                }
                if let Some(family) = &name.family
                    && family.chars().count() >= 2
                    && let Some(first) = family.chars().next()
                {
                    self.name_forms.entry(first).or_default().push(NameLookup {
                        person_index,
                        name_index,
                        part: NamePart::Family,
                    });
                }
                if name.given.chars().count() >= 2
                    && let Some(first) = name.given.chars().next()
                {
                    self.name_forms.entry(first).or_default().push(NameLookup {
                        person_index,
                        name_index,
                        part: NamePart::Given,
                    });
                }
            }
        }
    }

    fn add_participant(
        &mut self,
        label: &str,
        address_override: Option<&str>,
        given_hint: Option<&str>,
    ) {
        let (display, label_address) = participant_parts(label);
        // The user's own display name arrives bare, without an address.
        let display = display.or_else(|| {
            address_override
                .map(|_| label.trim())
                .filter(|label| !label.is_empty())
        });
        let address = address_override.or(label_address).map(str::trim);
        let Some(address) = address.filter(|value| !value.is_empty()) else {
            return;
        };
        let address = address.to_ascii_lowercase();
        let parsed_name = display
            .and_then(parse_person_name)
            .or_else(|| given_hint.and_then(parse_person_name));
        if is_system_address(&address)
            || display.is_some_and(|display| is_system_display_name(&clean_display_name(display)))
        {
            self.protected_addresses.insert(address);
            return;
        }
        let Some(mut name) = parsed_name else {
            return;
        };
        name.full_forms.sort();
        name.full_forms.dedup();
        // A given-name hint that differs from the display name ("Elsie" for
        // "Elsabet Morrow") is a second name for the same person; the display
        // given name stays matched too.
        let hint_name = given_hint
            .and_then(parse_single_name_word)
            .filter(|given| *given != name.given)
            .map(|given| {
                let canonical_full = name
                    .family
                    .as_ref()
                    .map_or_else(|| given.clone(), |family| format!("{given} {family}"));
                PersonName {
                    full_forms: vec![canonical_full.clone()],
                    canonical_full,
                    given,
                    family: name.family.clone(),
                }
            });
        let index = if let Some(index) = self.by_address.get(&address).copied() {
            index
        } else {
            let index = self.people.len();
            self.people.push(Person::default());
            self.by_address.insert(address.clone(), index);
            index
        };
        let person = &mut self.people[index];
        person.addresses.insert(address);
        for name in std::iter::once(name).chain(hint_name) {
            if !person.names.iter().any(|existing| {
                existing.canonical_full == name.canonical_full
                    && existing.given == name.given
                    && existing.family == name.family
            }) {
                person.names.push(name);
            }
        }
    }

    pub(crate) fn encode_request(&self, state: &Value, questions: &Questions) -> EncodedRequest {
        let mut texts = Vec::new();
        collect_state_texts(state, &mut texts);
        let _ = questions.map_choice_descriptions(|description| {
            texts.push(description.to_owned());
            description.to_owned()
        });
        let mut assignment = RequestAssignment {
            assigned: BTreeMap::new(),
            skipped_tokens: request_name_tokens(&texts),
            next_pool_index: 0,
            overflow_index: 25,
        };
        let encoded_state =
            map_value_strings(state, &mut |text| assignment.encode_text(self, text));
        let encoded_questions = questions
            .map_choice_descriptions(|description| assignment.encode_text(self, description));
        EncodedRequest {
            state: encoded_state,
            questions: encoded_questions,
            assignment,
        }
    }

    #[cfg(test)]
    fn from_test_people(people: impl IntoIterator<Item = (String, String)>) -> Self {
        let mut cipher = Self::default();
        for (name, address) in people {
            cipher.add_participant(&format!("{name} <{address}>"), None, None);
        }
        cipher.rebuild_name_forms();
        cipher
    }
}

impl Pseudonym {
    fn new(given: &str, family: Option<&str>) -> Self {
        let full = family.map_or_else(|| given.to_owned(), |family| format!("{given} {family}"));
        let address_local = family.map_or_else(
            || {
                given
                    .chars()
                    .filter(char::is_ascii_alphanumeric)
                    .collect::<String>()
                    .to_ascii_lowercase()
            },
            |family| {
                format!(
                    "{}.{}",
                    given.to_ascii_lowercase(),
                    family.to_ascii_lowercase()
                )
            },
        );
        Self {
            given: given.to_owned(),
            family: family.map(str::to_owned),
            full,
            address: format!("{address_local}@example.invalid"),
        }
    }
}

impl RequestAssignment {
    pub(crate) fn decode_text(&self, text: &str) -> String {
        let mut forms = Vec::new();
        for person in self.assigned.values() {
            let pseudonym = &person.pseudonym;
            forms.push(DecodeForm {
                pseudo: pseudonym.address.clone(),
                real: person.real_address.clone(),
                priority: 4,
            });
            forms.push(DecodeForm {
                pseudo: pseudonym.full.clone(),
                real: person.real_name.canonical_full.clone(),
                priority: 3,
            });
            forms.push(DecodeForm {
                pseudo: pseudonym.given.clone(),
                real: person.real_name.given.clone(),
                priority: 1,
            });
            if let (Some(pseudo_family), Some(real_family)) =
                (&pseudonym.family, &person.real_name.family)
            {
                forms.push(DecodeForm {
                    pseudo: pseudo_family.clone(),
                    real: real_family.clone(),
                    priority: 2,
                });
            }
        }
        forms.sort_by(|left, right| {
            right
                .pseudo
                .len()
                .cmp(&left.pseudo.len())
                .then_with(|| right.priority.cmp(&left.priority))
        });
        replace_decode_forms(text, &forms)
    }

    fn encode_text(&mut self, cipher: &NameCipher, text: &str) -> String {
        let mut output = String::with_capacity(text.len());
        let mut cursor = 0;
        while cursor < text.len() {
            if let Some((end, address)) = email_at(text, cursor) {
                if let Some(person_index) = cipher.by_address.get(&address).copied() {
                    let replacement = self.pseudonym(cipher, person_index).address.clone();
                    output.push_str(&replacement);
                } else if cipher.protected_addresses.contains(&address) {
                    output.push_str(&text[cursor..end]);
                } else {
                    output.push_str("address@example.invalid");
                }
                cursor = end;
            } else if let Some((end, person_index, kind)) = best_name_match(cipher, text, cursor) {
                let pseudonym = self.pseudonym(cipher, person_index).clone();
                let replacement = match kind {
                    MatchKind::Full => pseudonym.full,
                    MatchKind::Given => pseudonym.given,
                    MatchKind::Family => pseudonym.family.unwrap_or(pseudonym.given),
                };
                output.push_str(&replacement);
                cursor = end;
            } else {
                let character = text[cursor..]
                    .chars()
                    .next()
                    .expect("cursor is inside the input string");
                output.push(character);
                cursor += character.len_utf8();
            }
        }
        output
    }

    fn pseudonym(&mut self, cipher: &NameCipher, person_index: usize) -> &Pseudonym {
        if !self.assigned.contains_key(&person_index) {
            let person = &cipher.people[person_index];
            let Some(name) = person.names.first() else {
                unreachable!("known people always have a parsed display name")
            };
            let address = person.addresses.iter().next().cloned().unwrap_or_default();
            let pseudonym = self.next_pseudonym();
            self.assigned.insert(
                person_index,
                AssignedPerson {
                    pseudonym,
                    real_name: name.clone(),
                    real_address: address,
                },
            );
        }
        &self.assigned[&person_index].pseudonym
    }

    fn next_pseudonym(&mut self) -> Pseudonym {
        while self.next_pool_index < GIVEN_NAMES.len() {
            let index = self.next_pool_index;
            self.next_pool_index += 1;
            let given = GIVEN_NAMES[index];
            let family = FAMILY_NAMES[index];
            if self.skipped_tokens.contains(&given.to_ascii_lowercase())
                || self.skipped_tokens.contains(&family.to_ascii_lowercase())
            {
                continue;
            }
            return Pseudonym::new(given, Some(family));
        }
        let number = self.overflow_index;
        self.overflow_index += 1;
        Pseudonym::new(&format!("Person {number}"), None)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MatchKind {
    Full,
    Family,
    Given,
}

pub(crate) struct CipherDecisionClient<'a> {
    inner: &'a dyn DecisionClient,
    cipher: &'a NameCipher,
}

impl<'a> CipherDecisionClient<'a> {
    pub(crate) fn new(inner: &'a dyn DecisionClient, cipher: &'a NameCipher) -> Self {
        Self { inner, cipher }
    }
}

impl DecisionClient for CipherDecisionClient<'_> {
    fn model(&self) -> &str {
        self.inner.model()
    }

    fn max_parallel(&self) -> usize {
        self.inner.max_parallel()
    }

    fn decide(
        &self,
        state: &Value,
        questions: &Questions,
        cancel: Option<&AtomicBool>,
        deadline: Duration,
    ) -> Result<Answers, openloops_inference::provider::ProviderError> {
        let encoded = self.cipher.encode_request(state, questions);
        let answers = self
            .inner
            .decide(&encoded.state, &encoded.questions, cancel, deadline)?;
        Ok(answers.map_choice_text(|text| encoded.assignment.decode_text(text)))
    }
}

fn best_name_match(
    cipher: &NameCipher,
    text: &str,
    cursor: usize,
) -> Option<(usize, usize, MatchKind)> {
    let mut best: Option<(usize, usize, MatchKind)> = None;
    let first = text[cursor..].chars().next()?;
    for lookup in cipher.name_forms.get(&first).into_iter().flatten() {
        let name = &cipher.people[lookup.person_index].names[lookup.name_index];
        let form = match lookup.part {
            NamePart::Full(index) => &name.full_forms[index],
            NamePart::Family => name.family.as_deref().expect("indexed family name exists"),
            NamePart::Given => &name.given,
        };
        if !whole_word_match_at(text, cursor, form)
            || (matches!(lookup.part, NamePart::Given)
                && ENGLISH_GIVEN_NAMES.contains(&name.given.to_ascii_lowercase().as_str())
                && starts_sentence(text, cursor))
        {
            continue;
        }
        let candidate = (
            cursor + form.len(),
            lookup.person_index,
            lookup.part.match_kind(),
        );
        if is_better_match(candidate, best) {
            best = Some(candidate);
        }
    }
    best
}

fn is_better_match(
    candidate: (usize, usize, MatchKind),
    current: Option<(usize, usize, MatchKind)>,
) -> bool {
    let Some(current) = current else {
        return true;
    };
    let candidate_len = candidate.0;
    let current_len = current.0;
    candidate_len > current_len
        || (candidate_len == current_len
            && match (candidate.2, current.2) {
                (MatchKind::Full, MatchKind::Family | MatchKind::Given)
                | (MatchKind::Family, MatchKind::Given) => true,
                (MatchKind::Family, MatchKind::Full)
                | (MatchKind::Given, MatchKind::Full | MatchKind::Family) => false,
                (MatchKind::Full, MatchKind::Full)
                | (MatchKind::Family, MatchKind::Family)
                | (MatchKind::Given, MatchKind::Given) => candidate.1 < current.1,
            })
}

fn whole_word_match_at(text: &str, cursor: usize, term: &str) -> bool {
    if term.is_empty()
        || !text
            .get(cursor..)
            .is_some_and(|tail| tail.starts_with(term))
    {
        return false;
    }
    let end = cursor + term.len();
    let before = text[..cursor].chars().next_back();
    let after = text[end..].chars().next();
    before.is_none_or(|character| !is_word_character(character))
        && after.is_none_or(|character| !is_word_character(character))
}

fn is_word_character(character: char) -> bool {
    character.is_alphanumeric() || character == '_'
}

fn starts_sentence(text: &str, cursor: usize) -> bool {
    let prefix = &text[..cursor];
    let mut end = prefix.len();
    while let Some((start, character)) = prefix[..end].char_indices().next_back() {
        if character == '\n' || character == '\r' {
            return true;
        }
        if character.is_whitespace()
            || is_sentence_closer(character)
            || is_sentence_opener(character)
        {
            end = start;
            continue;
        }
        return ".!?:".contains(character);
    }
    true
}

fn is_sentence_closer(character: char) -> bool {
    matches!(character, ')' | ']' | '}' | '\'' | '"' | '”' | '’' | '»')
}

fn is_sentence_opener(character: char) -> bool {
    matches!(
        character,
        '(' | '[' | '{' | '\'' | '"' | '“' | '‘' | '«' | '—' | '–'
    )
}

fn exact_email_at(text: &str, cursor: usize, address: &str) -> bool {
    text.get(cursor..cursor.saturating_add(address.len()))
        .is_some_and(|candidate| candidate.eq_ignore_ascii_case(address))
        && email_boundary(text, cursor, cursor + address.len())
}

fn email_boundary(text: &str, start: usize, end: usize) -> bool {
    let is_email_character =
        |character: char| character.is_ascii_alphanumeric() || "._%+-@".contains(character);
    text[..start]
        .chars()
        .next_back()
        .is_none_or(|character| !is_email_character(character))
        && text[end..]
            .chars()
            .next()
            .is_none_or(|character| !is_email_character(character))
}

fn email_at(text: &str, cursor: usize) -> Option<(usize, String)> {
    let is_email_character =
        |character: char| character.is_ascii_alphanumeric() || "._%+-@".contains(character);
    if text[..cursor]
        .chars()
        .next_back()
        .is_some_and(is_email_character)
    {
        return None;
    }
    let bytes = text.as_bytes();
    let mut index = cursor;
    while bytes
        .get(index)
        .is_some_and(|byte| byte.is_ascii_alphanumeric() || b"._%+-".contains(byte))
    {
        index += 1;
    }
    if index == cursor || bytes.get(index) != Some(&b'@') {
        return None;
    }
    index += 1;
    let domain_start = index;
    while bytes
        .get(index)
        .is_some_and(|byte| byte.is_ascii_alphanumeric() || b".-".contains(byte))
    {
        index += 1;
    }
    while index > domain_start && matches!(bytes[index - 1], b'.' | b'-') {
        index -= 1;
    }
    let domain = text.get(domain_start..index)?;
    if !domain.contains('.') || domain.starts_with('.') || domain.ends_with('.') {
        return None;
    }
    if text[index..]
        .chars()
        .next()
        .is_some_and(|character| character.is_ascii_alphanumeric() || "_%+@".contains(character))
    {
        return None;
    }
    Some((index, text[cursor..index].to_ascii_lowercase()))
}

fn participant_parts(label: &str) -> (Option<&str>, Option<&str>) {
    if let Some((display, rest)) = label.split_once('<')
        && let Some((address, _)) = rest.split_once('>')
    {
        return (Some(display.trim()), Some(address.trim()));
    }
    let address = label.split_whitespace().find(|part| part.contains('@'));
    (None, address)
}

fn parse_person_name(display: &str) -> Option<PersonName> {
    let original = clean_display_name(display);
    if original.is_empty() || is_system_display_name(&original) {
        return None;
    }
    let (canonical_full, source_form) = if let Some((family, given)) = original.split_once(',') {
        let family = family.trim();
        let given = given.trim();
        (format!("{given} {family}"), Some(original.clone()))
    } else {
        (original.clone(), None)
    };
    // Initials and lower-case particles stay in the full forms but are not
    // the given or family name: "Ana Q. de la Cruz" is given "Ana", family
    // "Cruz".
    let mut words = Vec::new();
    for (index, word) in canonical_full.split_whitespace().enumerate() {
        if index == 0 && is_initial(word) {
            // "J. Smith": the initial stands for the given name.
            words.push(word);
            continue;
        }
        if is_initial(word) || (index > 0 && NAME_PARTICLES.contains(&word)) {
            continue;
        }
        if !valid_name_word(word) {
            return None;
        }
        words.push(word);
    }
    if !(1..=4).contains(&words.len()) {
        return None;
    }
    let given = words[0].to_owned();
    let family = (words.len() > 1).then(|| words[words.len() - 1].to_owned());
    let mut full_forms = vec![canonical_full.clone()];
    if let Some(family) = &family {
        full_forms.push(format!("{given} {family}"));
    }
    if let Some(source_form) = source_form {
        full_forms.push(source_form);
    }
    Some(PersonName {
        canonical_full,
        full_forms,
        given,
        family,
    })
}

/// Drops surrounding quotes, bracketed parts such as "(Acme)", and name
/// suffixes such as "Jr." or "III".
fn clean_display_name(display: &str) -> String {
    let mut unbracketed = String::with_capacity(display.len());
    let mut depth = 0_usize;
    for character in display.chars() {
        match character {
            '(' | '[' => depth += 1,
            ')' | ']' => depth = depth.saturating_sub(1),
            _ if depth == 0 => unbracketed.push(character),
            _ => {}
        }
    }
    let unquoted = unbracketed
        .trim()
        .trim_matches(|character| matches!(character, '"' | '\'' | '“' | '”'));
    unquoted
        .split_whitespace()
        .filter(|word| {
            let bare = word.trim_matches(|character| matches!(character, ',' | '.'));
            !NAME_SUFFIXES.contains(&bare.to_ascii_lowercase().as_str())
        })
        .collect::<Vec<_>>()
        .join(" ")
        .trim_end_matches(',')
        .trim()
        .to_owned()
}

fn is_initial(word: &str) -> bool {
    let bare = word.trim_end_matches('.');
    let mut characters = bare.chars();
    let single = characters
        .next()
        .is_some_and(|first| first.is_alphabetic() && first.is_uppercase())
        && characters.next().is_none();
    // "J.R." style initials.
    let dotted = word.contains('.')
        && word.chars().all(|character| {
            character == '.' || (character.is_alphabetic() && character.is_uppercase())
        });
    single || dotted
}

fn valid_name_word(word: &str) -> bool {
    let mut characters = word.chars();
    characters
        .next()
        .is_some_and(|first| first.is_uppercase() && first.is_alphabetic())
        && characters
            .all(|character| character.is_alphabetic() || matches!(character, '-' | '\'' | '’'))
}

fn parse_single_name_word(value: &str) -> Option<String> {
    let value = value.trim();
    (valid_name_word(value) && !SYSTEM_WORDS.contains(&value.to_ascii_lowercase().as_str()))
        .then(|| value.to_owned())
}

fn is_system_display_name(display: &str) -> bool {
    display
        .split(|character: char| !character.is_alphanumeric())
        .any(|word| SYSTEM_WORDS.contains(&word.to_ascii_lowercase().as_str()))
}

fn is_system_address(address: &str) -> bool {
    address
        .split_once('@')
        .is_some_and(|(local, _)| SYSTEM_LOCALS.contains(&local.to_ascii_lowercase().as_str()))
}

fn collect_state_texts(value: &Value, texts: &mut Vec<String>) {
    match value {
        Value::String(text) => texts.push(text.clone()),
        Value::Array(values) => values
            .iter()
            .for_each(|value| collect_state_texts(value, texts)),
        Value::Object(values) => values
            .values()
            .for_each(|value| collect_state_texts(value, texts)),
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

fn map_value_strings(value: &Value, map: &mut impl FnMut(&str) -> String) -> Value {
    match value {
        Value::String(text) => Value::String(map(text)),
        Value::Array(values) => Value::Array(
            values
                .iter()
                .map(|value| map_value_strings(value, map))
                .collect(),
        ),
        Value::Object(values) => Value::Object(
            values
                .iter()
                .map(|(key, value)| (key.clone(), map_value_strings(value, map)))
                .collect(),
        ),
        Value::Null | Value::Bool(_) | Value::Number(_) => value.clone(),
    }
}

fn request_name_tokens(texts: &[String]) -> BTreeSet<String> {
    texts
        .iter()
        .flat_map(|text| text.split(|character: char| !character.is_alphanumeric()))
        .filter(|token| !token.is_empty())
        .map(str::to_ascii_lowercase)
        .collect()
}

fn replace_decode_forms(text: &str, forms: &[DecodeForm]) -> String {
    let mut output = String::with_capacity(text.len());
    let mut cursor = 0;
    while cursor < text.len() {
        if let Some(form) = forms.iter().find(|form| {
            if form.pseudo.contains('@') {
                exact_email_at(text, cursor, &form.pseudo)
            } else {
                whole_word_match_at(text, cursor, &form.pseudo)
            }
        }) {
            output.push_str(&form.real);
            cursor += form.pseudo.len();
        } else {
            let character = text[cursor..]
                .chars()
                .next()
                .expect("cursor is inside the input string");
            output.push(character);
            cursor += character.len_utf8();
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use openloops_inference::decision::{
        Answer, DECISION_DEADLINE, DecisionClient, Questions, parse_answers, request_body,
    };
    use openloops_inference::provider::ProviderError;
    use serde_json::{Value, json};
    use std::collections::BTreeMap;
    use std::sync::atomic::AtomicBool;
    use std::sync::{Mutex, PoisonError};
    use std::time::Duration;

    fn table(entries: &[(&str, &str)]) -> NameCipher {
        NameCipher::from_test_people(
            entries
                .iter()
                .map(|(name, address)| ((*name).to_owned(), (*address).to_owned())),
        )
    }

    fn encoded_text(cipher: &NameCipher, text: &str) -> String {
        cipher
            .encode_request(&json!({"text": text}), &Questions::new())
            .state["text"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    #[test]
    fn replacement_uses_whole_words_case_and_keeps_possessive_suffixes() {
        let cipher = table(&[("Maren Qorrel", "maren.qorrel@example.invalid")]);
        let encoded = encoded_text(
            &cipher,
            "Maren's note mentions Maren and Qorrel, but unmaren, maren, and Qorrell stay.",
        );
        assert_eq!(
            encoded,
            "Priya's note mentions Priya and Venkataraman, but unmaren, maren, and Qorrell stay."
        );
    }

    #[test]
    fn last_first_display_names_are_replaced_in_both_orders() {
        let cipher = table(&[("Qorrel, Maren", "maren.qorrel@example.invalid")]);
        assert_eq!(
            encoded_text(&cipher, "Qorrel, Maren emailed Maren Qorrel."),
            "Priya Venkataraman emailed Priya Venkataraman."
        );
    }

    #[test]
    fn common_english_given_names_are_kept_at_sentence_start_but_replaced_mid_sentence() {
        let cipher = table(&[("Will Fenner", "will.fenner@example.invalid")]);
        assert_eq!(
            encoded_text(&cipher, "Will asked for help. Then Will called."),
            "Will asked for help. Then Priya called."
        );
    }

    #[test]
    fn common_english_given_names_are_kept_at_quoted_and_line_start_sentences() {
        let cipher = table(&[("Will Fenner", "will.fenner@example.invalid")]);
        assert_eq!(
            encoded_text(&cipher, "A note ended.\n“Will replied.”"),
            "A note ended.\n“Will replied.”"
        );
    }

    #[test]
    fn display_names_with_initials_particles_suffixes_quotes_or_brackets_are_people() {
        let cipher = table(&[
            ("Anwen Q. Ruzek", "anwen@example.invalid"),
            ("\"Bexley, Corin\"", "corin@example.invalid"),
            ("Dario Feltz (Halcyon Ridge LLC)", "dario@example.invalid"),
            ("Maribel de la Vantor", "maribel@example.invalid"),
            ("Quill Hastrow III", "quill@example.invalid"),
            ("Orrin Tealby, Esq.", "orrin@example.invalid"),
            ("J. Tarnow", "jt@example.invalid"),
        ]);
        let encoded = encoded_text(
            &cipher,
            "Anwen Ruzek, Anwen Q. Ruzek, Corin Bexley, Bexley, Corin, Dario, \
             Maribel de la Vantor, Vantor, Quill Hastrow, Hastrow, Orrin, Tealby, J. Tarnow and Tarnow wrote.",
        );
        for real in [
            "Anwen", "Ruzek", "Corin", "Bexley", "Dario", "Feltz", "Maribel", "Vantor", "Quill",
            "Hastrow", "Orrin", "Tealby", "Tarnow",
        ] {
            assert!(!encoded.contains(real), "{real} remains in {encoded}");
        }
    }

    #[test]
    fn a_given_name_hint_adds_a_name_and_keeps_the_display_given_name() {
        let mut cipher = NameCipher::default();
        cipher.add_participant(
            "Elsabet Morrow",
            Some("elsabet@example.invalid"),
            Some("Elsie"),
        );
        cipher.rebuild_name_forms();
        let encoded = encoded_text(&cipher, "Elsabet, Elsie and Elsie Morrow agreed.");
        assert!(!encoded.contains("Elsabet"), "{encoded}");
        assert!(!encoded.contains("Elsie"), "{encoded}");
        assert!(!encoded.contains("Morrow"), "{encoded}");
        assert_eq!(encoded, "Priya, Priya and Priya Venkataraman agreed.");
    }

    #[test]
    fn single_word_people_still_receive_a_given_and_family_pseudonym() {
        let cipher = table(&[("Maren", "maren@example.invalid")]);
        assert_eq!(
            encoded_text(&cipher, "Maren <maren@example.invalid> replied."),
            "Priya Venkataraman <priya.venkataraman@example.invalid> replied."
        );
    }

    #[test]
    fn short_name_tokens_are_replaced_only_inside_the_full_name() {
        let cipher = table(&[("A Smith", "a.smith@example.invalid")]);
        assert_eq!(
            encoded_text(&cipher, "A Smith wrote to A about Smith."),
            "Priya Venkataraman wrote to A about Venkataraman."
        );
    }

    #[test]
    fn organization_and_system_senders_keep_their_names_and_addresses() {
        let cipher = table(&[
            ("Acme Support", "support@acme.example.invalid"),
            ("Reminder Agent", "noreply@reminder.example.invalid"),
        ]);
        let encoded = encoded_text(
            &cipher,
            "Acme Support <support@acme.example.invalid>; Reminder Agent <noreply@reminder.example.invalid>.",
        );
        assert_eq!(
            encoded,
            "Acme Support <support@acme.example.invalid>; Reminder Agent <noreply@reminder.example.invalid>."
        );
    }

    #[test]
    fn pseudonym_pool_names_already_in_request_text_are_skipped() {
        let cipher = table(&[("Maren Qorrel", "maren.qorrel@example.invalid")]);
        let encoded = encoded_text(
            &cipher,
            "The quoted note already contains the token Priya. Maren replied.",
        );
        assert_eq!(
            encoded,
            "The quoted note already contains the token Priya. Tomas replied."
        );
    }

    #[test]
    fn assignments_past_the_pool_use_person_numbers() {
        let people = (0..25)
            .map(|index| {
                let suffix = char::from(b'A' + u8::try_from(index).unwrap());
                (
                    format!("Given{suffix} Family{suffix}"),
                    format!("person{index}@example.invalid"),
                )
            })
            .collect::<Vec<_>>();
        let cipher = NameCipher::from_test_people(people);
        let text = (0..25)
            .map(|index| {
                let suffix = char::from(b'A' + u8::try_from(index).unwrap());
                format!("Given{suffix} Family{suffix}")
            })
            .collect::<Vec<_>>()
            .join("; ");
        let encoded = encoded_text(&cipher, &text);
        assert!(encoded.contains("Person 25"));
    }

    #[test]
    fn known_addresses_become_the_pseudonym_address_and_other_addresses_are_generic() {
        let cipher = table(&[("Maren Qorrel", "maren.qorrel@example.invalid")]);
        assert_eq!(
            encoded_text(
                &cipher,
                "Maren <maren.qorrel@example.invalid> copied outsider@elsewhere.invalid.",
            ),
            "Priya <priya.venkataraman@example.invalid> copied address@example.invalid."
        );
    }

    #[test]
    fn request_assignments_are_deterministic_for_the_same_text() {
        let cipher = table(&[
            ("Maren Qorrel", "maren.qorrel@example.invalid"),
            ("Tobin Wexler", "tobin.wexler@example.invalid"),
        ]);
        let state = json!({"first":"Tobin Wexler","later":"Maren Qorrel"});
        let first = cipher.encode_request(&state, &Questions::new());
        let second = cipher.encode_request(&state, &Questions::new());
        assert_eq!(first.state, second.state);
    }

    #[test]
    fn decode_restores_choice_values_and_probability_keys_but_not_handles() {
        let cipher = table(&[("Maren Qorrel", "maren.qorrel@example.invalid")]);
        let encoded = cipher.encode_request(&json!({"text":"Maren Qorrel"}), &Questions::new());
        let decoded = encoded
            .assignment
            .decode_text("Priya Venkataraman and none p0 2026-10-21");
        assert_eq!(decoded, "Maren Qorrel and none p0 2026-10-21");
    }

    struct RecordingDecision {
        requests: Mutex<Vec<Value>>,
        cancel_ptr: Mutex<Option<usize>>,
        deadline: Mutex<Option<Duration>>,
        fail: bool,
    }

    impl RecordingDecision {
        fn new(fail: bool) -> Self {
            Self {
                requests: Mutex::new(Vec::new()),
                cancel_ptr: Mutex::new(None),
                deadline: Mutex::new(None),
                fail,
            }
        }
    }

    impl DecisionClient for RecordingDecision {
        fn model(&self) -> &'static str {
            "synthetic/decision"
        }

        fn max_parallel(&self) -> usize {
            3
        }

        fn decide(
            &self,
            state: &Value,
            questions: &Questions,
            cancel: Option<&AtomicBool>,
            deadline: Duration,
        ) -> Result<openloops_inference::decision::Answers, ProviderError> {
            *self
                .cancel_ptr
                .lock()
                .unwrap_or_else(PoisonError::into_inner) =
                cancel.map(|value| std::ptr::from_ref(value) as usize);
            *self.deadline.lock().unwrap_or_else(PoisonError::into_inner) = Some(deadline);
            let body = request_body(self.model(), state, questions, false)?;
            let request: Value =
                serde_json::from_slice(&body).map_err(|_| ProviderError::InvalidQuestion)?;
            self.requests
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(request);
            if self.fail {
                return Err(ProviderError::Cancelled);
            }
            let response = json!({
                "model":"synthetic/decision",
                "answers":{
                    "person":{
                        "type":"choice",
                        "choice":"p0",
                        "probabilities":{"p0":0.8,"none":0.1,"2026-10-21":0.1},
                        "confidence":0.8
                    }
                },
                "usage":{"input_tokens":7}
            });
            parse_answers(
                &serde_json::to_vec(&response).map_err(|_| ProviderError::InvalidResponse)?,
                self.model(),
                questions,
            )
        }
    }

    #[test]
    fn wrapper_ciphers_state_and_option_descriptions_without_changing_handles() {
        let cipher = table(&[("Maren Qorrel", "maren.qorrel@example.invalid")]);
        let inner = RecordingDecision::new(false);
        let wrapper = CipherDecisionClient::new(&inner, &cipher);
        let questions = Questions::new()
            .choice(
                "person",
                "Choose the participant.",
                &[
                    ("p0", "Maren Qorrel <maren.qorrel@example.invalid>"),
                    ("none", "No participant"),
                    ("2026-10-21", "Maren's date"),
                ],
            )
            .unwrap();
        let state = json!({"body":"Maren Qorrel wrote.","address":"maren.qorrel@example.invalid"});
        let cancel = AtomicBool::new(false);
        let answers = wrapper
            .decide(&state, &questions, Some(&cancel), DECISION_DEADLINE)
            .unwrap();
        let request = &inner
            .requests
            .lock()
            .unwrap_or_else(PoisonError::into_inner)[0];
        let serialized = request.to_string();
        assert!(!serialized.contains("Maren"));
        assert!(!serialized.contains("Qorrel"));
        assert!(!serialized.contains("maren.qorrel@"));
        assert!(
            request["questions"]["person"]["criteria"]["p0"]
                .as_str()
                .unwrap()
                .starts_with("Priya Venkataraman ")
        );
        assert!(
            request["questions"]["person"]["criteria"]
                .get("none")
                .is_some()
        );
        assert!(
            request["questions"]["person"]["criteria"]
                .get("2026-10-21")
                .is_some()
        );
        assert_eq!(
            answers.get("person"),
            Some(&Answer::Choice {
                choice: "p0".into(),
                probabilities: BTreeMap::from([
                    ("p0".into(), 0.8),
                    ("none".into(), 0.1),
                    ("2026-10-21".into(), 0.1),
                ]),
                confidence: 0.8,
            })
        );
        assert_eq!(
            *inner
                .cancel_ptr
                .lock()
                .unwrap_or_else(PoisonError::into_inner),
            Some(std::ptr::from_ref(&cancel) as usize)
        );
        assert_eq!(
            *inner
                .deadline
                .lock()
                .unwrap_or_else(PoisonError::into_inner),
            Some(DECISION_DEADLINE)
        );
        assert_eq!(wrapper.model(), inner.model());
        assert_eq!(wrapper.max_parallel(), inner.max_parallel());
    }

    #[test]
    fn wrapper_returns_inner_errors_unchanged() {
        let cipher = table(&[("Maren Qorrel", "maren.qorrel@example.invalid")]);
        let inner = RecordingDecision::new(true);
        let wrapper = CipherDecisionClient::new(&inner, &cipher);
        let questions = Questions::new().noul("ask", "Does the text ask?").unwrap();
        assert!(matches!(
            wrapper.decide(
                &json!({"text":"Maren Qorrel"}),
                &questions,
                None,
                Duration::from_secs(9)
            ),
            Err(ProviderError::Cancelled)
        ));
    }
}
