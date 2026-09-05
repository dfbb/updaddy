import { useEffect, useMemo, useState } from "react";
import { Save, Trash2 } from "lucide-react";
import { cleanupExpiredLogs, saveSettings, setLoginItem } from "../api/tauri";
import { useI18n } from "../i18n/useI18n";
import { defaultSettings, setSettings, useAppStore } from "../state/appStore";
import type { Ecosystem, LogLevel, Settings, ThemeMode } from "../types";
import LogViewer from "./LogViewer";

type SettingsTab = "general" | "logs";
const ecosystems: Ecosystem[] = ["homebrew", "npm", "pip", "gem", "rustup"];

function parseSchedule(raw?: string): { enabled: boolean; kind: "daily" | "weekly"; day: string; time: string } {
  const value = raw?.trim() ?? "";
  if (!value) return { enabled: false, kind: "daily", day: "monday", time: "03:00" };
  const weekly = value.replace(/^weekly:/, "").split(" ");
  if (weekly.length === 2) return { enabled: true, kind: "weekly", day: weekly[0], time: weekly[1] };
  return { enabled: true, kind: "daily", day: "monday", time: value };
}

function serializeSchedule(schedule: ReturnType<typeof parseSchedule>): string | undefined {
  if (!schedule.enabled) return undefined;
  return schedule.kind === "weekly" ? `weekly:${schedule.day} ${schedule.time}` : schedule.time;
}

interface SettingsPageProps {
  initialTab?: SettingsTab;
}

export default function SettingsPage({ initialTab = "general" }: SettingsPageProps) {
  const { t } = useI18n();
  const stored = useAppStore((state) => state.settings);
  const logs = useAppStore((state) => state.logs);
  const operationsDisabled = useAppStore((state) => state.operationsDisabled);
  const [tab, setTab] = useState<SettingsTab>(initialTab);
  const [draft, setDraft] = useState<Settings>(stored ?? defaultSettings);
  const [schedule, setSchedule] = useState(() => parseSchedule(stored?.schedule));
  const [saved, setSaved] = useState(false);
  const [error, setError] = useState<string>();
  const [cleaned, setCleaned] = useState<number>();

  useEffect(() => {
    setDraft(stored ?? defaultSettings);
    setSchedule(parseSchedule(stored?.schedule));
  }, [stored]);

  const updateDraft = <K extends keyof Settings>(key: K, value: Settings[K]) => {
    setSaved(false);
    setError(undefined);
    setDraft((current) => ({ ...current, [key]: value }));
  };

  const toggleEcosystem = (ecosystem: Ecosystem) => {
    const visible = new Set(draft.visible_ecosystems);
    if (visible.has(ecosystem)) visible.delete(ecosystem);
    else visible.add(ecosystem);
    updateDraft("visible_ecosystems", ecosystems.filter((item) => visible.has(item)));
  };

  const save = async () => {
    setError(undefined);
    const next = { ...draft, schedule: serializeSchedule(schedule) };
    try {
      await saveSettings(next);
      setSettings(next);
      if (next.login_item !== stored.login_item) await setLoginItem(next.login_item);
      setSaved(true);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    }
  };

  const proxy = draft.proxy ?? defaultSettings.proxy;
  const logSettings = draft.logs ?? defaultSettings.logs;
  const scheduleLabel = useMemo(() => schedule.kind === "weekly" ? t("settings.weekly") : t("settings.daily"), [schedule.kind, t]);

  return (
    <section className="settings-page">
      <div className="page-heading">
        <div><p className="eyebrow">{t("buttons.settings")}</p><h2>{t("settings.title")}</h2></div>
        {tab === "general" && <button type="button" className="button button-primary" disabled={operationsDisabled} onClick={() => void save()}><Save size={16} aria-hidden="true" />{t("settings.save")}</button>}
      </div>
      <div className="settings-tabs" role="tablist" aria-label={t("settings.tabs_label")}>
        <button type="button" role="tab" aria-selected={tab === "general"} className={`settings-tab ${tab === "general" ? "active" : ""}`} onClick={() => setTab("general")}>{t("settings.general_tab")}</button>
        <button type="button" role="tab" aria-selected={tab === "logs"} className={`settings-tab ${tab === "logs" ? "active" : ""}`} onClick={() => setTab("logs")}>{t("settings.logs_tab")}</button>
      </div>

      {tab === "general" ? (
        <div className="settings-grid">
          <section className="settings-card">
            <h3>{t("settings.appearance")}</h3>
            <label className="field-label" htmlFor="theme">{t("settings.theme")}</label>
            <select id="theme" disabled={operationsDisabled} value={draft.theme} onChange={(event) => updateDraft("theme", event.target.value as ThemeMode)}>
              <option value="system">{t("settings.theme_system")}</option><option value="light">{t("settings.theme_light")}</option><option value="dark">{t("settings.theme_dark")}</option>
            </select>
            <label className="field-label" htmlFor="locale">{t("settings.locale")}</label>
            <select id="locale" disabled={operationsDisabled} value={draft.locale} onChange={(event) => updateDraft("locale", event.target.value)}><option value="en">English</option><option value="zh-CN">简体中文</option></select>
          </section>

          <section className="settings-card">
            <h3>{t("settings.proxy")}</h3>
            <label className="checkbox-row"><input type="checkbox" disabled={operationsDisabled} checked={proxy.enabled} onChange={(event) => updateDraft("proxy", { ...proxy, enabled: event.target.checked })} />{t("settings.use_proxy")}</label>
            <label className="field-label" htmlFor="proxy-address">{t("settings.proxy_address")}</label>
            <input id="proxy-address" type="text" placeholder="socks5://127.0.0.1:1080" value={proxy.address} disabled={operationsDisabled || !proxy.enabled} onChange={(event) => updateDraft("proxy", { ...proxy, address: event.target.value })} />
            <label className="field-label" htmlFor="proxy-username">{t("settings.proxy_username")}</label>
            <input id="proxy-username" type="text" value={proxy.username ?? ""} disabled={operationsDisabled || !proxy.enabled} onChange={(event) => updateDraft("proxy", { ...proxy, username: event.target.value })} />
            <label className="field-label" htmlFor="proxy-password">{t("settings.proxy_password")}</label>
            <input id="proxy-password" type="password" value={proxy.password ?? ""} disabled={operationsDisabled || !proxy.enabled} onChange={(event) => updateDraft("proxy", { ...proxy, password: event.target.value })} />
            <p className="field-help">{t("settings.proxy_warning")}</p>
          </section>

          <section className="settings-card">
            <h3>{t("settings.schedule")}</h3>
            <label className="checkbox-row"><input type="checkbox" disabled={operationsDisabled} checked={schedule.enabled} onChange={(event) => setSchedule((current) => ({ ...current, enabled: event.target.checked }))} />{t("settings.schedule_enabled")}</label>
            <div className="field-inline"><label className="field-label" htmlFor="schedule-kind">{t("settings.schedule_type")}</label><select id="schedule-kind" value={schedule.kind} disabled={operationsDisabled || !schedule.enabled} onChange={(event) => setSchedule((current) => ({ ...current, kind: event.target.value as "daily" | "weekly" }))}><option value="daily">{t("settings.daily")}</option><option value="weekly">{t("settings.weekly")}</option></select></div>
            {schedule.kind === "weekly" && <div className="field-inline"><label className="field-label" htmlFor="schedule-day">{t("settings.weekday")}</label><select id="schedule-day" value={schedule.day} disabled={operationsDisabled || !schedule.enabled} onChange={(event) => setSchedule((current) => ({ ...current, day: event.target.value }))}>{["monday", "tuesday", "wednesday", "thursday", "friday", "saturday", "sunday"].map((day) => <option value={day} key={day}>{t(`settings.days.${day}`)}</option>)}</select></div>}
            <label className="field-label" htmlFor="schedule-time">{t("settings.time")}</label><input id="schedule-time" type="time" value={schedule.time} disabled={operationsDisabled || !schedule.enabled} onChange={(event) => setSchedule((current) => ({ ...current, time: event.target.value }))} />
            <p className="field-help">{schedule.enabled ? `${scheduleLabel} · ${schedule.time}` : t("settings.schedule_disabled")}</p>
          </section>

          <section className="settings-card">
            <h3>{t("settings.ecosystems")}</h3>
            {ecosystems.map((ecosystem) => <label className="checkbox-row" key={ecosystem}><input type="checkbox" disabled={operationsDisabled || draft.visible_ecosystems.length === 1 && draft.visible_ecosystems.includes(ecosystem)} checked={draft.visible_ecosystems.includes(ecosystem)} onChange={() => toggleEcosystem(ecosystem)} />{t(`ecosystems.${ecosystem}`)}</label>)}
            <label className="checkbox-row"><input type="checkbox" disabled={operationsDisabled} checked={draft.login_item} onChange={(event) => updateDraft("login_item", event.target.checked)} />{t("settings.login_item")}</label>
          </section>
        </div>
      ) : (
        <section className="settings-card settings-log-card">
          <div className="section-heading"><div><h3>{t("settings.logs_tab")}</h3></div><button type="button" className="button button-secondary" disabled={operationsDisabled} onClick={() => void cleanupExpiredLogs().then(setCleaned).catch(() => undefined)}><Trash2 size={15} aria-hidden="true" />{t("settings.cleanup_logs")}</button></div>
          <label className="field-label" htmlFor="log-level">{t("settings.log_level")}</label>
          <select id="log-level" value={logSettings.level} disabled={operationsDisabled} onChange={(event) => updateDraft("logs", { ...logSettings, level: event.target.value as LogLevel })}><option value="error">error</option><option value="warn">warn</option><option value="info">info</option><option value="debug">debug</option></select>
          <label className="checkbox-row"><input type="checkbox" checked={logSettings.retain_command_output} disabled={operationsDisabled} onChange={(event) => updateDraft("logs", { ...logSettings, retain_command_output: event.target.checked })} />{t("settings.retain_output")}</label>
          <p className="field-help">{t("settings.retention_notice")}{cleaned !== undefined ? ` ${t("settings.cleaned_count", { count: cleaned })}` : ""}</p>
          <LogViewer entries={logs} emptyLabel={t("logs.empty")} />
          {tab === "logs" && <button type="button" className="button button-primary settings-log-save" disabled={operationsDisabled} onClick={() => void save()}><Save size={16} aria-hidden="true" />{saved ? t("settings.saved") : t("settings.save")}</button>}
        </section>
      )}
      {saved && <p className="settings-feedback success" role="status">{t("settings.saved")}</p>}
      {error && <p className="settings-feedback error" role="alert">{error}</p>}
    </section>
  );
}

export { parseSchedule, serializeSchedule };
