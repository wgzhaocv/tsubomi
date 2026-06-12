import { useEffect } from "react";
import { Link } from "react-router";

import { useAppStore } from "@/lib/store";

export default function Home() {
  const greeting = useAppStore((s) => s.greeting);
  const health = useAppStore((s) => s.health);
  const error = useAppStore((s) => s.error);
  const load = useAppStore((s) => s.load);

  useEffect(() => {
    void load();
  }, [load]);

  return (
    <main className="flex min-h-dvh flex-col items-center justify-center gap-6 bg-background p-8 text-foreground">
      <h1 className="text-4xl font-semibold tracking-tight">🌷 つぼみ</h1>
      <p className="text-lg text-muted-foreground">{error ? `error: ${error}` : greeting}</p>
      {health && (
        <p className="text-sm text-muted-foreground">
          server: {health.status} · v{health.version}
        </p>
      )}
      <div className="flex items-center gap-4">
        <button
          onClick={() => void load()}
          className="rounded-md bg-primary px-4 py-2 text-primary-foreground transition hover:opacity-90"
        >
          Reload
        </button>
        <Link
          to="/about"
          className="text-sm text-muted-foreground underline-offset-4 hover:underline"
        >
          About →
        </Link>
      </div>
    </main>
  );
}
