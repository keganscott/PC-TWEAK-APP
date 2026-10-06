import {
  useEffect,
  useId,
  useRef,
  type ButtonHTMLAttributes,
  type ReactNode,
} from "react";
import { AlertTriangle, CheckCircle2, CircleHelp, Info, Loader2, OctagonX, X } from "lucide-react";

import type { ErrorText } from "../../lib/errors";

const cx = (...parts: (string | false | null | undefined)[]) => parts.filter(Boolean).join(" ");

// ---------------------------------------------------------------------------
// Button
// ---------------------------------------------------------------------------

/** primary: violet, the usual action. go: lime, the one main step on a
 * violet card. secondary: outlined. ghost: text only. danger: undo-everything. */
type Variant = "primary" | "go" | "secondary" | "ghost" | "danger";

const VARIANT: Record<Variant, string> = {
  primary: "bg-violet text-white hover:bg-violet-strong disabled:bg-surface-3 disabled:text-ink-faint",
  go: "bg-lime text-black hover:bg-lime-strong disabled:bg-black/30 disabled:text-white/70",
  secondary: "border border-line-strong text-ink hover:bg-surface-2 disabled:text-ink-faint",
  ghost: "text-ink-muted hover:text-ink hover:bg-surface-2 disabled:text-ink-faint",
  danger: "bg-bad/10 text-bad border border-bad/40 hover:bg-bad/20 disabled:border-line disabled:bg-transparent disabled:text-ink-faint",
};

export interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: Variant;
  /** Shows a spinner, disables the button and tells screen readers it is busy. */
  busy?: boolean;
  icon?: ReactNode;
}

export function Button({ variant = "secondary", busy = false, icon, className, children, disabled, ...rest }: ButtonProps) {
  return (
    <button
      type="button"
      {...rest}
      disabled={disabled || busy}
      aria-busy={busy || undefined}
      className={cx(
        "inline-flex items-center justify-center gap-2 rounded-lg px-4 py-2 text-sm font-bold",
        "transition-colors disabled:cursor-not-allowed",
        VARIANT[variant],
        className,
      )}
    >
      {busy ? <Loader2 aria-hidden className="size-4 animate-spin" /> : icon}
      {children}
    </button>
  );
}

// ---------------------------------------------------------------------------
// Surfaces
// ---------------------------------------------------------------------------

export function Card({ className, children, ...rest }: { className?: string; children: ReactNode } & React.HTMLAttributes<HTMLElement>) {
  return (
    <section {...rest} className={cx("rounded-2xl border border-line bg-surface-1 p-5", className)}>
      {children}
    </section>
  );
}

export function PageHeader({ title, description, actions }: { title: string; description?: ReactNode; actions?: ReactNode }) {
  return (
    <header className="mb-7 flex flex-wrap items-end justify-between gap-4">
      <div>
        <h1 className="text-3xl font-extrabold tracking-tight">{title}</h1>
        {description && <p className="mt-2 max-w-2xl text-sm text-ink-muted">{description}</p>}
      </div>
      {actions && <div className="flex items-center gap-2">{actions}</div>}
    </header>
  );
}

export function Skeleton({ className, label = "Loading" }: { className?: string; label?: string }) {
  return (
    <div role="status" aria-label={label} className={cx("animate-pulse rounded-xl bg-surface-2", className)} />
  );
}

export function Spinner({ label }: { label: string }) {
  return (
    <span role="status" className="inline-flex items-center gap-2 text-sm text-ink-muted">
      <Loader2 aria-hidden className="size-4 animate-spin" />
      {label}
    </span>
  );
}

// ---------------------------------------------------------------------------
// Status: never colour alone. Every tone has an icon and the caller's word.
// ---------------------------------------------------------------------------

export type Tone = "ok" | "warn" | "bad" | "info" | "neutral";

/** badge: the chip. edge/icon: how a Callout of that tone is drawn. */
const TONE: Record<Tone, { badge: string; edge: string; icon: string; Icon: typeof Info }> = {
  ok: { badge: "border-ok bg-ok text-black", edge: "border-ok/35", icon: "text-ok", Icon: CheckCircle2 },
  warn: { badge: "border-violet bg-violet text-white", edge: "border-violet/70", icon: "text-violet-soft", Icon: AlertTriangle },
  bad: { badge: "border-bad/50 bg-bad/10 text-bad", edge: "border-bad/50", icon: "text-bad", Icon: OctagonX },
  info: { badge: "border-line-strong text-info", edge: "border-line-strong", icon: "text-info", Icon: Info },
  neutral: { badge: "border-line-strong text-ink-muted", edge: "border-line", icon: "text-ink-muted", Icon: CircleHelp },
};

export function StatusBadge({ tone, children }: { tone: Tone; children: ReactNode }) {
  const { badge, Icon } = TONE[tone];
  return (
    <span className={cx("inline-flex items-center gap-1.5 rounded-md border px-2 py-0.5 text-xs font-bold", badge)}>
      <Icon aria-hidden className="size-3.5" />
      {children}
    </span>
  );
}

export function Callout({ tone, title, children, action }: { tone: Tone; title: ReactNode; children?: ReactNode; action?: ReactNode }) {
  const { edge, icon, Icon } = TONE[tone];
  return (
    <div role={tone === "bad" ? "alert" : "note"} className={cx("flex gap-3 rounded-2xl border bg-surface-1 p-4", edge)}>
      <Icon aria-hidden className={cx("mt-0.5 size-5 shrink-0", icon)} />
      <div className="min-w-0 flex-1 text-ink">
        <p className="font-bold">{title}</p>
        {children && <div className="mt-1 text-sm text-ink-muted">{children}</div>}
        {action && <div className="mt-3">{action}</div>}
      </div>
    </div>
  );
}

/** An engine error, in plain words; the engine's detail only in the technical view. */
export function ErrorCallout({ text, technical, action }: { text: ErrorText; technical: boolean; action?: ReactNode }) {
  return (
    <Callout tone="bad" title={text.title} action={action}>
      {text.hint && <p>{text.hint}</p>}
      {technical && text.detail && <p className="mt-1 break-all font-mono text-xs">{text.detail}</p>}
    </Callout>
  );
}

// ---------------------------------------------------------------------------
// SAMPLE: anything the mock shows is labelled (R21, plan section 7)
// ---------------------------------------------------------------------------

export function SampleBadge() {
  return (
    <span className="rounded border border-sample/50 bg-sample/10 px-1.5 py-0.5 text-[10px] font-bold tracking-wider text-sample">
      SAMPLE
    </span>
  );
}

// ---------------------------------------------------------------------------
// Dialog: focus trapped inside, Esc closes, focus returns to the opener.
// ---------------------------------------------------------------------------

const FOCUSABLE = 'a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

export function Dialog({
  open,
  title,
  onClose,
  children,
  footer,
}: {
  open: boolean;
  title: string;
  onClose: () => void;
  children: ReactNode;
  footer?: ReactNode;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const titleId = useId();
  // Kept in a ref so a parent re-render (a new onClose function) does not
  // re-run the effect and pull focus back to the first control.
  const onCloseRef = useRef(onClose);
  useEffect(() => {
    onCloseRef.current = onClose;
  }, [onClose]);

  useEffect(() => {
    if (!open) return;
    const opener = document.activeElement as HTMLElement | null;
    const node = ref.current;
    const items = () => Array.from(node?.querySelectorAll<HTMLElement>(FOCUSABLE) ?? []);
    (items()[0] ?? node)?.focus();

    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        onCloseRef.current();
        return;
      }
      if (e.key !== "Tab") return;
      const list = items();
      if (list.length === 0) {
        e.preventDefault();
        return;
      }
      const first = list[0]!;
      const last = list[list.length - 1]!;
      if (e.shiftKey && document.activeElement === first) {
        e.preventDefault();
        last.focus();
      } else if (!e.shiftKey && document.activeElement === last) {
        e.preventDefault();
        first.focus();
      }
    };
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("keydown", onKey);
      opener?.focus();
    };
  }, [open]);

  if (!open) return null;
  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/75 p-4">
      <div
        ref={ref}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        tabIndex={-1}
        className="w-full max-w-lg rounded-2xl border border-line-strong bg-surface-1"
      >
        <div className="flex items-center justify-between border-b border-line px-6 py-4">
          <h2 id={titleId} className="text-lg font-extrabold tracking-tight">
            {title}
          </h2>
          <button type="button" onClick={onClose} aria-label="Close" className="rounded-md p-1 text-ink-muted hover:bg-surface-2 hover:text-ink">
            <X aria-hidden className="size-5" />
          </button>
        </div>
        <div className="px-6 py-5 text-sm text-ink-muted">{children}</div>
        {footer && <div className="flex items-center justify-end gap-2 border-t border-line px-6 py-4">{footer}</div>}
      </div>
    </div>
  );
}

export { cx };
