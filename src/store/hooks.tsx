import { createContext, useContext, useSyncExternalStore, type ReactNode } from "react";

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
