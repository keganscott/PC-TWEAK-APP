import { useEffect, useState } from "react";

import { Button, Dialog } from "../ui/primitives";

/** The first-run welcome: what PeakTweaks does and never does, the safety net,
 * and where the scan is. Shown until it is closed once (`welcomeSeen`), and on
 * request from Settings. Every sentence here describes what the code does. */
const STEPS: { title: string; body: string[] }[] = [
  {
    title: "What PeakTweaks does",
    body: [
      "PeakTweaks checks this PC and points out settings worth a look for gaming.",
      "Every change it makes is written down first, with a copy of what was there before, so it can be undone one at a time or all together from Backups.",
      "It never changes BIOS or firmware settings, never touches game files or anti-cheat, and collects no data.",
    ],
  },
  {
    title: "Your safety net",
    body: [
      "Windows asked for permission to run PeakTweaks as administrator because the settings it changes need it.",
      "Before the first change, PeakTweaks asks Windows for a restore point, so the whole PC can be put back the way it is now. You start that from Home.",
    ],
  },
  {
    title: "Your scan",
    body: [
      "Home lists what PeakTweaks found on this PC, grouped by who can act on it: PeakTweaks, you, or new hardware. Things that are already good are listed too.",
      "Games shows what your games need from this PC, and Proof can measure a game before and after a change.",
    ],
  },
];

export function WelcomeDialog({ open, onClose }: { open: boolean; onClose: () => void }) {
  const [step, setStep] = useState(0);
  useEffect(() => {
    if (open) setStep(0);
  }, [open]);

  const last = step === STEPS.length - 1;
  const { title, body } = STEPS[step]!;
  return (
    <Dialog
      open={open}
      title={`Welcome to PeakTweaks: ${title}`}
      onClose={onClose}
      footer={
        <>
          <span className="mr-auto text-xs text-ink-faint" aria-live="polite">
            Step {step + 1} of {STEPS.length}
          </span>
          {step > 0 && (
            <Button variant="ghost" onClick={() => setStep(step - 1)}>
              Back
            </Button>
          )}
          {last ? (
            <Button variant="primary" onClick={onClose}>
              Get started
            </Button>
          ) : (
            <Button variant="primary" onClick={() => setStep(step + 1)}>
              Next
            </Button>
          )}
        </>
      }
    >
      <div className="flex flex-col gap-3 text-sm text-ink">
        {body.map((p) => (
          <p key={p}>{p}</p>
        ))}
      </div>
    </Dialog>
  );
}
