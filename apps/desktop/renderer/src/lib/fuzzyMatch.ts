// Tiny dependency-free fuzzy subsequence matcher used by the command
// palette. The palette runs this against every command on each
// keystroke, so it must stay allocation-light and O(query × text):
// no regex compilation, no intermediate arrays beyond the matched-
// index list (which the palette needs anyway to highlight hits).
//
// Scoring favours, in order of weight:
//   * matches at the start of a word (after a space / separator or at
//     index 0) — "magic resize" typed as "mr" should rank the
//     word-initial match above an in-word one,
//   * consecutive runs — "temp" matching "template" beats scattered
//     letters,
//   * earlier matches — a hit near the front of the label outranks a
//     late one,
// and penalises long gaps between matched characters. The absolute
// magnitude of the score is meaningless; only the relative ordering
// matters to the caller.

/** Result of a single fuzzy match. */
export interface FuzzyMatch {
  /** Higher is a better match. Only meaningful relative to other
   *  matches produced by the same `query`. */
  readonly score: number;
  /** Indices into `text` that the query characters matched, ascending.
   *  Used to render highlight spans. Empty when `query` is empty. */
  readonly indices: readonly number[];
}

const WORD_BOUNDARY = /[\s\-_/.:]/;

// Scoring weights. Tuned so a word-initial subsequence ("mr" →
// "Magic resize") always outranks an in-word one ("mr" → "trim"),
// and a contiguous prefix ("temp" → "Template") tops everything.
const SCORE_WORD_START = 12;
const SCORE_CONSECUTIVE = 8;
const SCORE_MATCH = 2;
const PENALTY_GAP = 1;
const PENALTY_LEADING = 1;
const MAX_LEADING_PENALTY = 6;

/**
 * Score `text` against a lower-cased `query`. Returns `null` when the
 * query is not a subsequence of the text. An empty query matches
 * everything with a neutral score so the palette can show its full
 * grouped list before the user types.
 *
 * `query` is expected to already be lower-cased and trimmed by the
 * caller (the palette lower-cases once per keystroke rather than once
 * per command). `text` is lower-cased internally.
 */
export function fuzzyScore(text: string, query: string): FuzzyMatch | null {
  if (query.length === 0) return { score: 0, indices: [] };
  if (query.length > text.length) return null;

  const haystack = text.toLowerCase();
  const indices: number[] = [];
  let score = 0;
  let queryIdx = 0;
  let prevMatch = -1;

  for (let i = 0; i < haystack.length && queryIdx < query.length; i += 1) {
    if (haystack[i] !== query[queryIdx]) continue;

    let charScore = SCORE_MATCH;

    const atWordStart =
      i === 0 || WORD_BOUNDARY.test(haystack[i - 1] ?? "");
    if (atWordStart) charScore += SCORE_WORD_START;

    if (prevMatch === i - 1) {
      // Consecutive with the previous matched character.
      charScore += SCORE_CONSECUTIVE;
    } else if (prevMatch >= 0) {
      // Penalise the gap we skipped, but never below zero for this
      // character so a single match can't produce a negative total.
      const gap = i - prevMatch - 1;
      charScore -= Math.min(gap * PENALTY_GAP, charScore);
    } else {
      // Distance from the start of the string before the first match;
      // a hit at index 0 is best. Capped so a long label doesn't
      // dominate the ranking purely by where the match lands.
      charScore -= Math.min(i * PENALTY_LEADING, MAX_LEADING_PENALTY);
    }

    score += charScore;
    indices.push(i);
    prevMatch = i;
    queryIdx += 1;
  }

  if (queryIdx < query.length) return null;
  return { score, indices };
}

/**
 * Convenience wrapper: match a query against several text fields
 * (e.g. a command's label plus its keyword aliases) and return the
 * best result. `indices` always refers to the FIRST field (the label)
 * so the caller can highlight it; a match found only in a later field
 * (a keyword) yields a positive score with empty `indices`, which the
 * palette renders without highlight spans.
 */
export function fuzzyScoreFields(
  fields: readonly string[],
  query: string,
): FuzzyMatch | null {
  if (query.length === 0) return { score: 0, indices: [] };
  let best: FuzzyMatch | null = null;
  for (let f = 0; f < fields.length; f += 1) {
    const match = fuzzyScore(fields[f] ?? "", query);
    if (match === null) continue;
    // Label matches (field 0) keep their highlight indices; keyword
    // matches (later fields) are demoted slightly and drop indices so
    // we never highlight characters that aren't in the visible label.
    const normalised: FuzzyMatch =
      f === 0 ? match : { score: match.score - 4, indices: [] };
    if (best === null || normalised.score > best.score) best = normalised;
  }
  return best;
}
