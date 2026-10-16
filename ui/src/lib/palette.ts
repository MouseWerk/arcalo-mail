// The command palette's matching, without React.

/** How well `query` matches `text`: every word of the query must start a word of the text, or
 *  appear in it; earlier and word-start matches rank higher. `-1`: no match. */
export function score(text: string, query: string): number {
  const hay = text.toLocaleLowerCase();
  const words = query.toLocaleLowerCase().split(/\s+/).filter(Boolean);
  if (!words.length) return 0;
  let total = 0;
  for (const w of words) {
    const at = hay.indexOf(w);
    if (at < 0) return -1;
    const wordStart = at === 0 || /[\s/:–-]/.test(hay[at - 1]);
    total += (wordStart ? 0 : 50) + at;
  }
  return total;
}
