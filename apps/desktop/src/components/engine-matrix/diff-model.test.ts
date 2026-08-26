import { describe, expect, it } from 'vitest';

import { parseUnifiedDiff } from './diff-model';

describe('parseUnifiedDiff', () => {
  it('parses hunks with add, remove, and context lines', () => {
    const parsed = parseUnifiedDiff(
      ['--- a/f.ts', '+++ b/f.ts', '@@ -1,3 +1,4 @@', ' context', '-old', '+new', '+added'].join(
        '\n',
      ),
    );

    expect(parsed.meta).toEqual(['--- a/f.ts', '+++ b/f.ts']);
    expect(parsed.hunks).toHaveLength(1);
    const hunk = parsed.hunks[0]!;
    expect(hunk.oldStart).toBe(1);
    expect(hunk.lines.map((line) => line.kind)).toEqual(['context', 'remove', 'add', 'add']);
    expect(hunk.lines[1]?.text).toBe('old');
    expect(hunk.lines[2]?.text).toBe('new');
  });

  it('returns an empty parse for empty input', () => {
    expect(parseUnifiedDiff('')).toEqual({ meta: [], hunks: [] });
  });

  it('keeps multiple hunks ordered and tolerates trailing newline markers', () => {
    const parsed = parseUnifiedDiff(
      [
        '@@ -1,2 +1,2 @@',
        '-a',
        '+b',
        '\\ No newline at end of file',
        '@@ -10,1 +11,1 @@',
        ' tail',
      ].join('\n'),
    );
    expect(parsed.hunks).toHaveLength(2);
    expect(parsed.hunks[0]!.lines[parsed.hunks[0]!.lines.length - 1]!.kind).toBe('meta');
    expect(parsed.hunks[1]!.lines[0]!.text).toBe('tail');
  });
});
