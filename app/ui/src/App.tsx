import { useEffect, useMemo, useState, type ReactNode } from "react";
import { onNavigate, onNotice } from "./lib/api";
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
    });
    return () => void un.then((f) => f());
  }, [toast]);

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
        : status.running
          ? "Looking for the server…"
          : "Stopped";
  const pill = status.error ? "bg-bad" : status.running ? "bg-ok" : "bg-muted/60";
  const props = { status, config, update };

  return (
    <div className="flex h-full">
      <nav className="flex w-[228px] shrink-0 flex-col border-r border-line bg-panel/70 px-3 pb-3" aria-label="Sections">
        {/* Space for the macOS window buttons; draggable. */}
        <div data-tauri-drag-region className="h-9 shrink-0" />
        <div className="mb-5 flex items-center gap-2.5 px-2">
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
        {nav.map(([id, label, icon]) => (
          <NavItem key={id} id={id} label={label} icon={icon} current={current === id} />
        ))}
        <div className="mt-auto flex items-center justify-between px-2 pt-3 text-[12px] text-muted">
          <span>v{status.version}</span>
          <SaveIndicator />
        </div>
      </nav>
      <div className="flex min-w-0 flex-1 flex-col">
        {status.error && (
          <div className="flex items-center gap-2 border-b border-bad/30 bg-bad/10 px-8 py-2.5 text-bad" role="alert">
            <Icon.Error size={16} /> {status.error}
          </div>
        )}
        {error && (
          <div className="flex items-center gap-2 border-b border-warn/30 bg-warn/10 px-8 py-2.5 text-warn" role="alert">
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
