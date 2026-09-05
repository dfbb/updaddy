import { updateAllVisible } from "../api/tauri";
import { RefreshCw } from "lucide-react";
import type { Ecosystem, PackageRecord, PackageTask } from "../types";
import { useI18n } from "../i18n/useI18n";
import { useAppStore } from "../state/appStore";
import EmptyState from "./EmptyState";
import StatusBadge from "./StatusBadge";
import { normalizePackageRecord, type PackageInput } from "./PackageTable";

export interface OverviewPageProps {
  packages?: PackageInput[];
  visibleEcosystems?: Ecosystem[];
  tasks?: PackageTask[];
  operationsDisabled?: boolean;
  onUpdateAll?: () => void | Promise<void>;
  onSelectEcosystem?: (ecosystem: Ecosystem) => void;
}

export { OverviewPage };

export default function OverviewPage({
  packages: suppliedPackages,
  visibleEcosystems: suppliedVisible,
  tasks: suppliedTasks,
  operationsDisabled: suppliedDisabled,
  onUpdateAll,
  onSelectEcosystem,
}: OverviewPageProps) {
  const { t } = useI18n();
  const state = useAppStore((store) => store);
  const packages = (suppliedPackages ?? state.packages).map(normalizePackageRecord);
  const visible = suppliedVisible ?? state.visibleEcosystems;
  const tasks = suppliedTasks ?? state.tasks;
  const operationsDisabled = suppliedDisabled ?? state.operationsDisabled;
  const updateAll = onUpdateAll ?? (() => updateAllVisible());
  const totalUpdates = packages.filter((item) => visible.includes(item.ecosystem) && item.update_available).length;
  const activeTasks = tasks.filter((task) => task.status === "pending" || task.status === "running");

  return (
    <section className="overview-page">
      <div className="page-heading">
        <div>
          <p className="eyebrow">{t("overview.eyebrow")}</p>
          <h2>{t("overview.title")}</h2>
        </div>
        <button type="button" className="button button-primary" disabled={operationsDisabled || totalUpdates === 0} onClick={() => void updateAll()}>
          <RefreshCw size={16} aria-hidden="true" />{t("buttons.update_all")}
        </button>
      </div>

      <div className="overview-total-card">
        <div>
          <span className="card-label">{t("overview.updates_available")}</span>
          <strong className="overview-total">{totalUpdates}</strong>
        </div>
        <StatusBadge status={totalUpdates ? "update_available" : "up_to_date"} />
      </div>

      {visible.length === 0 ? (
        <EmptyState title={t("empty.no_ecosystems")} description={t("empty.no_ecosystems_description")} />
      ) : (
        <div className="ecosystem-summary-grid">
          {visible.map((ecosystem) => {
            const ecosystemPackages = packages.filter((item) => item.ecosystem === ecosystem);
            const updates = ecosystemPackages.filter((item) => item.update_available).length;
            const active = activeTasks.some((task) => task.ecosystem === ecosystem);
            return (
              <button type="button" className="ecosystem-summary-card" key={ecosystem} onClick={() => onSelectEcosystem?.(ecosystem)}>
                <div className="summary-card-heading">
                  <span className="ecosystem-icon" aria-hidden="true">{ecosystem.slice(0, 1).toUpperCase()}</span>
                  <span>{t(`ecosystems.${ecosystem}`)}</span>
                  {active && <span className="summary-card-spinner" aria-label={t("task.running")} />}
                </div>
                <strong>{updates}</strong>
                <span>{updates === 1 ? t("overview.one_update") : t("overview.many_updates", { count: updates })}</span>
                <span className="summary-card-footer">{ecosystemPackages.length} {t("overview.installed")}</span>
              </button>
            );
          })}
        </div>
      )}
    </section>
  );
}
