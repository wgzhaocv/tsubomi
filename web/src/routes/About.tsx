import { Link } from "react-router";

export default function About() {
  return (
    <main className="flex min-h-dvh flex-col items-center justify-center gap-6 bg-background p-8 text-foreground">
      <h1 className="text-4xl font-semibold tracking-tight">About</h1>
      <p className="max-w-md text-center text-muted-foreground">
        つぼみ — an axum API, a React SPA, and a Rust CLI in one workspace.
      </p>
      <Link to="/" className="text-sm text-muted-foreground underline-offset-4 hover:underline">
        ← Home
      </Link>
    </main>
  );
}
