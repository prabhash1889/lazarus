import { type ReactNode } from 'react';

import { DiffPrototype } from '../components/engine-matrix/DiffPrototype';
import { EditorPrototype } from '../components/engine-matrix/EditorPrototype';
import { TerminalPrototype } from '../components/engine-matrix/TerminalPrototype';

const SAMPLE_DIFF = `--- a/src/example.ts
+++ b/src/example.ts
@@ -1,5 +1,6 @@
 import { boot } from './boot';
+import { report } from './report';
 
 export function main(): void {
-  boot();
+  const metrics = boot();
+  report(metrics);
 }
`;

/**
 * The engine rendering matrix surface (Phase 3.5): mounts the three heavy
 * components planned for later phases so their behavior can be exercised
 * and recorded on WebView2, WKWebView, and WebKitGTK. Reachable from the
 * command palette; results are tracked in docs/product/engine-rendering-matrix.md.
 */
export default function EngineMatrixScreen(): ReactNode {
  return (
    <main className="shell engine-matrix" data-testid="engine-matrix">
      <h1>Engine rendering matrix</h1>
      <p>
        Prototype status of the heavy components on this WebView engine. Record outcomes in
        docs/product/engine-rendering-matrix.md after exercising each control.
      </p>

      <section className="home-card" aria-label="Terminal prototype">
        <h2>xterm.js terminal</h2>
        <TerminalPrototype />
      </section>

      <section className="home-card" aria-label="Diff prototype">
        <h2>Diff view</h2>
        <DiffPrototype diffText={SAMPLE_DIFF} />
      </section>

      <section className="home-card" aria-label="Editor prototype">
        <h2>CodeMirror editor</h2>
        <EditorPrototype />
      </section>
    </main>
  );
}
