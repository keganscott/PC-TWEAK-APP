import { Check, Cpu, Gamepad2, Layers, Play, ShieldCheck, type LucideIcon } from "lucide-react";

import type { GameGpuChoice } from "../../generated/GameGpuChoice";
import type { GameInstall } from "../../generated/GameInstall";
import type { GameReadiness } from "../../generated/GameReadiness";
import type { GpuPreference } from "../../generated/GpuPreference";
import type { PlayReport } from "../../generated/PlayReport";
import type { Probe } from "../../generated/Probe";
import type { SecurityFeature } from "../../generated/SecurityFeature";
import { explain } from "../../lib/errors";
import { formatDateTime, formatDuration } from "../../lib/format";
import { GAME_GUIDANCE } from "../../lib/gameGuidance";
import { playedReports, reportSummary } from "../../lib/playReport";
import { useActions, useStore, useTechnical } from "../../store/hooks";
import type { Op } from "../../store/store";
import { useNavigate } from "../shell/nav";
import { GameArt } from "./GameArt";
import { GameSearch } from "./GameSearch";
import { Button, Card, cx, ErrorCallout, PageHeader, SampleBadge, Skeleton, StatusBadge, type Tone } from "../ui/primitives";

const FEATURE: Record<SecurityFeature, string> = {
  secure_boot: "Secure Boot",
  tpm: "TPM 2.0",
  iommu: "IOMMU (VT-d / AMD-Vi)",
};

const FEATURE_ICON: Record<SecurityFeature, LucideIcon> = { secure_boot: ShieldCheck, tpm: Cpu, iommu: Layers };

function probeBadge(p: Probe<unknown>): { tone: Tone; label: string } {
  if (p.state === "yes") return { tone: "ok", label: "On" };
  if (p.state === "no") return { tone: "bad", label: "Off" };
  return { tone: "neutral", label: "Could not tell" };
}

export function GamesView() {
  const games = useStore((s) => s.games);
  const target = useStore((s) => s.targetGame);
  const targetOp = useStore((s) => s.targetOp);
  const audit = useStore((s) => s.audit);
  const technical = useTechnical();
  const sample = useStore((s) => s.sample);
  const launchOps = useStore((s) => s.launchOps);
  const gamingMode = useStore((s) => s.settings?.gamingMode ?? null);
  const play = useStore((s) => s.play);
  const played = playedReports(play);
  const { selectTargetGame, launchGame } = useActions();
  const readiness = audit?.antiCheat ?? null;
  const installs = audit?.env.gameInstalls ?? null;
  // The graphics chip matters only where the scanner looked at it: laptops
  // with more than one chip.
  const twoChips = audit?.scan.findings.some((f) => f.id === "gpu.choice") ?? false;
  const choices = twoChips ? (audit?.env.gpuChoices ?? []) : [];
  // Games found on this PC are marked in the picker.
  const found = new Set((installs ?? []).map((i) => i.gameId));
  const featured = games.filter((g) => g.featured);
  const others = games.filter((g) => !g.featured).sort((a, b) => a.name.localeCompare(b.name));
  // A card for each featured game, and for the main game when it is from the
  // list; the main game's card comes first.
  const shown = (readiness?.perGame ?? [])
    .filter((g) => g.gameId === target || featured.some((f) => f.id === g.gameId))
    .sort((a, b) => Number(b.gameId === target) - Number(a.gameId === target));

  return (
    <>
      <PageHeader
        title="Games"
        description="Pick the game you play most. PeakTweaks then holds back any change that game's anti-cheat would not accept."
      />
      <div className="flex max-w-4xl flex-col gap-5 2xl:max-w-6xl">
        <Card>
          <fieldset disabled={targetOp.status === "running"}>
            <legend className="mb-3 font-bold">Main game</legend>
            <div className="grid grid-cols-2 gap-3 sm:grid-cols-3 xl:grid-cols-5">
              {featured.map((g) => {
                const checked = target === g.id;
                return (
                  <label
                    key={g.id}
                    className={cx(
                      "group relative isolate flex h-32 cursor-pointer flex-col justify-end overflow-hidden rounded-2xl border p-3 transition-[transform,border-color] duration-200",
                      "has-focus-visible:outline-2 has-focus-visible:outline-offset-2 has-focus-visible:outline-lime",
                      "motion-safe:hover:-translate-y-0.5",
                      checked ? "border-lime motion-safe:animate-gm-glow" : "border-line-strong hover:border-violet-soft",
                    )}
                  >
                    <GameArt id={g.id} name={g.name} className="-z-10 transition-transform duration-300 motion-safe:group-hover:scale-105" />
                    <div aria-hidden className="absolute inset-0 -z-10 bg-linear-to-t from-black/85 via-black/30 to-transparent" />
                    <input
                      type="radio"
                      name="target-game"
                      className="sr-only"
                      checked={checked}
                      onChange={() => void selectTargetGame(g.id)}
                    />
                    {checked && (
                      <span
                        aria-hidden
                        className="absolute top-2.5 left-2.5 flex items-center gap-1 rounded-full bg-lime px-2 py-0.5 text-[10px] font-bold tracking-wider text-black uppercase">
                        <Check aria-hidden className="size-3" strokeWidth={3} />
                        Main game
                      </span>
                    )}
                    <span className="font-display text-base leading-tight font-extrabold text-white">{g.name}</span>
                    {found.has(g.id) && (
                      <>
                        {" "}
                        <span className="mt-0.5 text-xs font-semibold text-white/80">on this PC</span>
                      </>
                    )}
                  </label>
                );
              })}
            </div>
            <div className="mt-4 flex flex-wrap items-center gap-3">
              <label htmlFor="other-game" className="text-sm text-ink-muted">
                Or another game
              </label>
              <GameSearch
                id="other-game"
                games={others}
                found={found}
                chosen={others.find((g) => g.id === target) ?? null}
                disabled={targetOp.status === "running"}
                onPick={(id) => void selectTargetGame(id)}
              />
              {target !== null && (
                <button
                  type="button"
                  onClick={() => void selectTargetGame(null)}
                  className="text-sm font-semibold text-ink-muted underline-offset-2 hover:text-ink hover:underline"
                >
                  No main game
                </button>
              )}
            </div>
          </fieldset>
          {targetOp.status === "failed" && (
            <div className="mt-3">
              <ErrorCallout text={explain(targetOp.error)} technical={technical} />
            </div>
          )}
        </Card>

        <section aria-labelledby="security-title">
          <h2 id="security-title" className="mb-3 text-base font-extrabold tracking-tight">
            Security features some anti-cheats require
          </h2>
          {!readiness ? (
            <Skeleton className="h-28 w-full" label="Loading security features" />
          ) : (
            <Card>
              <ul className="grid gap-3 sm:grid-cols-3">
                {(["secure_boot", "tpm", "iommu"] as const).map((f) => {
                  const probe = f === "secure_boot" ? readiness.secureBoot : f === "tpm" ? readiness.tpm : readiness.iommu;
                  const { tone, label } = probeBadge(probe);
                  const Icon = FEATURE_ICON[f];
                  return (
                    <li
                      key={f}
                      className={cx(
                        "flex gap-3 rounded-xl border bg-surface-2 p-3",
                        probe.state === "yes" ? "border-lime/40" : probe.state === "no" ? "border-bad/40" : "border-line",
                      )}
                    >
                      <span
                        aria-hidden
                        className={cx(
                          "flex size-9 shrink-0 items-center justify-center rounded-lg",
                          probe.state === "yes" ? "bg-lime/15 text-lime" : probe.state === "no" ? "bg-bad/15 text-bad" : "bg-surface-3 text-ink-muted",
                        )}
                      >
                        <Icon className="size-[18px]" />
                      </span>
                      <div className="min-w-0">
                        <p className="text-sm font-bold">{FEATURE[f]}</p>
                        <div className="mt-1">
                          <StatusBadge tone={tone}>{label}</StatusBadge>
                          {probe.state !== "yes" && <p className="mt-1 text-xs text-ink-faint wrap-anywhere">{probe.reason}</p>}
                        </div>
                      </div>
                    </li>
                  );
                })}
              </ul>
              <p className="mt-4 text-xs text-ink-faint">
                These are firmware settings: each is turned on in your PC's firmware setup (BIOS/UEFI), and your
                motherboard manual shows how. PeakTweaks only reads them and never switches them.
              </p>
            </Card>
          )}
        </section>

        {readiness && (
          <section aria-labelledby="per-game-title">
            <div className="mb-3 flex items-center gap-2">
              <h2 id="per-game-title" className="text-base font-extrabold tracking-tight">
                Per game
              </h2>
              {sample && <SampleBadge />}
            </div>
            <ul className="flex flex-col gap-3">
              {shown.map((g) => (
                <li key={g.gameId}>
                  <GameCard
                    readiness={g}
                    name={games.find((x) => x.id === g.gameId)?.name ?? g.gameId}
                    main={g.gameId === target}
                    install={
                      installs && games.find((x) => x.id === g.gameId)?.lookedFor
                        ? (installs.find((i) => i.gameId === g.gameId) ?? null)
                        : undefined
                    }
                    choice={choices.find((c) => c.gameId === g.gameId)}
                    launchOp={launchOps[g.gameId]}
                    gamingMode={gamingMode}
                    played={played.filter((r) => r.game === g.gameId)}
                    onLaunch={() => void launchGame(g.gameId)}
                    technical={technical}
                  />
                </li>
              ))}
            </ul>
          </section>
        )}
      </div>
    </>
  );
}

const CHOICE: Record<GpuPreference, string> = {
  not_set: "no choice saved in Windows, so Windows or the graphics driver picks",
  let_windows_decide: "set to let Windows decide",
  power_saving: "set to Power saving",
  high_performance: "set to High performance",
};

function installText(install: GameInstall): string {
  const kind =
    install.disk.state !== "yes"
      ? ""
      : install.disk.value.media === "hdd"
        ? ", a hard drive"
        : ", a solid-state drive";
  return `Installed at ${install.path}${install.drive ? ` (${install.drive}${kind})` : ""}.`;
}

/**
 * One game: anti-cheat readiness, where it is installed, the graphics chip on
 * two-chip laptops, and advice for the game's own settings. `install` is
 * `undefined` when the scan has not looked yet and `null` when it found nothing.
 */
function GameCard({
  readiness,
  name,
  main,
  install,
  choice,
  launchOp,
  gamingMode,
  played,
  onLaunch,
  technical,
}: {
  readiness: GameReadiness;
  name: string;
  /** The game picked as the main game. */
  main: boolean;
  install: GameInstall | null | undefined;
  choice: GameGpuChoice | undefined;
  launchOp: Op | undefined;
  /** The Gaming Mode setting, or null before the settings are read. */
  gamingMode: boolean | null;
  /** The kept reports of this game, oldest first. */
  played: readonly PlayReport[];
  onLaunch: () => void;
  technical: boolean;
}) {
  const guidance = GAME_GUIDANCE[readiness.gameId];
  const s = readiness.status;
  const badge =
    s.status === "ready"
      ? { tone: "ok" as const, label: "Meets the listed requirements" }
      : s.status === "not_ready"
        ? { tone: "bad" as const, label: "Missing requirements" }
        : s.status === "unknown"
          ? { tone: "neutral" as const, label: "Could not tell" }
          : { tone: "info" as const, label: "No known requirements" };
  const steam = install?.steamApp != null;
  return (
    <section className={cx("overflow-hidden rounded-2xl border bg-surface-1", main ? "border-lime/50" : "border-line")}>
      <div className="relative isolate flex min-h-28 flex-wrap items-end justify-between gap-3 p-5">
        <GameArt id={readiness.gameId} name={name} big className="-z-10" />
        <div aria-hidden className="absolute inset-0 -z-10 bg-linear-to-r from-black/85 via-black/55 to-black/10" />
        <div className="min-w-0">
          {main && (
            <span className="mb-1.5 inline-flex items-center gap-1 rounded-full bg-lime px-2 py-0.5 text-[10px] font-bold tracking-wider text-black uppercase">
              <Check aria-hidden className="size-3" strokeWidth={3} />
              Your main game
            </span>
          )}
          <div className="flex flex-wrap items-center gap-2">
            <h3 className="font-display text-2xl font-extrabold tracking-tight text-white">{name}</h3>
            <StatusBadge tone={badge.tone}>{badge.label}</StatusBadge>
          </div>
        </div>
        {steam && (
          <Button
            variant="go"
            onClick={onLaunch}
            disabled={launchOp?.status === "running"}
            icon={<Play aria-hidden className="size-4 fill-current" />}
            className="px-5 py-2.5"
          >
            Play {name}
          </Button>
        )}
      </div>
      <div className={cx("grid gap-x-8 gap-y-4 p-5 pt-4", guidance && "lg:grid-cols-[1fr_1.15fr]")}>
        <div className="min-w-0">
          {readiness.requires.length > 0 && (
            <p className="text-sm text-ink-muted">
              Requires {readiness.requires.map((f) => FEATURE[f]).join(", ")}
              {readiness.scope && ` for ${readiness.scope}`}.
            </p>
          )}
          {s.status === "not_ready" && s.missing.length > 0 && (
            <p className="mt-1 text-sm">
              Missing: {s.missing.map((f) => FEATURE[f]).join(", ")}.{" "}
              <span className="text-ink-muted">
                {s.missing.length === 1 ? "It is" : "They are"} turned on in your PC's firmware setup (BIOS/UEFI), as above.
              </span>
            </p>
          )}
          {(s.status === "not_ready" || s.status === "unknown") && s.unresolved.length > 0 && (
            <p className="mt-1 text-sm text-ink-muted">Could not check: {s.unresolved.map((f) => FEATURE[f]).join(", ")}.</p>
          )}
          {readiness.source && <p className="mt-2 text-xs text-ink-faint">Source: {readiness.source}</p>}
          {install !== undefined && (
            <p className="mt-3 text-sm wrap-anywhere">
              {install ? installText(install) : "Not found in the places PeakTweaks looks."}
            </p>
          )}
          {install?.exe && <MeasureRow gameId={readiness.gameId} name={name} technical={technical} />}
          {steam && (
            <p className="mt-2 text-xs text-ink-faint" role="status">
              {launchOp?.status === "running"
                ? "Asking Steam to start it…"
                : launchOp?.status === "done"
                  ? "Steam was asked to start it."
                  : "Starts through Steam, as you, without PeakTweaks' administrator rights."}
            </p>
          )}
          {steam && gamingMode !== null && (
            <p className="mt-1 text-xs text-ink-faint">
              {gamingMode
                ? "Gaming Mode is on, so its changes start when the game does."
                : "Gaming Mode is off. Turn it on at the top of the window to pause notifications and Windows Search indexing while the game runs."}
            </p>
          )}
          {launchOp?.status === "failed" && (
            <div className="mt-2">
              <ErrorCallout text={explain(launchOp.error)} technical={technical} />
            </div>
          )}
          {played.length > 0 && <Played reports={played} />}
          {choice && (
            <p className="mt-1 text-sm text-ink-muted">
              Graphics chip: {choice.preference.state === "yes" ? CHOICE[choice.preference.value] : "could not tell"}.
            </p>
          )}
        </div>
        {guidance && (
          <div className="min-w-0 border-t border-line pt-4 lg:border-t-0 lg:border-l lg:pt-0 lg:pl-8">
            <h4 className="text-sm font-bold">In the game's own settings</h4>
            <p className="mt-1 text-sm text-ink-muted">{guidance.intro}</p>
            {guidance.steps.length > 0 && (
              <ol className="mt-3 flex flex-col gap-2 text-sm">
                {guidance.steps.map((step, i) => (
                  <li key={step} className="flex gap-2.5">
                    <span
                      aria-hidden
                      className="flex size-5 shrink-0 items-center justify-center rounded-md bg-violet/20 text-[11px] font-bold text-violet-soft tabular-nums"
                    >
                      {i + 1}
                    </span>
                    <span>{step}</span>
                  </li>
                ))}
              </ol>
            )}
            {guidance.source && <p className="mt-3 text-xs text-ink-faint">Source: {guidance.source}</p>}
          </div>
        )}
      </div>
    </section>
  );
}

/**
 * "Record my next games" (`proof/auto.rs`): one click sets a Proof
 * comparison for this game to record by itself while it is played, and opens
 * Proof on it. Shown for a game found here that the game watcher looks for.
 */
function MeasureRow({ gameId, name, technical }: { gameId: string; name: string; technical: boolean }) {
  const auto = useStore((s) => s.play?.autoRecord ?? null);
  const watched = useStore((s) => s.play?.watched);
  const op = useStore((s) => s.measureOps[gameId]);
  const { measureGame, showComparison } = useActions();
  const navigate = useNavigate();
  if (!watched?.includes(gameId)) return null;
  const mine = auto?.gameId === gameId ? auto : null;
  return (
    <div className="mt-3 rounded-xl border border-violet/30 bg-violet/5 p-3">
      <div className="flex flex-wrap items-center gap-3">
        <Gamepad2 aria-hidden className="size-4 shrink-0 text-violet" />
        <p className="min-w-0 flex-1 text-sm" role="status">
          {mine
            ? mine.recordingNow
              ? `Proof is recording sample ${mine.recorded + 1} of ${mine.wanted} of ${name} now, for the ${mine.side} side.`
              : `Proof records ${name} while you play: ${mine.recorded} of ${mine.wanted} runs on the ${mine.side} side so far.`
            : `Proof can record ${name} by itself while you play, for a before and after on this PC.`}
        </p>
        {mine ? (
          <Button
            onClick={() => {
              showComparison(mine.sessionId);
              navigate("proof");
            }}
          >
            Open Proof
          </Button>
        ) : (
          <Button
            busy={op?.status === "running"}
            onClick={() => void measureGame(gameId).then((id) => id && navigate("proof"))}
            icon={<Gamepad2 aria-hidden className="size-4" />}
          >
            Record my next games
          </Button>
        )}
      </div>
      {op?.status === "failed" && (
        <div className="mt-2">
          <ErrorCallout text={explain(op.error)} technical={technical} />
        </div>
      )}
    </div>
  );
}

/** What PeakTweaks saw the last time this game ran, from the kept history. */
function Played({ reports }: { reports: readonly PlayReport[] }) {
  const last = reports.at(-1)!;
  const hottest = last.gpuHottestC;
  const times = reports.length === 1 ? "once" : reports.length === 2 ? "twice" : `${reports.length} times`;
  return (
    <p className="mt-3 text-sm">
      Last played {formatDateTime(last.endedUnixMs)}, for {formatDuration((last.endedUnixMs - last.startedUnixMs) / 1000)}
      {hottest.state === "yes" && `, hottest graphics card reading ${hottest.value} °C`}. {reportSummary(last)}{" "}
      <span className="text-ink-muted">Watched {times} in the games kept on this PC.</span>
    </p>
  );
}
