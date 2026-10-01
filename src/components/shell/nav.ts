import { createContext, useContext } from "react";

export type ViewId = "home" | "games" | "tools" | "proof" | "backups";

export const VIEWS: { id: ViewId; label: string }[] = [
  { id: "home", label: "Home" },
  { id: "games", label: "Games" },
  { id: "tools", label: "Tools" },
  { id: "proof", label: "Proof" },
  { id: "backups", label: "Backups" },
];

export const NavContext = createContext<(view: ViewId) => void>(() => {});

/** Switch the main view (used by in-page links such as "set up a restore point"). */
export function useNavigate(): (view: ViewId) => void {
  return useContext(NavContext);
}
