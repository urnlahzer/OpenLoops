# Working preview implementation plan

1. **Mail context:** extend the live Graph boundary to fetch authenticated
   identity, Inbox and Sent Items metadata and bounded history, group thread
   posts, and source-specific coverage. Preserve immutable IDs in memory.
2. **Expectation extraction:** add a versioned live conversation contract beside
   the legacy low-level extractor. Supply numbered text blocks and participant
   handles. Resolve exact quotation anchors and validate ownership and temporal
   relationships locally. Retain action/owner/waiting party/deadline/closure.
3. **Review:** replace fragment cards with action-first results, ownership and
   status, expandable chronological evidence, and explicit user decisions.
   Separate unresolved team ownership and possible completion from active work.
4. **Durability:** store only bounded keyed evidence fingerprints, decisions,
   timestamps, and reminder outcome codes in a dedicated versioned credential.
   Bind fingerprints to the account and stable source evidence. Never silently
   overwrite corrupt or oversized storage. No readable mailbox content on disk.
5. **Reminders:** create personal To Do tasks only from exact UI-reviewed title
   and reminder time. Bind the authenticated account to the scan account and
   persist an attempt marker before dispatch. Uncertain outcomes remain blocked
   for inspection rather than retried. No mail sending or implicit task closure.
6. **Verification:** targeted Graph projection/coverage tests, evidence and
   semantic contract tests, persistence/replay tests, selected-cloud-model
   synthetic evaluation, native UI build, lint and public-repository gate.

The new live path is deliberately separate from the disabled production
composition. It does not turn Phase 0 release gates green or silently activate
production automatic mutations. Existing domain policy remains authoritative
for the eventual durable production runtime. This preview's credential-backed
decision projection is specified in the roadmap and has no generic payload bag.
