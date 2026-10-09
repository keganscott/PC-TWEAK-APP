import type { GameGpuChoice } from "../../generated/GameGpuChoice";
import type { GameInstall } from "../../generated/GameInstall";
import type { GameReadiness } from "../../generated/GameReadiness";
import type { GpuPreference } from "../../generated/GpuPreference";
import type { Probe } from "../../generated/Probe";
import type { SecurityFeature } from "../../generated/SecurityFeature";
import { explain } from "../../lib/errors";
import { GAME_GUIDANCE } from "../../lib/gameGuidance";
import { useActions, useStore, useTechnical } from "../../store/hooks";
import type { Op } from "../../store/store";
import { Callout, Card, ErrorCallout, PageHeader, SampleBadge, Skeleton, StatusBadge, type Tone } from "../ui/primitives";

const FEATURE: Record<SecurityFeature, string> = {
  secure_boot: "Secure Boot",
  tpm: "TPM 2.0",
  iommu: "IOMMU (VT-d / AMD-Vi)",
};

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
  // A card for each featured game, and for the main game when it is from the list.
  const shown = (readiness?.perGame ?? []).filter(
    (g) => g.gameId === target || featured.some((f) => f.id === g.gameId),
  );

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
            <div className="flex flex-wrap gap-2">
              {featured.map((g) => {
                const checked = target === g.id;
                return (
                  <label
                    key={g.id}
                    className={`cursor-pointer rounded-lg border px-3.5 py-2 text-sm has-focus-visible:outline-2 has-focus-visible:outline-offset-2 has-focus-visible:outline-lime ${
                      checked ? "border-violet bg-violet font-bold text-white" : "border-line-strong text-ink-muted hover:bg-surface-2 hover:text-ink"
                    }`}
                  >
                    <input
                      type="radio"
                      name="target-game"
                      className="sr-only"
                      checked={checked}
                      onChange={() => void selectTargetGame(g.id)}
                    />
                    {g.name}
                    {found.has(g.id) && (
                      <>
                        {" "}
                        <span className="ml-0.5 text-xs font-normal opacity-80">on this PC</span>
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
              <select
                id="other-game"
                value={target !== null && !featured.some((g) => g.id === target) ? target : ""}
                onChange={(e) => {
                  if (e.target.value) void selectTargetGame(e.target.value);
                }}
                className={`min-w-60 rounded-md border bg-surface-0 px-3 py-2 text-sm ${
                  target !== null && others.some((g) => g.id === target) ? "border-violet font-bold" : "border-line-strong"
                }`}
              >
                <option value="">More games</option>
                {others.map((g) => (
                  <option key={g.id} value={g.id}>
                    {found.has(g.id) ? `${g.name} (on this PC)` : g.name}
                  </option>
                ))}
              </select>
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
              <dl className="grid gap-4 sm:grid-cols-3">
                {(["secure_boot", "tpm", "iommu"] as const).map((f) => {
                  const probe = f === "secure_boot" ? readiness.secureBoot : f === "tpm" ? readiness.tpm : readiness.iommu;
                  const { tone, label } = probeBadge(probe);
                  return (
                    <div key={f}>
                      <dt className="text-sm text-ink-muted">{FEATURE[f]}</dt>
                      <dd className="mt-1">
                        <StatusBadge tone={tone}>{label}</StatusBadge>
                        {probe.state !== "yes" && <p className="mt-1 text-xs text-ink-faint wrap-anywhere">{probe.reason}</p>}
                      </dd>
                    </div>
                  );
                })}
              </dl>
              <p className="mt-4 text-xs text-ink-faint">
                These are firmware settings. PeakTweaks only reads them and never switches them.
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
                    install={
                      installs && games.find((x) => x.id === g.gameId)?.lookedFor
                        ? (installs.find((i) => i.gameId === g.gameId) ?? null)
                        : undefined
                    }
                    choice={choices.find((c) => c.gameId === g.gameId)}
                    launchOp={launchOps[g.gameId]}
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
  install,
  choice,
  launchOp,
  onLaunch,
  technical,
}: {
  readiness: GameReadiness;
  name: string;
  install: GameInstall | null | undefined;
  choice: GameGpuChoice | undefined;
  launchOp: Op | undefined;
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
  return (
    <Card className="p-4">
      <div className="flex flex-wrap items-center gap-2">
        <h3 className="font-bold">{name}</h3>
        <StatusBadge tone={badge.tone}>{badge.label}</StatusBadge>
      </div>
      {readiness.requires.length > 0 && (
        <p className="mt-2 text-sm text-ink-muted">
          Requires {readiness.requires.map((f) => FEATURE[f]).join(", ")}
          {readiness.scope && ` for ${readiness.scope}`}.
        </p>
      )}
      {s.status === "not_ready" && s.missing.length > 0 && (
        <p className="mt-1 text-sm">Missing: {s.missing.map((f) => FEATURE[f]).join(", ")}.</p>
      )}
      {(s.status === "not_ready" || s.status === "unknown") && s.unresolved.length > 0 && (
        <p className="mt-1 text-sm text-ink-muted">Could not check: {s.unresolved.map((f) => FEATURE[f]).join(", ")}.</p>
      )}
      {readiness.source && <p className="mt-2 text-xs text-ink-faint">Source: {readiness.source}</p>}
      {s.status === "not_ready" && (
        <div className="mt-3">
          <Callout tone="info" title="Turning these on happens in your PC's firmware setup (BIOS/UEFI).">
            Your motherboard manual shows how. PeakTweaks does not change firmware settings.
          </Callout>
        </div>
      )}
      {install !== undefined && (
        <p className="mt-3 text-sm wrap-anywhere">
          {install ? installText(install) : "Not found in the places PeakTweaks looks."}
        </p>
      )}
      {install?.steamApp != null && (
        <div className="mt-3 flex flex-wrap items-center gap-3">
          <button
            type="button"
            onClick={onLaunch}
            disabled={launchOp?.status === "running"}
            className="rounded-lg bg-violet px-4 py-2 text-sm font-bold text-white hover:opacity-90 disabled:opacity-60"
          >
            Play {name}
          </button>
          <p className="text-xs text-ink-faint" role="status">
            {launchOp?.status === "running"
              ? "Asking Steam to start it…"
              : launchOp?.status === "done"
                ? "Steam was asked to start it."
                : "Starts through Steam, as you, without PeakTweaks' administrator rights."}
          </p>
        </div>
      )}
      {launchOp?.status === "failed" && (
        <div className="mt-2">
          <ErrorCallout text={explain(launchOp.error)} technical={technical} />
        </div>
      )}
      {choice && (
        <p className="mt-1 text-sm text-ink-muted">
          Graphics chip: {choice.preference.state === "yes" ? CHOICE[choice.preference.value] : "could not tell"}.
        </p>
      )}
      {guidance && (
        <div className="mt-3 border-t border-line pt-3">
          <h4 className="text-sm font-bold">In the game's own settings</h4>
          <p className="mt-1 text-sm text-ink-muted">{guidance.intro}</p>
          {guidance.steps.length > 0 && (
            <ul className="mt-2 list-disc pl-5 text-sm">
              {guidance.steps.map((step) => (
                <li key={step}>{step}</li>
              ))}
            </ul>
          )}
          {guidance.source && <p className="mt-2 text-xs text-ink-faint">Source: {guidance.source}</p>}
        </div>
      )}
    </Card>
  );
}
