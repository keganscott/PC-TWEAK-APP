import { useCallback, useEffect, useState } from "react";
import { RotateCcw } from "lucide-react";

import { explain } from "./lib/errors";
import { useActions, useStore } from "./store/hooks";
import { Logo } from "./components/brand/Logo";
import { AppShell } from "./components/shell/AppShell";
import type { ViewId } from "./components/shell/nav";
import { Button, ErrorCallout, Skeleton } from "./components/ui/primitives";
import { BackupsView } from "./components/views/BackupsView";
import { GamesView } from "./components/views/GamesView";
import { HomeView } from "./components/views/HomeView";
import { ProofView } from "./components/views/ProofView";
import { SettingsDialog } from "./components/views/SettingsDialog";
import { ToolsView } from "./components/views/ToolsView";
import { WelcomeDialog } from "./components/views/WelcomeDialog";

const VIEW: Record<ViewId, () => React.JSX.Element> = {
  home: HomeView,
  games: GamesView,
  tools: ToolsView,
  proof: ProofView,
  backups: BackupsView,
};

export function App() {
  const boot = useStore((s) => s.boot);
  const settings = useStore((s) => s.settings);
  const { boot: start, saveSettings } = useActions();
  const [view, setView] = useState<ViewId>("home");
  const [settingsOpen, setSettingsOpen] = useState(false);
  const openSettings = useCallback(() => setSettingsOpen(true), []);
  const closeSettings = useCallback(() => setSettingsOpen(false), []);
  // First launch: the welcome shows until it is closed once. Closing it is
  // remembered by the engine; if that save fails it simply shows next time.
  const [welcomeClosed, setWelcomeClosed] = useState(false);
  const [welcomeAgain, setWelcomeAgain] = useState(false);
  const welcomeOpen = welcomeAgain || (settings !== null && !settings.welcomeSeen && !welcomeClosed);
  const closeWelcome = useCallback(() => {
    setWelcomeClosed(true);
    setWelcomeAgain(false);
    if (settings && !settings.welcomeSeen) void saveSettings({ ...settings, welcomeSeen: true });
  }, [settings, saveSettings]);
  const showWelcome = useCallback(() => {
    setSettingsOpen(false);
    setWelcomeAgain(true);
  }, []);

  useEffect(() => {
    void start();
  }, [start]);

  if (boot.status === "loading") return <BootScreen />;
  if (boot.status === "failed") {
    return (
      <div className="mx-auto flex h-full max-w-xl flex-col justify-center gap-4 p-8">
        <Logo />
        <h1 className="text-2xl font-extrabold tracking-tight">PeakTweaks could not start</h1>
        <ErrorCallout
          text={explain(boot.error)}
          technical
          action={
            // Another copy holds the engine; retrying in this window cannot change that.
            boot.error.kind === "already_running" ? undefined : (
              <Button variant="primary" icon={<RotateCcw aria-hidden className="size-4" />} onClick={() => void start()}>
                Try again
              </Button>
            )
          }
        />
        <p className="text-sm text-ink-muted">
          When the engine itself could not start, the same reason is also saved, newest last, in{" "}
          <span className="break-all font-mono text-xs">%LOCALAPPDATA%\PeakTweaks\startup-error.log</span>, for
          support.
        </p>
      </div>
    );
  }

  const View = VIEW[view];
  return (
    <>
      <AppShell view={view} onNavigate={setView} onOpenSettings={openSettings}>
        <View />
      </AppShell>
      <SettingsDialog open={settingsOpen} onClose={closeSettings} onShowWelcome={showWelcome} />
      <WelcomeDialog open={welcomeOpen && !settingsOpen} onClose={closeWelcome} />
    </>
  );
}

function BootScreen() {
  return (
    <div className="flex h-full" aria-busy="true">
      <div className="w-60 border-r border-line px-6 pt-6">
        <Logo />
        <Skeleton className="mt-10 h-6 w-32" label="Starting PeakTweaks" />
      </div>
      <div className="flex flex-1 flex-col gap-4 px-9 pt-16">
        <Skeleton className="h-9 w-80" />
        <Skeleton className="h-56 w-full max-w-[1240px]" />
        <Skeleton className="h-32 w-full max-w-[1240px]" />
      </div>
    </div>
  );
}
