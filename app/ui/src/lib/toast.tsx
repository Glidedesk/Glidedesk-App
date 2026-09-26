// Toast notifications + a helper that runs an action and reports failures.
import { createContext, useCallback, useContext, useMemo, useState, type ReactNode } from "react";
import { Icon } from "../components/icons";
import { errorText } from "./api";

type Tone = "success" | "error" | "info";
interface Toast {
  id: number;
  tone: Tone;
  text: string;
}

interface ToastCtx {
  show: (text: string, tone?: Tone) => void;
  /** Runs `f`; shows `ok` on success and the error text on failure. */
  run: (f: () => Promise<unknown>, ok?: string) => Promise<boolean>;
}

const Ctx = createContext<ToastCtx | null>(null);
let nextId = 1;

export function ToastProvider({ children }: { children: ReactNode }) {
  const [toasts, setToasts] = useState<Toast[]>([]);
  const dismiss = useCallback((id: number) => setToasts((t) => t.filter((x) => x.id !== id)), []);
  const show = useCallback(
    (text: string, tone: Tone = "info") => {
      const id = nextId++;
      setToasts((t) => [...t.slice(-3), { id, tone, text }]);
      setTimeout(() => dismiss(id), tone === "error" ? 7000 : 3500);
    },
    [dismiss],
  );
  const run = useCallback(
    async (f: () => Promise<unknown>, ok?: string) => {
      try {
        await f();
        if (ok) show(ok, "success");
        return true;
      } catch (e) {
        show(errorText(e), "error");
        return false;
      }
    },
    [show],
  );
  const value = useMemo(() => ({ show, run }), [show, run]);
  return (
    <Ctx.Provider value={value}>
      {children}
      <div className="pointer-events-none fixed right-4 bottom-4 z-50 flex w-[360px] flex-col gap-2" aria-live="polite" aria-atomic="false">
        {toasts.map((t) => {
          const I = t.tone === "success" ? Icon.Check : t.tone === "error" ? Icon.Error : Icon.Info;
          const color = t.tone === "success" ? "text-ok" : t.tone === "error" ? "text-bad" : "text-accent";
          return (
            <div key={t.id} role={t.tone === "error" ? "alert" : "status"} className="toast-in pointer-events-auto flex items-start gap-3 rounded-xl border border-line bg-panel px-4 py-3 shadow-lg">
              <I className={`mt-0.5 shrink-0 ${color}`} />
              <div className="min-w-0 flex-1 leading-snug">{t.text}</div>
              <button type="button" aria-label="Dismiss" className="shrink-0 rounded text-muted hover:text-fg" onClick={() => dismiss(t.id)}>
                <Icon.Close size={16} />
              </button>
            </div>
          );
        })}
      </div>
    </Ctx.Provider>
  );
}

export function useToast(): ToastCtx {
  const c = useContext(Ctx);
  if (!c) throw new Error("useToast outside ToastProvider");
  return c;
}
