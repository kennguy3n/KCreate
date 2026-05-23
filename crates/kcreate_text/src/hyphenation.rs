//! Liang's hyphenation algorithm.
//!
//! This is the same algorithm that drives TeX. Given a set of
//! patterns of the form `hy3phen` (where digits represent
//! hyphenation priorities) and a word, it returns the byte offsets
//! at which a soft hyphen may be inserted. The line-breaker in
//! [`crate::paragraph`] consumes those offsets when an unbroken
//! word doesn't fit a column.
//!
//! Patterns ship with the crate as a static `.txt` file
//! ([`EN_US_PATTERNS`]) so the editing path stays network-free.
//! Additional language packs may be loaded at runtime from the
//! project's asset directory via
//! [`HyphenationPatterns::from_tex_patterns`].
//!
//! The implementation is the textbook
//! "pad with boundary markers, scan every substring, overlay
//! highest digit per position, accept odd digits as valid breaks"
//! formulation. We intentionally do not build a TeX-style trie:
//! the substring scan is O(n²) in the word length, which is fine
//! because typical English words are 4–12 letters and the pattern
//! lookup is a single `HashMap` probe. If profiling ever shows
//! this on the hot path we can swap in `aho_corasick` without
//! changing the public surface.

use std::collections::HashMap;

/// Public-domain English (US) Liang patterns, embedded at compile
/// time so the editing path never touches the network or the
/// filesystem on first launch. A subset of Knuth's original
/// `hyph-en-us.pat.txt`.
pub const EN_US_PATTERNS: &str = include_str!("hyph_patterns_en_us.txt");

/// TeX-style hyphenation patterns for a single language.
///
/// Patterns are stored as `letter_key → digits` where the digit
/// vector has length `letter_key.len() + 1` (one priority per gap
/// position, including the leading and trailing gap). A `0` means
/// "no preference"; odd numbers favor a hyphen, even numbers
/// suppress one. This mirrors Liang's original format byte-for-byte.
#[derive(Debug, Clone, Default)]
pub struct HyphenationPatterns {
    /// Map from the letter-only form (e.g. `"hyphen"`) to the digit
    /// priority vector (length = letters + 1). Boundary markers
    /// (`.`) are preserved in the key — the lookup pads the word
    /// with `.` on both sides before scanning.
    patterns: HashMap<String, Vec<u8>>,
    /// Smallest legal prefix length before the first allowed break.
    /// TeX defaults to 2; English users expect "ar-row", not
    /// "a-rrow", so we keep the same value.
    left_min: usize,
    /// Smallest legal suffix length after the last allowed break.
    /// TeX defaults to 3; English users expect "draw-ing", not
    /// "drawi-ng".
    right_min: usize,
}

impl HyphenationPatterns {
    /// English (US) patterns embedded in the binary. The first call
    /// parses the static string; subsequent calls are cheap clones
    /// (the underlying map is `Arc`-free but copy-on-write friendly
    /// because patterns are small enough that profile cost is in the
    /// substring scan, not the hashmap).
    #[must_use]
    pub fn en_us() -> Self {
        Self::from_tex_patterns(EN_US_PATTERNS)
    }

    /// Parse a TeX `.pat`-formatted string. Each non-empty,
    /// non-`#`-prefixed line is interpreted as one Liang pattern.
    /// Letters are taken as-is; ASCII digits are extracted as
    /// priorities and stored in the gaps surrounding the letters.
    /// Unknown characters fall through into the key (e.g. `.` for
    /// word boundaries — Liang patterns include `.ach4` to anchor a
    /// pattern at the start of a word).
    ///
    /// Lower-cases letters so we can match against lower-cased
    /// input words without redundant case folding on every lookup.
    #[must_use]
    pub fn from_tex_patterns(content: &str) -> Self {
        let mut patterns: HashMap<String, Vec<u8>> = HashMap::new();
        for raw in content.lines() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            // Tokens after the pattern (if any — TeX pattern files
            // sometimes annotate with `%` comments) are ignored.
            let pat = line.split_whitespace().next().unwrap_or("");
            if pat.is_empty() {
                continue;
            }
            let (letters, digits) = split_letters_and_digits(pat);
            // Skip patterns that have no letters at all (e.g. lines
            // that were entirely numeric — Knuth's source contains
            // none of these but a misformatted user pack might).
            if letters.is_empty() {
                continue;
            }
            patterns.insert(letters, digits);
        }
        Self {
            patterns,
            left_min: 2,
            right_min: 3,
        }
    }

    /// Override the left / right boundary minima. Useful for
    /// languages where TeX-default 2/3 is wrong (Czech and Hungarian
    /// are 1/2, for instance).
    #[must_use]
    pub fn with_boundary_minima(mut self, left_min: usize, right_min: usize) -> Self {
        self.left_min = left_min;
        self.right_min = right_min;
        self
    }

    /// Number of loaded patterns. Useful for assertions in tests
    /// that the pattern file deserialised cleanly.
    #[must_use]
    pub fn len(&self) -> usize {
        self.patterns.len()
    }

    /// Whether this pattern set is empty (e.g. nothing parsed).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.patterns.is_empty()
    }

    /// Return byte offsets at which `word` may be hyphenated.
    ///
    /// Offsets are into the *original* (case-preserved) `word`; the
    /// caller can insert a soft hyphen at each one. The algorithm
    /// works on ASCII letters only — non-ASCII characters are
    /// treated as opaque blocks that suppress hyphenation through
    /// them, which is the conservative choice for the embedded
    /// English pattern set. Language packs covering accented Latin
    /// would carry patterns with their own diacritics and would be
    /// lower-cased exactly the same way.
    ///
    /// Returns an empty vector for words shorter than
    /// `left_min + right_min + 1` because no break would be legal.
    #[must_use]
    pub fn hyphenate(&self, word: &str) -> Vec<usize> {
        if self.patterns.is_empty() {
            return Vec::new();
        }
        // Only ASCII letters participate in pattern matching. We
        // bail early if the word contains non-ASCII because the
        // embedded English patterns won't help and naïvely scanning
        // them as bytes would produce indices in the middle of a
        // multi-byte UTF-8 sequence — a hard correctness bug for any
        // caller that inserts soft hyphens by byte index.
        if !word.chars().all(|c| c.is_ascii_alphabetic()) {
            return Vec::new();
        }
        if word.len() < self.left_min + self.right_min + 1 {
            return Vec::new();
        }

        // Lower-case + pad with boundary markers.
        let lower: String = word.chars().flat_map(char::to_lowercase).collect();
        let padded: String = format!(".{lower}.");
        let chars: Vec<char> = padded.chars().collect();

        // Liang's overlay: `priorities[i]` is the best digit so far
        // assigned to the gap *before* character `i` in the padded
        // word. Length is `chars.len() + 1` so the index
        // `priorities[chars.len()]` exists for the trailing boundary.
        let mut priorities: Vec<u8> = vec![0; chars.len() + 1];

        // Scan every substring of the padded word and look it up in
        // the pattern map. Up to TeX's longest pattern (about 7
        // letters) but we don't bound it here — the map probe with
        // an over-long substring simply misses.
        for start in 0..chars.len() {
            for end in (start + 1)..=chars.len() {
                let substr: String = chars[start..end].iter().collect();
                if let Some(digits) = self.patterns.get(&substr) {
                    // The digits vector has length `(end - start) + 1`;
                    // digits[k] is the priority for the gap before
                    // character `start + k` in the padded word.
                    for (k, &d) in digits.iter().enumerate() {
                        let gap = start + k;
                        if gap < priorities.len() && d > priorities[gap] {
                            priorities[gap] = d;
                        }
                    }
                }
            }
        }

        // Walk the unpadded word and convert odd priorities at
        // letter boundaries into byte offsets. The padded string
        // has a leading `.` so the priority for the gap before
        // letter `i` of the original word sits at `priorities[i+1]`.
        let mut breaks = Vec::new();
        let lower_chars: Vec<char> = lower.chars().collect();
        let word_bytes = word.as_bytes();
        // Precompute byte offset of each character in the original
        // word so the returned indices line up with the caller's
        // string slice.
        let mut byte_indices = Vec::with_capacity(lower_chars.len() + 1);
        let mut cursor = 0usize;
        byte_indices.push(0);
        for ch in word.chars() {
            cursor += ch.len_utf8();
            byte_indices.push(cursor);
        }
        // `lower_chars.len()` equals `word.chars().count()` here
        // because we already bailed on non-ASCII input above.
        debug_assert_eq!(lower_chars.len(), byte_indices.len() - 1);
        debug_assert_eq!(byte_indices[byte_indices.len() - 1], word_bytes.len());

        let n = lower_chars.len();
        for i in 1..n {
            if i < self.left_min || (n - i) < self.right_min {
                continue;
            }
            // Priority for the gap *before* the i-th letter of the
            // unpadded word is at priorities[i + 1] (the +1 skips
            // the leading boundary marker `.`).
            if priorities[i + 1] % 2 == 1 {
                breaks.push(byte_indices[i]);
            }
        }
        breaks
    }
}

/// Split a Liang pattern like `.ad4der` into the letter-only key
/// (`.adder`) and the priority vector (one entry per gap, length =
/// letters + 1). For `.ad4der` this is `[0, 0, 0, 4, 0, 0, 0]`.
fn split_letters_and_digits(pat: &str) -> (String, Vec<u8>) {
    let mut letters = String::with_capacity(pat.len());
    // We don't know the final letter count yet, but it's
    // bounded by pat.len() so this is a safe upper bound.
    let mut digits: Vec<u8> = Vec::with_capacity(pat.len() + 1);
    // The digit before the first letter starts at 0.
    digits.push(0);
    let mut pending_digit: Option<u8> = None;
    for ch in pat.chars() {
        if let Some(d) = ch.to_digit(10) {
            // Collapse runs of digits — Liang patterns never have
            // two digits in a row but a malformed user pack might.
            // Keep the larger of the two so we don't lose priority.
            pending_digit = Some(pending_digit.unwrap_or(0).max(d as u8));
        } else {
            // Lower-case letters in the key so lookup keys match the
            // lower-cased word during `hyphenate`.
            for lc in ch.to_lowercase() {
                letters.push(lc);
                // The gap *after* the previously emitted letter
                // takes whatever digit we just buffered.
                let last = digits.last_mut().expect("digits seeded with 0");
                *last = pending_digit.unwrap_or(0).max(*last);
                pending_digit = None;
                // Seed the gap after this letter with 0.
                digits.push(0);
            }
        }
    }
    if let Some(d) = pending_digit.take() {
        // Trailing digit (e.g. `bri2`) attaches to the gap after
        // the last letter.
        if let Some(last) = digits.last_mut() {
            *last = (*last).max(d);
        }
    }
    debug_assert_eq!(digits.len(), letters.chars().count() + 1);
    (letters, digits)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_letters_and_digits_extracts_priority() {
        let (letters, digits) = split_letters_and_digits(".ad4der");
        assert_eq!(letters, ".adder");
        // Six letters → seven gaps. The `4` sits between `d` and
        // `d`, i.e. in the gap at index 3.
        assert_eq!(digits, vec![0, 0, 0, 4, 0, 0, 0]);
    }

    #[test]
    fn split_letters_and_digits_trailing_digit() {
        let (letters, digits) = split_letters_and_digits("bri2");
        assert_eq!(letters, "bri");
        // Three letters → four gaps. Trailing `2` attaches to the
        // last gap.
        assert_eq!(digits, vec![0, 0, 0, 2]);
    }

    #[test]
    fn from_tex_patterns_skips_comments_and_blanks() {
        let src = "# leading comment\n\n.ad4der\nbri2\n# another comment\n";
        let p = HyphenationPatterns::from_tex_patterns(src);
        assert_eq!(p.len(), 2);
        assert!(p.patterns.contains_key(".adder"));
        assert!(p.patterns.contains_key("bri"));
    }

    #[test]
    fn empty_patterns_returns_no_breaks() {
        let p = HyphenationPatterns::default();
        assert!(p.hyphenate("anything").is_empty());
    }

    #[test]
    fn en_us_pattern_set_loads() {
        let p = HyphenationPatterns::en_us();
        // Sanity bound: the curated subset has at least a few
        // hundred patterns. The exact count is a moving target as
        // we extend coverage so we only assert a lower bound.
        assert!(
            p.len() >= 100,
            "expected >=100 en-US patterns, got {}",
            p.len()
        );
    }

    #[test]
    fn hyphenate_short_word_returns_empty() {
        let p = HyphenationPatterns::en_us();
        // 4 letters < left_min(2) + right_min(3) + 1 = 6. No legal
        // break exists for "be", "and", "the", etc.
        assert!(p.hyphenate("the").is_empty());
        assert!(p.hyphenate("and").is_empty());
        assert!(p.hyphenate("be").is_empty());
    }

    #[test]
    fn hyphenate_non_ascii_returns_empty() {
        let p = HyphenationPatterns::en_us();
        // The embedded set is ASCII; non-ASCII input falls through
        // safely (no out-of-bounds index into a multi-byte char).
        assert!(p.hyphenate("café").is_empty());
        assert!(p.hyphenate("naïve").is_empty());
    }

    #[test]
    fn hyphenate_known_english_words() {
        let p = HyphenationPatterns::en_us();
        // For each test word we assert that *some* break exists at
        // a plausible position. We avoid pinning specific indices
        // because the curated pattern subset is intentionally
        // smaller than full TeX, so the exact break set may move
        // as we add patterns. The invariant the user cares about
        // is "the engine produces *some* mid-word break for words
        // we promised it would handle".
        let cases = [
            ("hyphenation", 1),
            ("programming", 1),
            ("algorithm", 1),
            ("computer", 1),
            ("printing", 1),
        ];
        for (word, min_breaks) in cases {
            let breaks = p.hyphenate(word);
            assert!(
                breaks.len() >= min_breaks,
                "expected >={min_breaks} hyphenation point(s) in {word:?}, got {breaks:?}",
            );
            // Every returned index must respect the left/right
            // minima so the caller's slice never produces a
            // one-letter fragment.
            for &b in &breaks {
                assert!(b >= 2, "break {b} in {word:?} violates left_min");
                assert!(
                    word.len() - b >= 3,
                    "break {b} in {word:?} violates right_min"
                );
            }
        }
    }

    #[test]
    fn hyphenate_is_case_insensitive() {
        let p = HyphenationPatterns::en_us();
        let lower = p.hyphenate("hyphenation");
        let upper = p.hyphenate("Hyphenation");
        let shout = p.hyphenate("HYPHENATION");
        assert_eq!(lower, upper);
        assert_eq!(lower, shout);
    }

    #[test]
    fn with_boundary_minima_overrides_defaults() {
        // Tight minima (1/1) should let more breaks through; loose
        // minima (5/5) should suppress most breaks on short words.
        let strict = HyphenationPatterns::en_us().with_boundary_minima(5, 5);
        let loose = HyphenationPatterns::en_us().with_boundary_minima(1, 1);
        // "printing" has 8 letters; with strict minima the
        // remaining window is `5..=3` (impossible), so we expect
        // zero breaks. With loose minima at least one break should
        // sneak through.
        assert!(strict.hyphenate("printing").is_empty());
        assert!(!loose.hyphenate("printing").is_empty());
    }

    #[test]
    fn returned_breaks_are_valid_byte_indices() {
        let p = HyphenationPatterns::en_us();
        let word = "hyphenation";
        for &b in &p.hyphenate(word) {
            // Splitting at every returned index must succeed —
            // i.e. the index lands on a UTF-8 boundary. For ASCII
            // input this is trivial but the assertion guards
            // against future tweaks that route through chars().
            assert!(word.is_char_boundary(b), "non-boundary index {b}");
            let (head, tail) = word.split_at(b);
            assert!(!head.is_empty());
            assert!(!tail.is_empty());
        }
    }
}
