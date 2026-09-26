// The editable config, shared by every page through one provider.
// Edits are debounced; a newer reload/commit always wins over older work.
import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { api, errorText, onConfigChanged } from "./api";
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
  // Edits behind `pending`, so they can be re-applied on top of a reloaded config.
  const replay = useRef<((c: Config) => void)[]>([]);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  // Every reload/commit/flush takes a ticket; stale results are ignored.
  const gen = useRef(0);

  const cancelPending = () => {
    if (timer.current) clearTimeout(timer.current);
    timer.current = null;
    pending.current = null;
    replay.current = [];
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

  // The agent changed settings by itself (a computer joined, reset, reload):
  // take its version and re-apply every edit not saved yet — including edits
  // made while the fresh copy was loading.
  const changedGen = useRef(0);
  const inFlight = useRef(0);
  const refetchAfterSave = useRef(false);
  const refreshRef = useRef<() => void>(() => {});
  useEffect(() => {
    const refresh = () => {
      const mine = ++changedGen.current;
      void api.getConfig().then(
        (fresh) => {
          if (mine !== changedGen.current) return; // a newer event is loading
          if (inFlight.current > 0) {
            // A save is on its way; this copy may predate it. Look again after it.
            refetchAfterSave.current = true;
            return;
          }
          const edits = pending.current ? replay.current.slice() : [];
          if (edits.length === 0) {
            setConfig(fresh);
            return;
          }
          if (timer.current) clearTimeout(timer.current);
          const next = structuredClone(fresh);
          for (const m of edits) m(next);
          setConfig(next);
          pending.current = next;
          timer.current = setTimeout(() => void flushRef.current(), 0);
        },
        (e) => setError(errorText(e)),
      );
    };
    refreshRef.current = refresh;
    const un = onConfigChanged(refresh);
    return () => void un.then((f) => f());
  }, []);

  const flush = useCallback(async () => {
    const next = pending.current;
    pending.current = null;
    replay.current = [];
    timer.current = null;
    if (!next) return;
    const ticket = ++gen.current;
    setSaving(true);
    inFlight.current += 1;
    try {
      await api.setConfig(next);
      if (ticket === gen.current) {
        setError(null);
        setSavedAt(Date.now());
      }
    } catch (e) {
      if (ticket === gen.current) {
        setError(errorText(e));
        // Show what is really saved — unless the user already made a newer edit,
        // which must not be thrown away (it is saved on its own timer).
        if (!pending.current) void reload();
      }
    } finally {
      setSaving(false);
      inFlight.current -= 1;
      if (inFlight.current === 0 && refetchAfterSave.current) {
        refetchAfterSave.current = false;
        refreshRef.current();
      }
    }
  }, [reload]);

  const flushRef = useRef(flush);
  flushRef.current = flush;

  const update = useCallback<Update>(
    (mutate, immediate = false) => {
      replay.current.push(mutate);
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
