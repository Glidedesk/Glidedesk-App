// Small accessible building blocks. Native elements first (a11y for free,
// tiny bundle); roles/ARIA only where native elements do not exist.
import { useEffect, useId, useRef, type ReactNode } from "react";
import { Icon } from "./icons";

export function Page({ title, subtitle, actions, children }: { title: string; subtitle?: string; actions?: ReactNode; children: ReactNode }) {
  return (
    <main className="flex h-full min-w-0 flex-1 flex-col overflow-hidden">
      <header data-tauri-drag-region className="flex flex-wrap items-end justify-between gap-3 border-b border-line bg-panel/40 px-4 pt-5 pb-4 md:px-8 md:pt-7 md:pb-5">
        <div className="min-w-0 flex-1 basis-64" data-tauri-drag-region>
          <h1 className="text-[20px] font-semibold tracking-tight md:text-[22px]">{title}</h1>
          {subtitle && <p className="mt-1 text-muted">{subtitle}</p>}
        </div>
        {actions && <div className="flex flex-wrap items-center gap-2">{actions}</div>}
      </header>
      <div className="page-in flex-1 overflow-y-auto px-4 py-5 md:px-8 md:py-6">{children}</div>
    </main>
  );
}

export function Section({ title, description, children }: { title: string; description?: string; children: ReactNode }) {
  return (
    <section className="mb-6">
      <h2 className="mb-1 text-[13px] font-semibold uppercase tracking-wide text-muted">{title}</h2>
      {description && <p className="mb-2 text-muted">{description}</p>}
      <div className="divide-y divide-line overflow-hidden rounded-xl border border-line bg-panel">{children}</div>
    </section>
  );
}

export function Row({ label, hint, children, htmlFor }: { label: ReactNode; hint?: ReactNode; children?: ReactNode; htmlFor?: string }) {
  return (
    <div className="flex flex-col gap-2 px-4 py-3 sm:flex-row sm:items-center sm:justify-between sm:gap-6">
      <div className="min-w-0">
        <label htmlFor={htmlFor} className="block font-medium">
          {label}
        </label>
        {hint && <div className="mt-0.5 text-[12.5px] leading-snug text-muted">{hint}</div>}
      </div>
      {children != null && <div className="flex max-w-full flex-wrap items-center gap-2 sm:shrink-0 sm:justify-end">{children}</div>}
    </div>
  );
}

export function Switch({ checked, onChange, label, disabled }: { checked: boolean; onChange: (v: boolean) => void; label: string; disabled?: boolean }) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={label}
      disabled={disabled}
      onClick={() => onChange(!checked)}
      className={`relative h-[22px] w-[38px] rounded-full transition-colors disabled:opacity-40 ${checked ? "bg-accent" : "bg-line"}`}
    >
      <span className={`absolute top-[3px] h-4 w-4 rounded-full bg-white shadow transition-all ${checked ? "left-[19px]" : "left-[3px]"}`} />
    </button>
  );
}

export function Button({
  children,
  onClick,
  variant = "default",
  disabled,
  title,
  type = "button",
  icon,
  autoFocus,
}: {
  children: ReactNode;
  onClick?: () => void;
  variant?: "default" | "primary" | "danger" | "ghost";
  disabled?: boolean;
  title?: string;
  type?: "button" | "submit";
  icon?: ReactNode;
  autoFocus?: boolean;
}) {
  const styles = {
    default: "border border-line bg-panel shadow-xs hover:bg-panel-2",
    primary: "bg-accent text-white shadow-sm hover:brightness-110",
    danger: "border border-line bg-panel text-bad shadow-xs hover:bg-bad/10",
    ghost: "text-fg/80 hover:bg-panel-2 hover:text-fg",
  }[variant];
  return (
    <button
      type={type}
      title={title}
      disabled={disabled}
      onClick={onClick}
      autoFocus={autoFocus}
      className={`inline-flex items-center gap-1.5 rounded-lg px-3 py-1.5 font-medium transition focus-visible:ring-2 focus-visible:ring-accent focus-visible:ring-offset-1 focus-visible:ring-offset-bg focus-visible:outline-none active:scale-[0.98] disabled:pointer-events-none disabled:opacity-40 ${styles}`}
    >
      {icon}
      {children}
    </button>
  );
}

export function Card({ children, className = "" }: { children: ReactNode; className?: string }) {
  return <div className={`rounded-2xl border border-line bg-panel shadow-xs ${className}`}>{children}</div>;
}

export function EmptyState({ icon, title, children }: { icon: ReactNode; title: string; children?: ReactNode }) {
  return (
    <div className="flex flex-col items-center justify-center rounded-2xl border border-dashed border-line bg-panel/50 px-8 py-14 text-center">
      <div className="mb-3 flex h-12 w-12 items-center justify-center rounded-2xl bg-accent-soft text-accent">{icon}</div>
      <div className="text-[15px] font-semibold">{title}</div>
      {children && <div className="mt-1 max-w-md text-muted">{children}</div>}
    </div>
  );
}

export function Spinner({ size = 18 }: { size?: number }) {
  return (
    <svg className="animate-spin text-accent" width={size} height={size} viewBox="0 0 24 24" aria-hidden="true">
      <circle cx="12" cy="12" r="9" stroke="currentColor" strokeOpacity="0.2" strokeWidth="3" fill="none" />
      <path d="M21 12a9 9 0 0 0-9-9" stroke="currentColor" strokeWidth="3" fill="none" strokeLinecap="round" />
    </svg>
  );
}

export function Badge({ tone, children }: { tone: "ok" | "warn" | "muted" | "busy" | "bad" | "accent"; children: ReactNode }) {
  const t = {
    ok: "bg-ok/15 text-ok",
    warn: "bg-warn/15 text-warn",
    muted: "bg-panel-2 text-muted",
    busy: "bg-accent-soft text-accent",
    bad: "bg-bad/15 text-bad",
    accent: "bg-accent-soft text-accent",
  }[tone];
  return <span className={`inline-flex items-center gap-1 rounded-full px-2 py-0.5 text-[12px] font-medium ${t}`}>{children}</span>;
}

export function Dot({ tone }: { tone: "ok" | "warn" | "muted" | "busy" | "bad" }) {
  const c = { ok: "bg-ok", warn: "bg-warn", muted: "bg-muted/60", busy: "bg-accent animate-pulse", bad: "bg-bad" }[tone];
  return <span aria-hidden className={`inline-block h-2 w-2 rounded-full ${c}`} />;
}

export function NumberInput({
  value,
  onChange,
  min,
  max,
  step = 1,
  unit,
  label,
  width = "w-24",
}: {
  value: number;
  onChange: (v: number) => void;
  min: number;
  max: number;
  step?: number;
  unit?: string;
  label: string;
  width?: string;
}) {
  return (
    <span className="inline-flex items-center gap-1.5">
      <input
        aria-label={label}
        type="number"
        className={`${width} rounded-lg border border-line bg-panel px-2 py-1 text-right tabular-nums`}
        value={Number.isFinite(value) ? value : 0}
        min={min}
        max={max}
        step={step}
        onChange={(e) => {
          const v = Number(e.target.value);
          if (Number.isFinite(v)) onChange(Math.min(max, Math.max(min, v)));
        }}
      />
      {unit && <span className="text-muted">{unit}</span>}
    </span>
  );
}

export function Slider({ value, onChange, min, max, step, label, format }: { value: number; onChange: (v: number) => void; min: number; max: number; step: number; label: string; format?: (v: number) => string }) {
  return (
    <span className="inline-flex items-center gap-2">
      <input aria-label={label} type="range" className="w-40 accent-accent" value={value} min={min} max={max} step={step} onChange={(e) => onChange(Number(e.target.value))} />
      <span className="w-12 text-right tabular-nums text-muted">{format ? format(value) : value}</span>
    </span>
  );
}

export function Select<T extends string>({ value, onChange, options, label }: { value: T; onChange: (v: T) => void; options: [T, string][]; label: string }) {
  return (
    <select aria-label={label} className="max-w-full rounded-lg border border-line bg-panel px-2 py-1" value={value} onChange={(e) => onChange(e.target.value as T)}>
      {options.map(([v, l]) => (
        <option key={v} value={v}>
          {l}
        </option>
      ))}
    </select>
  );
}

export function TextInput({
  value,
  onChange,
  placeholder,
  label,
  width = "w-64",
  invalid,
  type = "text",
  autoComplete,
  describedBy,
}: {
  value: string;
  onChange: (v: string) => void;
  placeholder?: string;
  label: string;
  width?: string;
  invalid?: boolean;
  type?: "text" | "password";
  /** For password fields: "new-password" (setting one) or "current-password" (entering one). */
  autoComplete?: "new-password" | "current-password";
  describedBy?: string;
}) {
  return (
    <input
      type={type}
      autoComplete={autoComplete}
      aria-describedby={describedBy}
      aria-label={label}
      aria-invalid={invalid || undefined}
      className={`${width} max-w-full rounded-lg border bg-panel px-2 py-1 ${invalid ? "border-bad" : "border-line"}`}
      value={value}
      placeholder={placeholder}
      spellCheck={false}
      onChange={(e) => onChange(e.target.value)}
    />
  );
}

export function Modal({ open, onClose, title, children, footer }: { open: boolean; onClose: () => void; title: string; children: ReactNode; footer?: ReactNode }) {
  const ref = useRef<HTMLDialogElement>(null);
  const opener = useRef<Element | null>(null);
  const id = useId();
  useEffect(() => {
    const d = ref.current;
    if (!d) return;
    if (open && !d.open) {
      opener.current = document.activeElement;
      d.showModal();
      // Focus the primary action (last footer button) rather than the first control.
      const target = d.querySelector<HTMLElement>("[autofocus], [data-autofocus]") ?? d.querySelector<HTMLElement>("footer button:last-child");
      target?.focus();
    }
    if (!open && d.open) {
      d.close();
      if (opener.current instanceof HTMLElement) opener.current.focus();
    }
  }, [open]);
  return (
    <dialog ref={ref} aria-labelledby={id} onClose={onClose} className="m-auto w-[560px] max-w-[92vw] rounded-2xl border border-line bg-panel p-0 text-fg shadow-2xl backdrop:bg-black/40">
      <div className="border-b border-line px-5 py-3.5">
        <h2 id={id} className="text-[15px] font-semibold">
          {title}
        </h2>
      </div>
      <div className="max-h-[60vh] overflow-y-auto px-3 py-4 sm:px-5">{children}</div>
      {footer && <footer className="flex flex-wrap justify-end gap-2 border-t border-line px-3 py-3 sm:px-5">{footer}</footer>}
    </dialog>
  );
}

export function Callout({ tone, children }: { tone: "warn" | "bad" | "info"; children: ReactNode }) {
  const t = { warn: "border-warn/40 bg-warn/10", bad: "border-bad/40 bg-bad/10", info: "border-accent/25 bg-accent-soft" }[tone];
  const I = { warn: Icon.Warning, bad: Icon.Error, info: Icon.Info }[tone];
  const c = { warn: "text-warn", bad: "text-bad", info: "text-accent" }[tone];
  return (
    <div className={`mb-5 flex gap-3 rounded-xl border px-4 py-3 ${t}`}>
      <I className={`mt-0.5 shrink-0 ${c}`} />
      <div className="min-w-0 flex-1 leading-relaxed">{children}</div>
    </div>
  );
}
