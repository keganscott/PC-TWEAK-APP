import { createContext, useContext } from "react";

export type ViewId = "home" | "games" | "tools" | "proof" | "backups";

export const VIEWS: { id: ViewId; label: string; group: "Overview" | "Optimise" }[] = [
  { id: "home", label: "Home", group: "Overview" },
  { id: "games", label: "Games", group: "Overview" },
  { id: "tools", label: "Tools", group: "Optimise" },
  { id: "proof", label: "Proof", group: "Optimise" },
  { id: "backups", label: "Backups", group: "Optimise" },
];

export const NavContext = createContext<(view: ViewId) => void>(() => {});

/** Switch the main view (used by in-page links such as "set up a restore point"). */
export function useNavigate(): (view: ViewId) => void {
  return useContext(NavContext);
}
