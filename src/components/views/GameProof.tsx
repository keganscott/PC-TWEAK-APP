import { Gamepad2 } from "lucide-react";

import type { ProofSessionSummary } from "../../generated/ProofSessionSummary";
import { explain } from "../../lib/errors";
import { formatDateTime } from "../../lib/format";
import { useActions, useStore, useTechnical } from "../../store/hooks";
import { AUTO_RUNS } from "../../store/store";
import { useNavigate } from "../shell/nav";
import { Button, ErrorCallout, cx } from "../ui/primitives";
import { GameArt } from "./GameArt";

/** Where a found game stands with Proof: recording now, nothing yet, the
 * before side in, or both sides in. */
export type GameProofStage = "recording" | "none" | "between" | "done";

/**
 * "Record my next games" for one game found here (`proof/auto.rs`), shared by
 * its Games card and Proof's game tiles. `exe` is the program file the scan
 * found; a comparison counts for the game when it was made for that program.
 */
export function useGameProof(gameId: string, exe: string, name: string) {
  const auto = useStore((s) => s.play?.autoRecord ?? null);
  const watched = useStore((s) => s.play?.watched?.includes(gameId) ?? false);
  const sessions = useStore((s) => s.proof.sessions);
  const op = useStore((s) => s.measureOps[gameId]);
  const { measureGame, showComparison } = useActions();
  const navigate = useNavigate();
  const mine = auto?.gameId === gameId ? auto : null;
  const file = exe.slice(exe.lastIndexOf("\\") + 1).toLowerCase();
  // The newest comparison made for this game's own program.
  const latest = sessions
    .filter((c) => c.session.gameId === gameId && c.session.exe.toLowerCase() === file)
    .reduce<ProofSessionSummary | null>((a, c) => (!a || c.session.createdUnixMs > a.session.createdUnixMs ? c : a), null);
  const stage: GameProofStage = mine
    ? "recording"
    : latest && latest.beforeRuns >= AUTO_RUNS && latest.afterRuns >= AUTO_RUNS
      ? "done"
      : latest && latest.beforeRuns >= AUTO_RUNS
        ? "between"
        : "none";
  const text = mine
    ? mine.recordingNow
      ? `Proof is recording sample ${mine.recorded + 1} of ${mine.wanted} of ${name} now, for the ${mine.side} side.`
      : `Proof records ${name} while you play: ${mine.recorded} of ${mine.wanted} runs on the ${mine.side} side so far.`
    : stage === "between"
      ? `The before side has its ${latest!.beforeRuns} runs of ${name}. Make your changes in Tools, then record the after side the same way.`
      : stage === "done"
        ? `Proof has before and after runs of ${name}, in the test started ${formatDateTime(latest!.session.createdUnixMs)}. The result is on Proof.`
        : `Proof can record ${name} by itself while you play, for a before and after on this PC.`;
  const open = (sessionId: string) => {
    showComparison(sessionId);
    navigate("proof");
  };
  const record = () => void measureGame(gameId).then((id) => id && navigate("proof"));
  return { watched, mine, latest, stage, text, op, open, record, navigate };
}

/** The buttons for where the game stands. */
export function GameProofActions({ proof }: { proof: ReturnType<typeof useGameProof> }) {
  const { mine, latest, stage, op, open, record, navigate } = proof;
  const recordButton = (label: string) => (
    <Button busy={op?.status === "running"} onClick={record} icon={<Gamepad2 aria-hidden className="size-4" />}>
      {label}
    </Button>
  );
  return (
    <div className="flex flex-wrap gap-2">
      {mine ? (
        <Button onClick={() => open(mine.sessionId)}>Open Proof</Button>
      ) : stage === "between" ? (
        <>
          <Button variant="ghost" onClick={() => navigate("tools")}>
            Open Tools
          </Button>
          {recordButton("Record the after side")}
        </>
      ) : stage === "done" ? (
        <>
          <Button variant="ghost" onClick={() => open(latest!.session.sessionId)}>
            See the result
          </Button>
          {recordButton("Record a new test")}
        </>
      ) : (
        recordButton("Record my next games")
      )}
    </div>
  );
}

/**
 * Proof's way in without typing anything: a tile for each game found on this
 * PC that the game watcher looks for, each with where it stands. Shown until a
 * comparison is open.
 */
export function ProofGames() {
  const installs = useStore((s) => s.audit?.env.gameInstalls ?? null);
  const watched = useStore((s) => s.play?.watched);
  const found = (installs ?? []).filter((i) => i.exe && watched?.includes(i.gameId));
  if (found.length === 0) return null;
  return (
    <section aria-labelledby="proof-games-title">
      <h2 id="proof-games-title" className="font-display text-xl font-extrabold tracking-tight">
        Record your games
      </h2>
      <p className="mt-1 text-sm text-ink-muted">
        Pick a game found on this PC and play as usual: Proof takes 30-second samples while it is the window in front, three
        before your changes and three after.
      </p>
      <ul className="mt-3 grid gap-3 sm:grid-cols-2 xl:grid-cols-3">
        {found.map((i) => (
          <li key={i.gameId}>
            <ProofGameTile gameId={i.gameId} exe={i.exe!} name={i.name} />
          </li>
        ))}
      </ul>
    </section>
  );
}

function ProofGameTile({ gameId, exe, name }: { gameId: string; exe: string; name: string }) {
  const proof = useGameProof(gameId, exe, name);
  const technical = useTechnical();
  return (
    <article
      aria-labelledby={`proof-game-${gameId}`}
      className={cx(
        "relative isolate flex h-full flex-col overflow-hidden rounded-2xl border bg-surface-1 p-4",
        proof.mine?.recordingNow ? "border-bad/60" : proof.stage === "none" ? "border-line" : "border-violet/50",
      )}
    >
      <GameArt id={gameId} name={name} className="-z-10 opacity-70" />
      <div aria-hidden className="absolute inset-0 -z-10 bg-linear-to-t from-black via-black/80 to-black/20" />
      <h3 id={`proof-game-${gameId}`} className="font-display text-lg font-extrabold tracking-tight">
        {name}
      </h3>
      <p className="mt-1 flex-1 text-sm text-ink" role="status">
        {proof.text}
      </p>
      <div className="mt-3">
        <GameProofActions proof={proof} />
      </div>
      {proof.op?.status === "failed" && (
        <div className="mt-2">
          <ErrorCallout text={explain(proof.op.error)} technical={technical} />
        </div>
      )}
    </article>
  );
}
