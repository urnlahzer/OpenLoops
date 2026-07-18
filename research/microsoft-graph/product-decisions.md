# Microsoft Graph product decisions

**Status:** Open

**Owner:** OpenLoops project

**Research snapshot:** 2026-07-18

Update this document when decisions are made. A research recommendation is not
a decision until the `Decision` and `Rationale` columns are completed.

| # | Product question | Research default | Decision | Rationale |
|---:|---|---|---|---|
| 1 | Must OpenLoops read email bodies, or can it operate from metadata? | Bodies likely required to infer commitments; start permission tests with `Mail.ReadBasic`, then add `Mail.Read` explicitly | Open | Open |
| 2 | Which mail folders are in scope? | Inbox and Sent Items, with other folders opt-in | Open | Open |
| 3 | How much history is inspected on first connection? | Bounded, visible 30-day default | Open | Open |
| 4 | Does OpenLoops create/update/delete Microsoft To Do tasks? | User-confirmed create/update; no autonomous delete | Open | Open |
| 5 | Must every task creation be confirmed by the user? | Yes for the first release | Open | Open |
| 6 | May OpenLoops persist human-readable candidate loops awaiting review? | No persistent review queue by default | Open | Open |
| 7 | May OpenLoops persist derived summaries, embeddings, prompts, or classifications? | No; they are derived Microsoft 365 content | Open | Open |
| 8 | Will Microsoft 365 content be sent to an external model or service? | No initially; local/transient processing only | Open | Open |
| 9 | Is background synchronization required while the UI is closed? | Per-user background process only while the user is logged in | Open | Open |
| 10 | Must personal Microsoft accounts work at launch? | Yes, subject to contract tests | Open | Open |
| 11 | Is a Windows-first release acceptable? | Yes; keep cross-platform interfaces internally | Open | Open |
| 12 | Are macOS and Linux launch requirements? | macOS later; Linux only with a supported Secret Service and no plaintext fallback | Open | Open |
| 13 | Are shared mailboxes or shared To Do lists launch requirements? | No | Open | Open |
| 14 | Are multiple Microsoft accounts per OS user required? | No for the first release | Open | Open |
| 15 | Are Docker, NAS, remote-browser, or headless deployments launch requirements? | No; separate future threat model | Open | Open |
| 16 | Is near-real-time notification worth a webhook or Azure dependency? | No; polling first | Open | Open |
| 17 | Will the project operate a central Entra registration? | Eventually, after publisher/governance gates; BYO during development | Open | Open |
| 18 | Can the project maintain a publisher domain and Microsoft publisher verification? | Required before broad use of a shared registration | Open | Open |
| 19 | Are national/sovereign Microsoft clouds in scope? | No for the first release | Open | Open |
| 20 | Is any telemetry acceptable? | None by default; explicit redacted diagnostics only | Open | Open |

## Decisions that alter the privacy boundary

The security and privacy documents must be revised before implementing any
decision that enables:

- Persistent email/task content or a review queue.
- Derived summaries, embeddings, prompts, or classification history.
- External model or telemetry transmission.
- Automatic actions without user confirmation.
- A public webhook, hosted relay, remote UI, NAS, container, or multi-user mode.
- Application permissions or tenant-wide access.
