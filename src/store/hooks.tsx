import { createContext, useContext, useEffect, useSyncExternalStore, type ReactNode } from "react";

import type { AppStore, State } from "./store";

const StoreContext = createContext<AppStore | null>(null);

export function StoreProvider({ store, children }: { store: AppStore; children: ReactNode }) {
  return <StoreContext.Provider value={store}>{children}</StoreContext.Provider>;
}

function useAppStore(): AppStore {
  const store = useContext(StoreContext);
  if (!store) throw new Error("useStore must be used inside <StoreProvider>");
  return store;
}

/**
 * Read one slice of state. The selector must return something already in the
 * state (`s => s.tweaks`), not a new object or array built on every call: that
 * would never compare equal and would re-render without end. Derive in the
 * component with `useMemo` instead.
 */
export function useStore<T>(selector: (s: State) => T): T {
  const store = useAppStore();
  return useSyncExternalStore(store.subscribe, () => selector(store.getState()));
}

export function useActions(): AppStore["actions"] {
  return useAppStore().actions;
}

/** True when the user chose the technical wording (registry paths visible). */
export function useTechnical(): boolean {
  return useStore((s) => s.settings?.language === "technical");
}

/** How often a page showing live readings asks for new ones. Each takes half a second. */
export const LIVE_EVERY_MS = 3000;

/** Keep the live readings (`live.rs`) fresh while the calling component is on
 * screen and the window is visible. Several callers share one reading: the
 * store does not ask again while one is being read. */
export function useLiveReadings(): void {
  const { readLive } = useActions();
  useEffect(() => {
    const read = () => {
      if (!document.hidden) void readLive();
    };
    read();
    const timer = window.setInterval(read, LIVE_EVERY_MS);
    return () => window.clearInterval(timer);
  }, [readLive]);
}
