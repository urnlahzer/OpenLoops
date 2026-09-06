# OpenLoops build-run handoff

**Run:** unattended MVP build, 2026-07-21 → 07-22. **Branch:** `claude/mvp-build` (from `codex/phase0-reminder-checkpoint`). **Nothing pushed** — review and push when ready.

## What got done

Fifteen commits, each maker → deterministic-verifier → fresh-context Opus adversarial panel → release-judge → staged public-repo scan → commit. Every work item's tests, 18 deterministic checkers, and ~700 synthetic mutation cases stay green at each commit.

### Phase 0 completed (was ~85%)
- **P0-WI-12** — repaired the blocking retry contradiction in ADR-009 (duplicate invocation replays with zero requests vs. serialized eligible retry, ≤3 attempts).
- **ADR-010** add-in bridge, **ADR-011** automation/evaluation, **ADR-012** distribution/registration, **ADR-013** self-email — all four missing decision contracts authored and accepted, each with a machine-readable manifest, deterministic checker, mutation suite, and detailed threat model. **All 14 ADRs now accepted.**
- **Phase 0 exit review** — the last two threat models (Graph mail content; disconnect/uninstall) authored so all nine routing rows are covered; full exit audit passed. One honest finding recorded, not rewritten: P0-WI-08's closure note says 161 mutation cases; the suite now measures 163 (later work items appended cases to a shared suite).

### Phases 1–7 buildable subset (new: ~24,100 lines of Rust + the add-in, 448 workspace tests)
- **openloops-domain** — deterministic policy/state engine: facet catalogs, the 420-tuple legality model (41 legal projections derived from rules), promotion table, deadline precision/aging, command idempotency. Safety invariants are API-enforced (a system actor cannot terminalize a loop; reminder facets cannot reach obligation state).
- **openloops-contracts** — strict analysis-output parser with duplicate-member rejection at every depth and missing-vs-null discipline.
- **openloops-persistence** — the ADR-005 crypto engine: AES-256-GCM envelope + 78-byte AAD, nonce-reservation ledger, HMAC digest suite (KAT-verified), DPAPI in one audited unsafe module, SQLite WAL/FULL store, rollback anchor with a crash-window matrix. Seven crates activated at contract-exact pins.
- **openloops-inference** — byte-deterministic canonicalizer (ADR-006 DOM walker + conformance vectors) and the ADR-007 validation pipeline (steps 4–10; every claim type routes review-required).
- **openloops-domain deadline parser** — deterministic temporal parsing with typed DST ambiguity, no ambient clock.
- **openloops-graph** — ADR-002 PKCE transaction machine + loopback listener (with exact Host-authority validation) and the ADR-004 typed transport (Retry-After, backoff, single-flight, redirect rejection); ADR-009 To Do adapter shape (marker derivation, reconciliation classification, field-ownership/TOCTOU engine).
- **openloops-application** — ingestion page-atomic transaction, quarantine lane, the ADR-009 operation ledger with the repaired retry semantics, tick-driven scheduler.
- **outlook-addin** — review queue, card projections, empathetic copy templates (banned-phrase tested), no browser storage.
- **openloops-desktop** — composition probe linking every layer behind disabled capability flags; smoke marker preserved.
- **openloops-eval** — 33-fixture public synthetic corpus + a runner over the real deterministic pipeline (development tooling, explicitly not a gate).
- **openloops-harness** — the 37-row disposable-tenant experiment matrix, sanitized records, fails closed to skip-all without credentials.

## Guardrails held the whole run
No capability flag flipped; no G-gate marked passed; no AC/scenario completed; no real Microsoft/network contact; no credential accepted; nothing pushed; the public-repo scan passed before every commit. Where a Phase 0 checker's zero-runtime scan became structurally impossible once code existed, it was **re-scoped to a confinement assertion** (crypto/persistence APIs allowed only in the persistence crate; exact-version pins), never deleted — verified by the adversarial panel.

## Needs YOU (parked — none simulated)

1. **Live Graph gates** — G-ID, G-MAIL, G-TODO, G-CAL, G-ADDIN. The `openloops-harness` crate is ready: supply a BYO Entra public-client registration, a disposable tenant, and synthetic test accounts via its env/file config, then run outside CI. Real transport impls are deliberately absent (a `Wire`/`ExchangeTransport` trait seam) pending these.
2. **G-SELFMAIL**, **G-STATE/G-PRIV** live runs, **G-AUTO/G-AUTO-FULL** — the latter need the **sealed judge corpus**, which is owner-held and must stay out of the repo and out of any maker context.
3. **G-SEC-AUDIT** — independent security review (cannot be self-performed).
4. **G-RELEASE** — signing, packaging, installer, SBOM/provenance.
5. **Offline-registry follow-ups** (compile-ready when you have network): activate the six contract-pinned graph crates (oauth2/reqwest/tokio/url/webbrowser/httparse — their transitive closures are absent from this machine's registry mirror, so the listener uses `std::net` and the transport runs over a trait seam); activate `unicode-normalization` + `html5ever` to replace the canonicalizer's disclosed hand-rolled NFC/HTML approximations (byte-deterministic today, `anchor_version=1` reserved for the swap); activate a vetted tz crate (`jiff`/`chrono-tz`) for the deadline engine.
6. **Push** — `git push -u origin claude/mvp-build` when you've reviewed.

## Notes on the process
Two work items (IMPL-01, IMPL-06) reported "blocked" purely because my expected-file lists were too narrow or cargo wasn't on the verifier's PATH — the code was sound; I completed them as release judge. The adversarial panel earned its keep three times, catching real defects the maker's own tests missed: a skipped contract-required Host-authority check justified by a misquoted contract (IMPL-06), a digest layout prematurely closed under one owner when the contract requires two (IMPL-07), and an aging loop state representable without a resolved deadline boundary (IMPL-01). All were fixed before commit.

Toolchain note for your shell: rustc/cargo 1.97.1 are installed but not on the default PATH — prepend `%USERPROFILE%\.cargo\bin`.
