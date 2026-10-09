import { useEffect, useId, useState } from "react";

import type { Language } from "../../generated/Language";
import type { RigClass } from "../../generated/RigClass";
import { explain } from "../../lib/errors";
import { probeValue, RIG_LABEL } from "../../lib/format";
import { useActions, useStore, useTechnical } from "../../store/hooks";
import { Button, Dialog, ErrorCallout } from "../ui/primitives";

export function SettingsDialog({
  open,
  onClose,
  onShowWelcome,
}: {
  open: boolean;
  onClose: () => void;
  onShowWelcome: () => void;
}) {
  const settings = useStore((s) => s.settings);
  const detected = useStore((s) => probeValue(s.audit?.env.hardware?.rigClass));
  const settingsOp = useStore((s) => s.settingsOp);
  const technical = useTechnical();
  const { saveSettings } = useActions();
  const [language, setLanguage] = useState<Language>("plain");
  const [rig, setRig] = useState<RigClass | "">("");
  const [reminders, setReminders] = useState(true);
  const rigId = useId();

  // Start from what is stored each time the dialog opens.
  useEffect(() => {
    if (open && settings) {
      setLanguage(settings.language);
      setRig(settings.rigClassOverride ?? "");
      setReminders(!settings.remindersOff);
    }
  }, [open, settings]);

  const save = async () => {
    // Stay open on failure so the error is seen next to what was chosen.
    // Only what this dialog edits changes; anything else stored is kept.
    if (settings && (await saveSettings({ ...settings, language, rigClassOverride: rig || null, remindersOff: !reminders }))) onClose();
  };

  return (
    <Dialog
      open={open}
      title="Settings"
      onClose={onClose}
      footer={
        <>
          <Button variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          <Button variant="primary" busy={settingsOp.status === "running"} onClick={() => void save()}>
            Save
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-5 text-ink">
        <fieldset>
          <legend className="font-medium">Wording</legend>
          <div className="mt-2 flex flex-col gap-2 text-sm">
            {(
              [
                ["plain", "Plain", "Everyday words."],
                ["technical", "Technical", "Also shows registry paths and the engine's own error details."],
              ] as const
            ).map(([value, label, hint]) => (
              <label key={value} className="flex cursor-pointer items-start gap-2">
                <input
                  type="radio"
                  name="language"
                  checked={language === value}
                  onChange={() => setLanguage(value)}
                  className="mt-1 accent-accent"
                />
                <span>
                  {label}
                  <span className="block text-xs text-ink-faint">{hint}</span>
                </span>
              </label>
            ))}
          </div>
        </fieldset>
        <div>
          <label htmlFor={rigId} className="font-medium">
            PC class
          </label>
          <select
            id={rigId}
            value={rig}
            onChange={(e) => setRig(e.target.value as RigClass | "")}
            className="mt-2 w-full rounded-md border border-line bg-surface-0 px-3 py-2 text-sm"
          >
            <option value="">Automatic{detected ? ` (detected: ${RIG_LABEL[detected]})` : ""}</option>
            <option value="low">Low</option>
            <option value="mid">Mid</option>
            <option value="high">High</option>
          </select>
          <p className="mt-1 text-xs text-ink-faint">
            Changes defaults and wording only. It never turns off a safety check.
          </p>
        </div>
        <label className="flex cursor-pointer items-start gap-2 text-sm">
          <input
            type="checkbox"
            checked={reminders}
            onChange={(e) => setReminders(e.target.checked)}
            className="mt-1 accent-accent"
          />
          <span>
            <span className="font-medium">Gentle reminders on Home</span>
            <span className="block text-xs text-ink-faint">
              When junk files were last cleared a month ago or more, and when a graphics driver is more than six months old.
            </span>
          </span>
        </label>
        <div>
          <Button variant="ghost" onClick={onShowWelcome}>
            Show the welcome again
          </Button>
        </div>
        {settingsOp.status === "failed" && <ErrorCallout text={explain(settingsOp.error)} technical={technical} />}
      </div>
    </Dialog>
  );
}
