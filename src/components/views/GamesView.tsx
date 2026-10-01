import type { GameReadiness } from "../../generated/GameReadiness";
import type { Probe } from "../../generated/Probe";
import type { SecurityFeature } from "../../generated/SecurityFeature";
import { explain } from "../../lib/errors";
import { useActions, useStore, useTechnical } from "../../store/hooks";
import { Callout, Card, ErrorCallout, PageHeader, Skeleton, StatusBadge, type Tone } from "../ui/primitives";

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
  const { selectTargetGame } = useActions();
  const readiness = audit?.antiCheat ?? null;

  return (
    <>
      <PageHeader
        title="Games"
        description="Pick the game you play most. PeakTweaks then holds back any change that game's anti-cheat would not accept."
      />
      <div className="flex max-w-4xl flex-col gap-5">
        <Card>
          <fieldset>
            <legend className="mb-3 font-semibold">Main game</legend>
            <div className="flex flex-wrap gap-2">
              {[{ id: null as string | null, name: "None" }, ...games].map((g) => {
                const checked = target === g.id;
                return (
                  <label
                    key={g.id ?? "none"}
                    className={`cursor-pointer rounded-md border px-3.5 py-2 text-sm ${
                      checked ? "border-accent bg-accent/10 text-ink" : "border-line text-ink-muted hover:bg-surface-2"
                    }`}
                  >
                    <input
                      type="radio"
                      name="target-game"
                      className="sr-only"
                      checked={checked}
                      disabled={targetOp.status === "running"}
                      onChange={() => void selectTargetGame(g.id)}
                    />
                    {g.name}
                  </label>
                );
              })}
            </div>
          </fieldset>
          {targetOp.status === "failed" && (
            <div className="mt-3">
              <ErrorCallout text={explain(targetOp.error)} technical={technical} />
            </div>
          )}
        </Card>

        <section aria-labelledby="security-title">
          <h2 id="security-title" className="mb-3 text-lg font-semibold">
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
            <h2 id="per-game-title" className="mb-3 text-lg font-semibold">
              Per game
            </h2>
            <ul className="flex flex-col gap-3">
              {readiness.perGame.map((g) => (
                <li key={g.gameId}>
                  <GameCard readiness={g} name={games.find((x) => x.id === g.gameId)?.name ?? g.gameId} />
                </li>
              ))}
            </ul>
          </section>
        )}
      </div>
    </>
  );
}

function GameCard({ readiness, name }: { readiness: GameReadiness; name: string }) {
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
        <h3 className="font-medium">{name}</h3>
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
    </Card>
  );
}
