import { Link } from '@tanstack/react-router';

export default function HomeScreen() {
  return (
    <main className="shell">
      <h1>Lazarus</h1>
      <p>Local-first, multi-agent, spec-driven engineering platform.</p>
      <section className="home-card">
        <h2>Welcome</h2>
        <p className="muted">
          This Home surface is a placeholder proving the shell, routing, and theme layers. Task,
          workspace, and agent features arrive in later phases.
        </p>
        <div className="actions">
          <Link to="/host-status">Open Host status</Link>
        </div>
      </section>
    </main>
  );
}
