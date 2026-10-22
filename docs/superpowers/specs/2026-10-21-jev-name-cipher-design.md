# Jev name cipher: design

Date: 2026-10-21. Owner request: "use a cipher and obscure names on the way up
and down from Jev".

## Why

Requests to the decision endpoint (Jev, `typesafe/jev-1.13` on OpenRouter)
carry the names of real people:

- in body text and subjects, in every pass;
- in explicit fields: extraction `user.display_name`, `user.given_name` and
  `participants[].text`; recap `sender`; closure walk `user.name` and
  `waiting_party`; `walk.handoff_to` option descriptions.

The questions need roles and relations, not real names. The cipher replaces
each known person's name and address with an invented one before a request
leaves the app, and maps answers back.

## Scope

- Every request that the desktop app sends to the decision endpoint: recap,
  triage, extraction, rule questions, event match, scoped event, thread
  merge, the compare probe, and (after the closure-walk branch merges) the
  walk, date, hand-off and expiry requests.
- The owner training export (`triage.jsonl`, `closure.jsonl`, `rules.jsonl`,
  later `walk.jsonl`). The harness sends these rows to Jev during
  `evaluate` and `optimize`, so they carry the same cipher.
- Out of scope: the chat model (escalation and extraction chat requests).
  A later change can reuse the same cipher there.

## Known people

Code builds one table per scan (and per export, and per compare run) from
the messages it reads:

- each message's sender, To and Cc: display name and address;
- the user: `own_display_name`, `own_given_name`, `own_addresses`.

One person is one lowercase address; the display names seen for that
address all belong to that person. Code first removes surrounding quotes,
bracketed parts such as "(Acme)", and suffixes such as "Jr.", "III" or
"Esq.". Initials ("Q.") and lower-case particles ("de", "van") after the
first word stay in the full name but are not the given or family name, so
"Ana Q. de la Cruz" has given name "Ana" and family name "Cruz". A leading
initial ("J. Smith") is the given name. When the user's given name differs
from the first word of their display name ("Elsie" for "Elsabet Morrow"),
both are matched as given names. A display name counts as a person's name
only when it looks like one:

- one to four name words, each starting with an upper-case letter and made
  of letters, hyphens or apostrophes;
- no word from a fixed organization and system list (for example `Inc`,
  `LLC`, `Ltd`, `Team`, `Support`, `Billing`, `Notifications`, `Service`,
  `Admin`, `Sales`, `Help`, `Desk`, `Calendar`, `Bot`, `Reply`);
- the address local part is not a system name (`noreply`, `no-reply`,
  `donotreply`, `notifications`, `mailer-daemon`, `postmaster`).

`Last, First` becomes `First Last`. System senders keep their names and
addresses, so later sender-kind questions still see them.

## Pseudonyms per request

Each request gets its own assignment. Code finds which known people occur
in the request's strings (state values and choice option descriptions) and
gives them pseudonyms in order of first occurrence in the serialized state.
The same input always gives the same assignment, so replay caches stay
valid.

- Pseudonyms come from fixed lists of invented given and family names. The
  first eight pairs are the harness's `SYNTHETIC_PEOPLE`, so runtime text
  looks like training text.
- A pool name equal to any real name token in the request is skipped.
- Past the end of the lists, people get `Person 25`, `Person 26`, and so on.

## Replacement

In every string value of the state (recursively) and every choice option
description. Registry instructions are fixed text and stay unchanged.

- Forms, longest first: full name, family name, given name, address.
- Whole words only, case-sensitive, so `will` in "I will send" is never a
  name. A possessive `'s` stays attached.
- A given name that is also an English word (fixed list: `Will`, `May`,
  `Mark`, `Bill`, `Grant`, `Hope`, `June`, `April`, `August`, `Rose`,
  `Chase`, `Drew`, `Dawn`, `Art`, `Pat`, `Sue`, `Rob`, `Joy`, `Faith`,
  `Summer`, `Frank`, `Gene`, `Jack`, `Ray`, `Guy`, `Dean`, `Penny`, `Ruby`,
  `Grace`, `Holly`, `Iris`, `Hunter`, `Carter`, `Parker`, `Mason`) is not
  replaced when it is the first word of a sentence.
- Given or family names shorter than two letters are replaced only inside
  the full name.
- A known person's address becomes `given.family@example.invalid` with the
  pseudonym's names in lower case. Any other email address in the text
  becomes `address@example.invalid`.

## The way down

Answers carry probabilities, option handles and `none`. The cipher maps any
pseudonym in a choice value or a probability key back to the real text, so
callers never see a pseudonym. The table and the assignments stay in
memory for one scan. They are never logged, persisted, exported or sent.

## Where it runs

A wrapper type implements `DecisionClient` around the real client and
applies the cipher to every request. `scan` wraps the decision client once,
right after it creates it; the compare probe and the export do the same.
The passes keep their code.

## Known limits

- People named only in the text (not a sender or recipient) keep their
  names.
- Nicknames and spellings that differ from the display name ("Liz" for
  "Elizabeth") stay.
- Organization names stay.
- A pseudonym can differ in gender from the pronouns in the text.

## Testing

- Unit tests: whole words, case, possessives, `Last, First`, the
  English-word list at sentence start and mid-sentence, short names,
  organization and system senders untouched, pool collisions, `Person N`
  past the lists, addresses, deterministic assignment, decode of choice
  values and probability keys.
- A full scan through a recording fake client whose answers do not depend
  on names: no request body contains any fixture person's real given name,
  family name or address, and the scan result is the same as without the
  cipher.
- The export writes no fixture person's real name or address.
