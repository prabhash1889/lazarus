import { useMemo, type ReactNode } from 'react';

import { joinClassNames } from '../Button';
import { parseUnifiedDiff } from './diff-model';

const LINE_CLASS: Record<string, string> = {
  context: 'diff-line-context',
  add: 'diff-line-add',
  remove: 'diff-line-remove',
  meta: 'diff-line-meta',
};

const LINE_SIGN: Record<string, string> = {
  context: ' ',
  add: '+',
  remove: '-',
  meta: '',
};

/**
 * Phase 3.5 prototype of the diff tile renderer. Exercises line-level
 * coloring, monospace alignment, and horizontal scrolling - the pieces the
 * per-engine matrix needs to validate - without the full Git diff data
 * model that arrives in Phase 5.
 */
export function DiffPrototype({ diffText }: { diffText: string }): ReactNode {
  const parsed = useMemo(() => parseUnifiedDiff(diffText), [diffText]);

  return (
    <div className="diff-prototype" data-testid="diff-prototype">
      {parsed.meta.length > 0 ? (
        <div className="diff-file-meta" aria-hidden="true">
          {parsed.meta.slice(0, 4).map((line, index) => (
            <div key={index} className="diff-line diff-line-meta">
              {line}
            </div>
          ))}
        </div>
      ) : null}
      {parsed.hunks.map((hunk, hunkIndex) => (
        <div key={hunkIndex} className="diff-hunk">
          <div className="diff-line diff-line-meta">{hunk.header}</div>
          {hunk.lines.map((line, index) => (
            <div key={index} className={joinClassNames('diff-line', LINE_CLASS[line.kind])}>
              <span className="diff-sign" aria-hidden="true">
                {LINE_SIGN[line.kind]}
              </span>
              <span>{line.text}</span>
            </div>
          ))}
        </div>
      ))}
    </div>
  );
}
