import { useId, useMemo, useRef, useState, type KeyboardEvent } from "react";

import type { GameInfo } from "../../generated/GameInfo";

/** Letters and digits only, lower case: "ARC Raiders" and "arc-raiders" both
 * become "arcraiders", so punctuation and capitals never stop a match. */
export function searchKey(text: string): string {
  return text.toLowerCase().replace(/[^\p{L}\p{N}]/gu, "");
}

/** The games whose name holds what was typed, names that start with it first,
 * each group in the order given. Everything when nothing is typed. */
export function matchGames(games: readonly GameInfo[], typed: string): GameInfo[] {
  const key = searchKey(typed);
  if (!key) return [...games];
  const starts = games.filter((g) => searchKey(g.name).startsWith(key));
  const contains = games.filter((g) => !starts.includes(g) && searchKey(g.name).includes(key));
  return [...starts, ...contains];
}

/**
 * The game list as a box to type in (Kegan, 2026-10-10: "you should be
 * allowed to type in letters to look up a game name"). A combobox with a
 * list of matches: arrow keys move through it, Enter picks, Escape closes.
 * `chosen` is the picked game from this list, shown when the box is not being
 * typed in.
 */
export function GameSearch({
  id,
  games,
  found,
  chosen,
  disabled,
  onPick,
}: {
  /** For the label outside. */
  id: string;
  games: readonly GameInfo[];
  /** Ids of games found on this PC, marked in the list. */
  found: ReadonlySet<string>;
  chosen: GameInfo | null;
  disabled: boolean;
  onPick: (id: string) => void;
}) {
  const listId = useId();
  const [typed, setTyped] = useState<string | null>(null);
  const [open, setOpen] = useState(false);
  const [active, setActive] = useState(0);
  const listRef = useRef<HTMLUListElement>(null);
  const matches = useMemo(() => matchGames(games, typed ?? ""), [games, typed]);
  const current = matches[Math.min(active, matches.length - 1)];
  const optionId = (id: string) => `${listId}-${id}`;

  const close = () => {
    setOpen(false);
    setTyped(null);
    setActive(0);
  };
  const pick = (g: GameInfo) => {
    close();
    onPick(g.id);
  };
  const move = (by: number) => {
    if (!open) setOpen(true);
    if (matches.length === 0) return;
    const next = (Math.min(active, matches.length - 1) + by + matches.length) % matches.length;
    setActive(next);
    listRef.current?.querySelector(`[id="${CSS.escape(optionId(matches[next]!.id))}"]`)?.scrollIntoView?.({ block: "nearest" });
  };
  const onKeyDown = (e: KeyboardEvent<HTMLInputElement>) => {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      move(1);
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      move(-1);
    } else if (e.key === "Enter") {
      if (open && current) {
        e.preventDefault();
        pick(current);
      }
    } else if (e.key === "Escape") {
      if (open) {
        e.preventDefault();
        close();
      }
    }
  };

  return (
    <div className="relative min-w-60 flex-1 sm:max-w-sm">
      <input
        id={id}
        type="text"
        role="combobox"
        aria-autocomplete="list"
        aria-expanded={open}
        aria-controls={listId}
        aria-activedescendant={open && current ? optionId(current.id) : undefined}
        autoComplete="off"
        spellCheck={false}
        disabled={disabled}
        placeholder="Search more games"
        value={typed ?? chosen?.name ?? ""}
        onChange={(e) => {
          setTyped(e.target.value);
          setActive(0);
          setOpen(true);
        }}
        onFocus={() => setOpen(true)}
        onClick={() => setOpen(true)}
        onBlur={close}
        onKeyDown={onKeyDown}
        className={`w-full rounded-md border bg-surface-0 px-3 py-2 text-sm placeholder:text-ink-faint ${
          chosen ? "border-violet font-bold" : "border-line-strong"
        }`}
      />
      {open && (
        <ul
          id={listId}
          ref={listRef}
          role="listbox"
          aria-label="Games"
          className="absolute z-20 mt-1 max-h-72 w-full overflow-auto rounded-md border border-line-strong bg-surface-1 py-1 text-sm shadow-lg"
        >
          {matches.length === 0 ? (
            <li role="presentation" className="px-3 py-2 text-ink-muted">
              No game by that name in the list.
            </li>
          ) : (
            matches.map((g) => {
              const isActive = g === current;
              return (
                <li
                  key={g.id}
                  id={optionId(g.id)}
                  role="option"
                  aria-selected={chosen?.id === g.id}
                  // Keep focus in the box so the pick lands before the blur closes the list.
                  onMouseDown={(e) => e.preventDefault()}
                  onClick={() => pick(g)}
                  onMouseMove={() => setActive(matches.indexOf(g))}
                  className={`flex cursor-pointer items-center justify-between gap-3 px-3 py-1.5 ${
                    isActive ? "bg-violet text-white" : "text-ink"
                  } ${chosen?.id === g.id ? "font-bold" : ""}`}
                >
                  <span>{g.name}</span>
                  {found.has(g.id) && <span className={`text-xs ${isActive ? "text-white/80" : "text-ink-faint"}`}>on this PC</span>}
                </li>
              );
            })
          )}
        </ul>
      )}
    </div>
  );
}
