/**
 * Minimal unified-diff model for the Phase 3.5 engine rendering matrix.
 * The real diff pipeline arrives with the Git engine (Phase 5); this
 * prototype exists so the diff view's rendering behavior can be validated
 * on every WebView engine before feature work depends on it.
 */

export interface DiffLine {
  kind: 'context' | 'add' | 'remove' | 'meta';
  text: string;
}

export interface DiffHunk {
  header: string;
  oldStart: number;
  lines: DiffLine[];
}

export interface ParsedDiff {
  meta: string[];
  hunks: DiffHunk[];
}

const HUNK_HEADER = /^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@(.*)$/;

/** Parses a unified diff; tolerant of empty input and trailing noise. */
export function parseUnifiedDiff(diffText: string): ParsedDiff {
  const result: ParsedDiff = { meta: [], hunks: [] };
  let current: DiffHunk | null = null;

  for (const line of diffText.split('\n')) {
    const hunkMatch = line.match(HUNK_HEADER);
    if (hunkMatch !== null) {
      current = {
        header: line,
        oldStart: Number.parseInt(hunkMatch[1] ?? '0', 10),
        lines: [],
      };
      result.hunks.push(current);
      continue;
    }
    if (current === null) {
      if (line.trim() !== '') {
        result.meta.push(line);
      }
      continue;
    }
    if (line.startsWith('+')) {
      current.lines.push({ kind: 'add', text: line.slice(1) });
    } else if (line.startsWith('-')) {
      current.lines.push({ kind: 'remove', text: line.slice(1) });
    } else if (line.startsWith(' ') || line === '') {
      current.lines.push({ kind: 'context', text: line.startsWith(' ') ? line.slice(1) : '' });
    } else if (line.startsWith('\\')) {
      // "\ No newline at end of file" - render as context noise.
      current.lines.push({ kind: 'meta', text: line });
    } else {
      // A new file section started without an @@ we recognize.
      current = null;
      result.meta.push(line);
    }
  }
  return result;
}
