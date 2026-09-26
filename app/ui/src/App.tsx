import { useEffect, useMemo, useState, type ReactNode } from "react";
import { api, onNavigate, onNotice } from "./lib/api";
import { useStatus } from "./lib/store";
import { ConfigProvider, useConfig } from "./lib/config";
import { ToastProvider, useToast } from "./lib/toast";
import { Icon } from "./components/icons";
import { Callout, Spinner } from "./components/ui";
import { LayoutPage } from "./pages/LayoutPage";
import { ComputersPage } from "./pages/ComputersPage";
import { SharingPage } from "./pages/SharingPage";
import { InputPage } from "./pages/InputPage";
import { NetworkPage } from "./pages/NetworkPage";
import { GeneralPage } from "./pages/GeneralPage";
import { AdvancedPage } from "./pages/AdvancedPage";
import { ClientHome } from "./pages/ClientHome";
import { Wizard } from "./pages/Wizard";
import { Identify } from "./pages/Identify";

type PageId = "layout" | "computers" | "sharing" | "input" | "network" | "general" | "advanced" | "home";

function route(): { page: string; params: URLSearchParams } {
  const h = window.location.hash.replace(/^#\/?/, "");
  const [page = "", query = ""] = h.split("?");
  return { page, params: new URLSearchParams(query) };
}

export function App() {
  const [r, setR] = useState(route);
  useEffect(() => {
    const on = () => setR(route());
    window.addEventListener("hashchange", on);
    const un = onNavigate((p) => {
      window.location.hash = `/${p}`;
    });
    return () => {
      window.removeEventListener("hashchange", on);
      void un.then((f) => f());
    };
  }, []);
  if (r.page === "identify") return <Identify label={r.params.get("label") ?? ""} />;
  return (
    <ToastProvider>
      <ConfigProvider>
        <Main page={r.page} />
      </ConfigProvider>
    </ToastProvider>
  );
}

function Loading({ error }: { error: string | null }) {
  return (
    <div data-tauri-drag-region className="flex h-full flex-col items-center justify-center gap-4">
      <div className="flex h-16 w-16 items-center justify-center rounded-[20px] bg-gradient-to-b from-accent-2 to-accent text-white shadow-lg">
        <Icon.Logo size={34} />
      </div>
      <div className="text-[17px] font-semibold">Glidedesk</div>
      {error ? (
        <div className="max-w-md">
          <Callout tone="bad">{error}</Callout>
        </div>
      ) : (
        <div className="flex items-center gap-2 text-muted">
          <Spinner size={16} /> Starting…
        </div>
      )}
    </div>
  );
}

function SaveIndicator() {
  const { saving, savedAt } = useConfig();
  const [, force] = useState(0);
  useEffect(() => {
    if (!savedAt) return;
    const t = setTimeout(() => force((n) => n + 1), 2200);
    return () => clearTimeout(t);
  }, [savedAt]);
  if (saving)
    return (
      <span className="inline-flex items-center gap-1.5 text-muted">
        <Spinner size={12} /> Saving…
      </span>
    );
  if (savedAt && Date.now() - savedAt < 2000)
    return (
      <span className="inline-flex items-center gap-1 text-ok">
        <Icon.Check size={14} /> Saved
      </span>
    );
  return null;
}

function NavItem({ id, label, icon, current }: { id: PageId; label: string; icon: ReactNode; current: boolean }) {
  return (
    <a
      href={`#/${id}`}
      aria-current={current ? "page" : undefined}
      className={`group mb-0.5 flex items-center gap-2.5 rounded-lg px-3 py-[7px] font-medium no-underline transition focus-visible:ring-2 focus-visible:ring-accent focus-visible:outline-none ${
        current ? "bg-accent-soft text-accent" : "text-fg/85 hover:bg-panel-2 hover:text-fg"
      }`}
    >
      <span className={current ? "text-accent" : "text-muted group-hover:text-fg"}>{icon}</span>
      {label}
    </a>
  );
}

function Main({ page }: { page: string }) {
  const status = useStatus();
  const { config, update, reload, error } = useConfig();
  const toast = useToast();
  const theme = config?.general.theme ?? "system";
  useEffect(() => {
    if (theme === "system") delete document.documentElement.dataset.theme;
    else document.documentElement.dataset.theme = theme;
  }, [theme]);
  // Agent errors (e.g. clipboard unavailable) also appear as toasts while the window is open.
  useEffect(() => {
    const un = onNotice((n) => {
      if (n.kind === "error") toast.show(n.message, "error");
      else if (n.kind === "info") toast.show(n.message);
    });
    return () => void un.then((f) => f());
  }, [toast]);
  // Phones: the sections live in a drawer opened from the top bar.
  const [menu, setMenu] = useState(false);
  useEffect(() => setMenu(false), [page]);
  useEffect(() => {
    if (!menu) return;
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && setMenu(false);
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [menu]);

  const role = status?.role ?? "unset";
  const nav = useMemo<[PageId, string, ReactNode][]>(
    () =>
      role === "client"
        ? [
            ["home", "This computer", <Icon.Home key="h" />],
            ["sharing", "Clipboard & Files", <Icon.Clipboard key="c" />],
            ["network", "Network", <Icon.Network key="n" />],
            ["general", "General", <Icon.Settings key="g" />],
            ["advanced", "Advanced", <Icon.Wrench key="a" />],
          ]
        : [
            ["layout", "Layout", <Icon.Layout key="l" />],
            ["computers", "Computers", <Icon.Computers key="p" />],
            ["sharing", "Clipboard & Files", <Icon.Clipboard key="c" />],
            ["input", "Keyboard & Mouse", <Icon.Keyboard key="k" />],
            ["network", "Network", <Icon.Network key="n" />],
            ["general", "General", <Icon.Settings key="g" />],
            ["advanced", "Advanced", <Icon.Wrench key="a" />],
          ],
    [role],
  );
  const current = (nav.find(([id]) => id === page)?.[0] ?? nav[0]?.[0] ?? "layout") as PageId;

  if (!status || !config) return <Loading error={status ? error : null} />;
  if (role === "unset") return <Wizard status={status} config={config} onDone={() => void reload()} />;

  const online = status.server?.clients.filter((c) => ["online", "degraded", "locked"].includes(c.state)).length ?? 0;
  const header =
    role === "server"
      ? status.running
        ? `Sharing · ${online} online`
        : "Stopped"
      : status.client?.state === "connected"
        ? `Connected to ${status.client.server_name ?? "server"}`
        : status.client?.state === "rejected"
          ? "Refused by the server"
          : status.running
            ? "Looking for the server…"
            : "Stopped";
  const offer = status.server?.offer ?? status.client?.offer ?? null;
  const pill =
    status.error || status.client?.state === "rejected"
      ? "bg-bad"
      : role === "client" && status.running && status.client?.state !== "connected"
        ? "bg-warn"
        : status.running
          ? "bg-ok"
          : "bg-muted/60";
  const props = { status, config, update };

  const identity = (
    <div className="flex min-w-0 items-center gap-2.5">
      <div className="flex h-9 w-9 shrink-0 items-center justify-center rounded-[11px] bg-gradient-to-b from-accent-2 to-accent text-white shadow-sm">
        <Icon.Logo size={20} />
      </div>
      <div className="min-w-0">
        <div className="truncate font-semibold">{status.device_name}</div>
        <div className="flex items-center gap-1.5 truncate text-[12px] text-muted">
          <span className={`inline-block h-1.5 w-1.5 shrink-0 rounded-full ${pill}`} aria-hidden />
          {header}
        </div>
      </div>
    </div>
  );

  return (
    <div className="flex h-full flex-col md:flex-row">
      {/* Phones: top bar with the menu. */}
      <div className="flex shrink-0 items-center justify-between gap-2 border-b border-line bg-panel/70 px-3 py-2 md:hidden">
        {identity}
        <button
          type="button"
          aria-label={menu ? "Close menu" : "Open menu"}
          aria-expanded={menu}
          aria-controls="sections"
          onClick={() => setMenu((m) => !m)}
          className="rounded-lg p-2 text-fg/80 hover:bg-panel-2 focus-visible:ring-2 focus-visible:ring-accent focus-visible:outline-none"
        >
          {menu ? <Icon.Close size={20} /> : <Icon.Menu size={20} />}
        </button>
      </div>
      {menu && <div className="fixed inset-0 z-20 bg-black/40 md:hidden" aria-hidden onClick={() => setMenu(false)} />}
      <nav
        id="sections"
        className={`${menu ? "fixed inset-y-0 left-0 z-30 flex shadow-2xl" : "hidden"} w-[260px] shrink-0 flex-col border-r border-line bg-panel px-3 pb-3 md:static md:flex md:w-[228px] md:bg-panel/70 md:shadow-none`}
        aria-label="Sections"
      >
        {/* Space for the macOS window buttons; draggable. */}
        <div data-tauri-drag-region className="h-4 shrink-0 md:h-9" />
        <div className="mb-5 hidden px-2 md:block">{identity}</div>
        <div className="mb-3 px-2 text-[12px] font-semibold uppercase tracking-wide text-muted md:hidden">Sections</div>
        {nav.map(([id, label, icon]) => (
          <NavItem key={id} id={id} label={label} icon={icon} current={current === id} />
        ))}
        <div className="mt-auto flex items-center justify-between px-2 pt-3 text-[12px] text-muted">
          <span>v{status.version}</span>
          <SaveIndicator />
        </div>
      </nav>
      <div className="flex min-h-0 min-w-0 flex-1 flex-col">
        {status.error && (
          <div className="flex items-center gap-2 border-b border-bad/30 bg-bad/10 px-4 py-2.5 text-bad md:px-8" role="alert">
            <Icon.Error size={16} /> {status.error}
          </div>
        )}
        {offer && (
          <div className="flex flex-wrap items-center gap-x-3 gap-y-1 border-b border-accent/30 bg-accent-soft px-4 py-2.5 text-fg md:px-8" role="status">
            <Icon.Clipboard size={16} />
            <span className="min-w-0 flex-1 truncate">
              <b>{offer}</b> is ready to paste here — press {status.platform === "macos" ? "⌘V" : "Ctrl+V"} in a folder.
            </span>
            <button className="font-medium text-accent hover:underline" onClick={() => void toast.run(() => api.fetchOffer(), "Files are on the clipboard")}>
              Get them now
            </button>
          </div>
        )}
        {error && (
          <div className="flex items-center gap-2 border-b border-warn/30 bg-warn/10 px-4 py-2.5 text-warn md:px-8" role="alert">
            <Icon.Warning size={16} /> {error}
          </div>
        )}
        {current === "layout" && <LayoutPage {...props} />}
        {current === "computers" && <ComputersPage {...props} />}
        {current === "sharing" && <SharingPage {...props} />}
        {current === "input" && <InputPage config={config} update={update} />}
        {current === "network" && <NetworkPage {...props} />}
        {current === "general" && <GeneralPage {...props} />}
        {current === "advanced" && <AdvancedPage {...props} reload={() => void reload()} />}
        {current === "home" && <ClientHome {...props} />}
      </div>
    </div>
  );
}
