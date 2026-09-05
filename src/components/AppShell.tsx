import type { ReactNode } from "react";
import { ArrowUp, History, Home, Settings } from "lucide-react";
import type { Ecosystem } from "../types";
import { useI18n } from "../i18n/useI18n";
import { useAppStore } from "../state/appStore";

export type AppPage = "overview" | "ecosystem" | "history" | "settings";

export interface AppShellProps {
  activePage: AppPage;
  activeEcosystem?: Ecosystem;
  onNavigate: (page: AppPage, ecosystem?: Ecosystem) => void;
  children: ReactNode;
}

const ecosystems: Ecosystem[] = ["homebrew", "npm", "pip", "gem", "rustup"];

export default function AppShell({ activePage, activeEcosystem, onNavigate, children }: AppShellProps) {
  const { t } = useI18n();
  const visible = useAppStore((state) => state.visibleEcosystems);
  const totalUpdates = useAppStore((state) => state.totalUpdates);
  const operationsDisabled = useAppStore((state) => state.operationsDisabled);

  return (
    <div className="app-shell">
      <header className="app-header">
        <div className="brand-lockup">
          <div className="brand-mark" aria-hidden="true"><ArrowUp size={21} /></div>
          <div>
            <h1>updaddy</h1>
          </div>
        </div>
        <div className="header-actions">
          <div className="header-summary" aria-live="polite">
            <span className="header-summary-label">{t("overview.updates_available")}</span>
            <strong className="update-count">{totalUpdates}</strong>
          </div>
          <button type="button" className="button button-secondary header-settings" title={t("buttons.settings")} onClick={() => onNavigate("settings")} aria-label={t("buttons.settings")}><Settings size={17} /></button>
        </div>
      </header>
      <div className="app-body">
        <nav className="side-nav" aria-label={t("navigation.label")}>
          <button type="button" className={`nav-item ${activePage === "overview" ? "active" : ""}`} aria-current={activePage === "overview" ? "page" : undefined} onClick={() => onNavigate("overview")}>
            <Home size={16} aria-hidden="true" />{t("overview.label")}
          </button>
          <div className="nav-section-label">{t("navigation.ecosystems")}</div>
          {ecosystems.filter((ecosystem) => visible.includes(ecosystem)).map((ecosystem) => (
            <button
              type="button"
              key={ecosystem}
              className={`nav-item nav-ecosystem ${activePage === "ecosystem" && activeEcosystem === ecosystem ? "active" : ""}`}
              aria-current={activePage === "ecosystem" && activeEcosystem === ecosystem ? "page" : undefined}
              onClick={() => onNavigate("ecosystem", ecosystem)}
            >
              <span className="nav-dot" aria-hidden="true" />{t(`ecosystems.${ecosystem}`)}
            </button>
          ))}
          <div className="nav-spacer" />
          <button type="button" className={`nav-item ${activePage === "history" ? "active" : ""}`} aria-current={activePage === "history" ? "page" : undefined} onClick={() => onNavigate("history")}>
            <History size={16} aria-hidden="true" />{t("navigation.history")}
          </button>
          <button type="button" className={`nav-item ${activePage === "settings" ? "active" : ""}`} aria-current={activePage === "settings" ? "page" : undefined} onClick={() => onNavigate("settings")}>
            <Settings size={16} aria-hidden="true" />{t("buttons.settings")}
          </button>
          {operationsDisabled && <p className="nav-status">{t("navigation.operation_in_progress")}</p>}
        </nav>
        <main className="app-content">{children}</main>
      </div>
    </div>
  );
}

export { AppShell };
