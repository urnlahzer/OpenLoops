# Policy and state boundary threat model

## Boundary

P0-WI-11 accepts ADR-008's deterministic decision contract only. No policy
runtime, record persistence, model call, reminder adapter, Graph/Office request,
permission, automation mode, capability, acceptance result, or gate is active.

## Threats and controls

| Threat | Fail-closed control |
|---|---|
| Model output terminalizes a loop | Models produce hypotheses only. Terminal state requires one closed typed user command and its evidence/manual precondition. |
| One facet overwrites another | Store independent closed facets and validate the full legal combination; UI label precedence never mutates secondary facets. |
| Multiple closure hypotheses collapse into one lossy enum | Derive the loop projection from current validated relations and exact keep-open transitions; persist no model text or generic hypothesis bag. |
| Keep-open suppresses later or sibling evidence | Identity includes target loop, kind, ordered versioned source refs, and policy version. Only the exact identity is suppressed. |
| Reminder completion/deletion closes an obligation | Reminder events are monotonic within their facet. Candidate/open/terminal obligation state is unchanged absent an explicit lifecycle command. |
| Later evidence silently reopens terminal state | Add only `needs_review` plus validated relation. Reopen is an explicit user command retaining prior history. |
| Terminal loops continue aging into overdue | Preserve deadline evidence/value but stop active aging when the obligation is not open. |
| Imprecise evidence becomes a fabricated instant | Preserve the exact precision tag; operational boundaries are policy projections and never source evidence. Soft, uncorrelated event-relative, and undated cases do not age. |
| Stale or duplicated command applies twice | Lookup opaque command key before version check; identical retry is a no-op, key collision rejects, unseen stale version produces visible conflict, and commit appends one transition atomically. |
| Duplicate merge loses independent obligations | Version-check both loops; transfer only validated non-conflicting relations/history, retain conflicts for review, and append transitions to survivor and loser. |
| Correction silently creates profiling/training | Current correction and future typed rule are separate confirmations. No free text, model training, hidden profile, or automatic historical replay. |
| Accepted OWN-03 is mistaken for passed automation gates | Hybrid and automatic capabilities remain disabled and unadvertised. ADR-011 and G-AUTO/G-AUTO-FULL retain exclusive activation authority. |
| Local terminal change causes an implicit Graph operation | Local transition commits independently. ADR-009 owns every reminder operation; P0-WI-11 contains no permission, call, origin, adapter, or operation. |
| Evidence loss is treated as negative proof | Changed/partial/unavailable evidence freezes dependent automation and never proves failure, dismissal, or closure. |
| New field leaks readable content | Exact ADR-PRIV-001 field coverage; unknown/open/generic/unbounded/readable values reject before encryption and after authenticated decode. |
| Rollback replays lifecycle or outward mutation | ADR-005 rollback suspicion freezes outward mutation and requires reconciliation before any adapter resumes. |

## Required later gates

G-STATE/G-PRIV remain prerequisites for accepting persistent logical records;
G-MODEL remains required for model hypotheses; G-TODO/G-CAL and ADR-009 remain
required for reminder operations; G-AUTO and G-AUTO-FULL plus ADR-011 remain
required for automatic behavior. P0-WI-11 passes none of them.
