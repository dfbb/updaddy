import { useEffect } from "react";
import { getSettings, getStateSnapshot, subscribeToBackendEvents } from "./api/tauri";
import { applyEvent, applySnapshot, setSettings, useAppStore } from "./state/appStore";
import { setTheme } from "./theme/theme";
import { useI18n } from "./i18n/useI18n";

export default function App() {
  const { t } = useI18n();
  const packages = useAppStore((s) => s.packages);
  const totalUpdates = useAppStore((s) => s.totalUpdates);
  const theme = useAppStore((s) => s.theme);
  useEffect(() => { setTheme(theme); }, [theme]);
  useEffect(() => { let unlisten: (() => void)[] = []; Promise.all([getSettings().then(setSettings).catch(() => undefined), getStateSnapshot().then(applySnapshot).catch(() => undefined), subscribeToBackendEvents((name, payload) => applyEvent(name, payload)).then((fns) => { unlisten = fns; })]); return () => unlisten.forEach((fn) => fn()); }, []);
  return <main className="app-shell"><header><h1>updaddy</h1><span>{t("overview")}</span><span className="update-count">{totalUpdates}</span></header><p>{packages.length ? `${packages.length} ${t("ecosystems.homebrew")}` : t("task.pending")}</p></main>;
}
