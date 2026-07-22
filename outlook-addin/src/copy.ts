/**
 * The empathetic, evidence-limited copy templates (OL-UX-003: "MUST use
 * empathetic, evidence-limited wording and MUST NOT state that the user
 * failed, was unreliable, or necessarily omitted work outside email").
 *
 * A closed template table keyed by state — one row per
 * `contracts/evidence/identity-boundary.json` `unavailable_projection.safe_codes`
 * entry, one per problem `ReminderState` from `projection.ts`, and one for
 * the OL-DUE-007 missing-deadline prompt. Every template's placeholders are
 * named and validated at render time; [`BANNED_PHRASES`] is the exact,
 * closed list of failure-attribution language no template may contain
 * (checked by this module's own tests and re-checked, adversarially, by
 * `test/copy.test.ts`).
 */

/** The closed set of copy keys this table supports. */
export type CopyKey =
  | "evidence_unavailable"
  | "evidence_changed"
  | "evidence_ambiguous"
  | "navigation_unavailable"
  | "reconnect_required"
  | "stale"
  | "reminder_missing"
  | "reminder_conflict"
  | "reminder_ambiguous_write"
  | "reminder_changed"
  | "needs_deadline_prompt";

/** One closed template row. */
export interface CopyTemplate {
  readonly key: CopyKey;
  readonly template: string;
  readonly placeholders: readonly string[];
}

/**
 * Absence of evidence is never failure proof (OL-UX-003, evidence
 * `unavailable_projection.closure_rule`: "unavailable changed or missing
 * evidence never closes resolves dismisses or proves failure"). Every
 * template below is written to that standard: it describes what OpenLoops
 * currently knows, never what the user did or did not do.
 */
const TEMPLATES: Readonly<Record<CopyKey, CopyTemplate>> = {
  evidence_unavailable: {
    key: "evidence_unavailable",
    template:
      'We couldn’t reload the source for "{loopLabel}" just now. This doesn’t confirm anything was missed — the message may have moved, or the connection may be temporarily unavailable.',
    placeholders: ["loopLabel"],
  },
  evidence_changed: {
    key: "evidence_changed",
    template:
      'The source for "{loopLabel}" looks different from what we last saw. Take a look when you can — we’ve paused automatic updates on this one until it’s reviewed.',
    placeholders: ["loopLabel"],
  },
  evidence_ambiguous: {
    key: "evidence_ambiguous",
    template:
      'We found more than one possible match for "{loopLabel}" and want your confirmation before continuing.',
    placeholders: ["loopLabel"],
  },
  navigation_unavailable: {
    key: "navigation_unavailable",
    template:
      "We can’t open this item directly right now. It may still be there — this is a display limitation, not a sign it’s gone.",
    placeholders: [],
  },
  reconnect_required: {
    key: "reconnect_required",
    template:
      "Reconnect your account when you have a moment so OpenLoops can keep checking for updates on {loopLabel}.",
    placeholders: ["loopLabel"],
  },
  stale: {
    key: "stale",
    template:
      'What you’re seeing for "{loopLabel}" may be a little behind — we’ll refresh it as soon as we can reach the source again.',
    placeholders: ["loopLabel"],
  },
  reminder_missing: {
    key: "reminder_missing",
    template:
      'The reminder linked to "{loopLabel}" isn’t where we expected. It may have been removed on purpose — let us know how you’d like to proceed.',
    placeholders: ["loopLabel"],
  },
  reminder_conflict: {
    key: "reminder_conflict",
    template:
      'Something changed on "{loopLabel}" at the same time we tried to update it. Nothing was overwritten — take a look and choose how to reconcile it.',
    placeholders: ["loopLabel"],
  },
  reminder_ambiguous_write: {
    key: "reminder_ambiguous_write",
    template:
      'We’re not certain whether the last update to "{loopLabel}" went through. Nothing further will happen automatically until you confirm.',
    placeholders: ["loopLabel"],
  },
  reminder_changed: {
    key: "reminder_changed",
    template:
      'The reminder for "{loopLabel}" was edited directly. We’ll keep your edit — just confirm how future updates should be handled.',
    placeholders: ["loopLabel"],
  },
  needs_deadline_prompt: {
    key: "needs_deadline_prompt",
    template:
      'Want to set a deadline for "{loopLabel}"? If there isn’t one, that’s fine too — you can say so and we’ll stop asking.',
    placeholders: ["loopLabel"],
  },
};

/**
 * The closed, exact list of failure-attribution phrases OL-UX-003
 * prohibits. Checked case-insensitively.
 */
export const BANNED_PHRASES: readonly string[] = [
  "you failed",
  "you forgot",
  "your fault",
  "you didn't",
  "you did not",
  "you never",
  "you neglected",
  "you ignored",
  "you missed",
  "unreliable",
  "necessarily omitted",
];

/** Returns every closed [`CopyKey`], in the table's declared order. */
export function copyKeys(): readonly CopyKey[] {
  return [
    "evidence_unavailable",
    "evidence_changed",
    "evidence_ambiguous",
    "navigation_unavailable",
    "reconnect_required",
    "stale",
    "reminder_missing",
    "reminder_conflict",
    "reminder_ambiguous_write",
    "reminder_changed",
    "needs_deadline_prompt",
  ];
}

/** Returns the closed template row for `key`. */
export function templateFor(key: CopyKey): CopyTemplate {
  return TEMPLATES[key];
}

/**
 * A rejected [`renderCopy`] call: `key`'s template names `placeholder` but
 * `values` did not supply it. Thrown rather than silently substituting a
 * blank, which could otherwise render as accidental, unintended
 * failure-shaped wording.
 */
export class MissingPlaceholderError extends Error {
  readonly key: CopyKey;
  readonly placeholder: string;

  constructor(key: CopyKey, placeholder: string) {
    super(`copy template "${key}" is missing placeholder "${placeholder}"`);
    this.name = "MissingPlaceholderError";
    this.key = key;
    this.placeholder = placeholder;
  }
}

/**
 * Renders `key`'s template with `values`. Every named placeholder in the
 * template must be present in `values`.
 *
 * @throws {MissingPlaceholderError} when a named placeholder is absent from
 * `values`.
 */
export function renderCopy(
  key: CopyKey,
  values: Readonly<Record<string, string>>,
): string {
  const template = templateFor(key);
  let text = template.template;
  for (const placeholder of template.placeholders) {
    const value = values[placeholder];
    if (value === undefined) {
      throw new MissingPlaceholderError(key, placeholder);
    }
    text = text.split(`{${placeholder}}`).join(value);
  }
  return text;
}

/** Whether `text` contains any [`BANNED_PHRASES`] entry, case-insensitively. */
export function containsBannedPhrase(text: string): boolean {
  const lowered = text.toLowerCase();
  return BANNED_PHRASES.some((phrase) =>
    lowered.includes(phrase.toLowerCase()),
  );
}
