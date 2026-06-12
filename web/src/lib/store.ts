import { create } from "zustand";

export interface Health {
  status: string;
  version: string;
}

interface AppState {
  greeting: string;
  health: Health | null;
  error: string | null;
  load: () => Promise<void>;
}

// Global app state. `load` pulls the server greeting + health into the store so
// any route/component can read it without re-fetching.
export const useAppStore = create<AppState>((set) => ({
  greeting: "…",
  health: null,
  error: null,
  load: async () => {
    set({ error: null });
    try {
      const [hello, health] = await Promise.all([
        fetch("/api/hello").then((r) => r.json() as Promise<{ message: string }>),
        fetch("/api/health").then((r) => r.json() as Promise<Health>),
      ]);
      set({ greeting: hello.message, health });
    } catch (e) {
      set({ error: e instanceof Error ? e.message : String(e) });
    }
  },
}));
