// Dev-only stand-in for the Tauri runtime, so the settings UI can be opened in
// a normal browser against a real agent through the dev bridge
// (app/ui/dev/agent-bridge.ts). Pick the agent with `?agent=<name>`.
// Only loaded by main.tsx in `vite dev` when not running inside the app.

type Cb = (payload: unknown) => void;

export async function installDevShim(): Promise<void> {
  const params = new URLSearchParams(window.location.search);
  const names = (await fetch("/__gd/agents").then((r) => r.json(), () => [])) as string[];
  const agent = params.get("agent") ?? names[0];
  if (!agent) return;
  const base = `/__gd/${agent}`;
  const callbacks = new Map<number, Cb>();
  const listeners = new Map<string, Set<number>>();
  let nextId = 1;
  let status: unknown = null;

  const send = async (req: Record<string, unknown>) => {
    const r = (await fetch(`${base}/request`, { method: "POST", body: JSON.stringify(req) }).then((x) => x.json())) as {
      ok: boolean;
      data?: unknown;
      error?: string;
    };
    if (!r.ok) throw r.error ?? "request failed";
    return r.data ?? null;
  };
  const emit = (event: string, payload: unknown) => {
    for (const id of listeners.get(event) ?? []) callbacks.get(id)?.({ event, id, payload });
  };
  const connect = () => {
    const es = new EventSource(`${base}/events`);
    es.onmessage = (m) => {
      const msg = JSON.parse(m.data as string) as { event: string; data?: unknown };
      if (msg.event === "status") {
        status = msg.data;
        emit("agent-status", msg.data);
      } else if (msg.event === "notice") emit("agent-notice", msg.data);
      else if (msg.event === "config_changed") emit("agent-config", null);
    };
    es.onerror = () => {
      es.close();
      setTimeout(connect, 1000);
    };
  };

  (window as unknown as Record<string, unknown>).__TAURI_EVENT_PLUGIN_INTERNALS__ = {
    unregisterListener: (event: string, id: number) => listeners.get(event)?.delete(id),
  };
  (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {
    transformCallback: (cb: Cb) => {
      const id = nextId++;
      callbacks.set(id, cb);
      return id;
    },
    unregisterCallback: (id: number) => callbacks.delete(id),
    convertFileSrc: (p: string) => p,
    invoke: async (cmd: string, args: Record<string, unknown> = {}) => {
      switch (cmd) {
        case "plugin:event|listen": {
          const { event, handler } = args as { event: string; handler: number };
          if (!listeners.has(event)) listeners.set(event, new Set());
          listeners.get(event)?.add(handler);
          return handler;
        }
        case "plugin:event|unlisten":
          return null;
        case "agent":
          return send(args.request as Record<string, unknown>);
        case "status":
          return status ?? (status = await send({ cmd: "status" }));
        case "request_permissions":
        case "open_external":
          return null;
        case "export_settings":
          return false;
        case "import_settings":
          return null;
        default:
          throw `dev shim: unknown command ${cmd}`;
      }
    },
  };
  connect();
}
