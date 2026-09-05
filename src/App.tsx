import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { getSettings, getStateSnapshot, subscribeToBackendEvents } from "./api/tauri";
import AppShell, { type AppPage } from "./components/AppShell";
import EcosystemTabs from "./components/EcosystemTabs";
import HistoryPage from "./components/HistoryPage";
import OverviewPage from "./components/OverviewPage";
import SettingsPage from "./components/SettingsPage";
import { applyEvents, applySnapshot, setSettings, useAppStore } from "./state/appStore";
import { setTheme } from "./theme/theme";
import type { BackendEvent, Ecosystem } from "./types";

export default function App() {
  const theme = useAppStore((state) => state.theme);
  const [page, setPage] = useState<AppPage>("overview");
  const [selectedEcosystem, setSelectedEcosystem] = useState<Ecosystem | undefined>();

  useEffect(() => {
    setTheme(theme);
  }, [theme]);

  useEffect(() => {
    let unlisten: (() => void)[] = [];
    let mounted = true;
    let flushTimer: number | undefined;
    let pendingEvents: Array<readonly [string, BackendEvent]> = [];
    const queueEvent = (name: string, payload: BackendEvent) => {
      pendingEvents.push([name, payload]);
      if (flushTimer !== undefined) return;
      flushTimer = window.setTimeout(() => {
        flushTimer = undefined;
        const events = pendingEvents;
        pendingEvents = [];
        if (mounted) applyEvents(events);
      }, 16);
    };
    void Promise.all([
      getSettings().then(setSettings).catch(() => undefined),
      getStateSnapshot().then(applySnapshot).catch(() => undefined),
      subscribeToBackendEvents(queueEvent).then((fns) => {
        if (mounted) unlisten = fns;
        else fns.forEach((fn) => fn());
      }).catch(() => undefined),
    ]);
    return () => {
      mounted = false;
      if (flushTimer !== undefined) window.clearTimeout(flushTimer);
      pendingEvents = [];
      unlisten.forEach((fn) => fn());
    };
  }, []);

  useEffect(() => {
    let unlistenSettings: (() => void) | undefined;
    let unlistenOverview: (() => void) | undefined;
    void listen("open-settings", () => setPage("settings"))
      .then((cleanup) => { unlistenSettings = cleanup; })
      .catch(() => undefined);
    void listen("open-overview", () => setPage("overview"))
      .then((cleanup) => { unlistenOverview = cleanup; })
      .catch(() => undefined);
    return () => {
      unlistenSettings?.();
      unlistenOverview?.();
    };
  }, []);

  const navigate = (nextPage: AppPage, ecosystem?: Ecosystem) => {
    setPage(nextPage);
    if (ecosystem) setSelectedEcosystem(ecosystem);
  };

  return (
    <AppShell activePage={page} activeEcosystem={selectedEcosystem} onNavigate={navigate}>
      {page === "overview" && <OverviewPage onSelectEcosystem={(ecosystem) => navigate("ecosystem", ecosystem)} />}
      {page === "ecosystem" && <EcosystemTabs selected={selectedEcosystem} onSelect={setSelectedEcosystem} />}
      {page === "history" && <HistoryPage />}
      {page === "settings" && <SettingsPage />}
    </AppShell>
  );
}
