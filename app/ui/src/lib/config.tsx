// The editable config, shared by every page through one provider.
// Edits are debounced; a newer reload/commit always wins over older work.
import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { api, errorText } from "./api";
import type { Config } from "./types";

type Update = (mutate: (c: Config) => void, immediate?: boolean) => void;

interface ConfigCtx {
  config: Config | null;
  update: Update;
  /** Replace the whole config now (import, wizard) — cancels pending edits. */
  commit: (c: Config) => Promise<void>;
  reload: () => Promise<void>;
  saving: boolean;
  savedAt: number | null;
  error: string | null;
}

const Ctx = createContext<ConfigCtx | null>(null);

export function ConfigProvider({ children }: { children: ReactNode }) {
  const [config, setConfig] = useState<Config | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [savedAt, setSavedAt] = useState<number | null>(null);
  const pending = useRef<Config | null>(null);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  // Every reload/commit/flush takes a ticket; stale results are ignored.
  const gen = useRef(0);

  const cancelPending = () => {
    if (timer.current) clearTimeout(timer.current);
    timer.current = null;
    pending.current = null;
  };

  const reload = useCallback(async () => {
    cancelPending();
    const ticket = ++gen.current;
    try {
      const c = await api.getConfig();
      if (ticket === gen.current) {
        setConfig(c);
        setError(null);
      }
    } catch (e) {
      if (ticket === gen.current) setError(errorText(e));
    }
  }, []);

  useEffect(() => {
    void reload();
  }, [reload]);

  const flush = useCallback(async () => {
    const next = pending.current;
    pending.current = null;
    timer.current = null;
    if (!next) return;
    const ticket = ++gen.current;
    setSaving(true);
    try {
      await api.setConfig(next);
      if (ticket === gen.current) {
        setError(null);
        setSavedAt(Date.now());
      }
    } catch (e) {
      if (ticket === gen.current) {
        setError(errorText(e));
        void reload();
      }
    } finally {
      setSaving(false);
    }
  }, [reload]);

  const update = useCallback<Update>(
    (mutate, immediate = false) => {
      setConfig((prev) => {
        if (!prev) return prev;
        const next = structuredClone(prev);
        mutate(next);
        pending.current = next;
        return next;
      });
      if (timer.current) clearTimeout(timer.current);
      timer.current = setTimeout(() => void flush(), immediate ? 0 : 400);
    },
    [flush],
  );

  const commit = useCallback(
    async (c: Config) => {
      cancelPending();
      const ticket = ++gen.current;
      setSaving(true);
      try {
        await api.setConfig(c);
        if (ticket === gen.current) {
          setConfig(c);
          setSavedAt(Date.now());
          setError(null);
        }
      } finally {
        setSaving(false);
      }
      await reload();
    },
    [reload],
  );

  // Save anything still pending when the window closes.
  useEffect(() => {
    const onHide = () => void flush();
    window.addEventListener("beforeunload", onHide);
    return () => window.removeEventListener("beforeunload", onHide);
  }, [flush]);

  const value = useMemo(() => ({ config, update, commit, reload, saving, savedAt, error }), [config, update, commit, reload, saving, savedAt, error]);
  return <Ctx.Provider value={value}>{children}</Ctx.Provider>;
}

export function useConfig(): ConfigCtx {
  const c = useContext(Ctx);
  if (!c) throw new Error("useConfig outside ConfigProvider");
  return c;
}
