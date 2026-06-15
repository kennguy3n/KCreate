// H1 — fuzzy matcher unit tests.
//
// The command palette runs `fuzzyScore` / `fuzzyScoreFields` on every
// command per keystroke, so these pin the contract the palette ranks
// on: empty query is neutral, non-subsequences are rejected, and the
// scoring favours word-initial + consecutive matches so the most
// relevant command floats to the top.

import { describe, it, expect } from "vitest";

import { fuzzyScore, fuzzyScoreFields } from "./fuzzyMatch";

describe("fuzzyScore", () => {
  it("treats an empty query as a neutral match-everything", () => {
    expect(fuzzyScore("anything", "")).toEqual({ score: 0, indices: [] });
  });

  it("returns null when the query is not a subsequence", () => {
    expect(fuzzyScore("Template", "xyz")).toBeNull();
    // Right letters, wrong order: "mr" is not a subsequence of "trim".
    expect(fuzzyScore("trim", "mr")).toBeNull();
  });

  it("returns null when the query is longer than the text", () => {
    expect(fuzzyScore("ab", "abc")).toBeNull();
  });

  it("reports ascending matched indices for a subsequence", () => {
    const match = fuzzyScore("Template", "tpl");
    expect(match).not.toBeNull();
    // t@0, p... no 'p' before 'l'? "template" => t0 e1 m2 p3 l4 a5 t6 e7.
    expect(match!.indices).toEqual([0, 3, 4]);
  });

  it("ranks a word-initial match above an in-word one", () => {
    const wordStart = fuzzyScore("Template", "t");
    const inWord = fuzzyScore("Butter", "t");
    expect(wordStart).not.toBeNull();
    expect(inWord).not.toBeNull();
    expect(wordStart!.score).toBeGreaterThan(inWord!.score);
  });

  it("ranks a consecutive run above a gapped one", () => {
    const consecutive = fuzzyScore("ab", "ab");
    const gapped = fuzzyScore("axb", "ab");
    expect(consecutive).not.toBeNull();
    expect(gapped).not.toBeNull();
    expect(consecutive!.score).toBeGreaterThan(gapped!.score);
  });

  it("folds the text case (query is pre-lowercased by the caller)", () => {
    // The matcher lowercases `text` internally; the contract is that
    // the caller already lowercased `query` (the palette does this
    // once per keystroke). So mixed-case text matches a lowercase
    // query regardless of the text's casing.
    expect(fuzzyScore("Magic Resize", "magic")).not.toBeNull();
    expect(fuzzyScore("MAGIC RESIZE", "magic")).not.toBeNull();
  });
});

describe("fuzzyScoreFields", () => {
  it("keeps highlight indices for a label (field 0) match", () => {
    const match = fuzzyScoreFields(["Brand kit", "theme"], "brand");
    expect(match).not.toBeNull();
    expect(match!.indices.length).toBeGreaterThan(0);
  });

  it("matches a keyword-only term but drops indices (nothing to highlight)", () => {
    // "brand" is not a subsequence of the label "Open Theme" but is a
    // full keyword match — it must score yet carry no label indices.
    const match = fuzzyScoreFields(["Open Theme", "brand"], "brand");
    expect(match).not.toBeNull();
    expect(match!.score).toBeGreaterThan(0);
    expect(match!.indices).toEqual([]);
  });

  it("demotes a keyword match below an equivalent label match", () => {
    const label = fuzzyScoreFields(["brand"], "brand");
    const keyword = fuzzyScoreFields(["Open Theme", "brand"], "brand");
    expect(label).not.toBeNull();
    expect(keyword).not.toBeNull();
    expect(keyword!.score).toBeLessThan(label!.score);
  });

  it("returns null when no field matches", () => {
    expect(fuzzyScoreFields(["Open Theme", "brand"], "zzz")).toBeNull();
  });
});
