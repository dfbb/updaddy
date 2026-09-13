import { useMemo, useState } from "react";
import { CircleArrowUp, RefreshCw } from "lucide-react";
import { scanEcosystem, updateEcosystem } from "../api/tauri";
import type { Ecosystem, PackageRecord, PackageTask } from "../types";
import { useI18n } from "../i18n/useI18n";
import { useAppStore } from "../state/appStore";
import EmptyState from "./EmptyState";
import PackageTable from "./PackageTable";
import TaskProgressBar from "./TaskProgressBar";
import { normalizePackageRecord, type PackageInput } from "./PackageTable";

export interface EcosystemTabsProps {
  selected?: Ecosystem;
  packages?: PackageInput[];
  visibleEcosystems?: Ecosystem[];
  workers?: Record<string, string>;
  tasks?: PackageTask[];
  operationsDisabled?: boolean;
  onSelect?: (ecosystem: Ecosystem) => void;
  onScan?: (ecosystem: Ecosystem) => void | Promise<void>;
  onUpdateAll?: (ecosystem: Ecosystem) => void | Promise<void>;
  onCancel?: (taskId: string) => void | Promise<void>;
}

export { EcosystemTabs };

const ecosystems: Ecosystem[] = ["homebrew", "npm", "pip", "gem", "rustup"];

function isAvailable(state: string | undefined): boolean {
  return !state || !["missing", "not_found", "disabled"].includes(state.toLowerCase());
}

export default function EcosystemTabs({
  selected,
  packages: suppliedPackages,
  visibleEcosystems: suppliedVisible,
  workers: suppliedWorkers,
  tasks: suppliedTasks,
  operationsDisabled: suppliedDisabled,
  onSelect,
  onScan,
  onUpdateAll,
  onCancel,
}: EcosystemTabsProps) {
  const { t } = useI18n();
  const state = useAppStore((store) => store);
  const packages = (suppliedPackages ?? state.packages).map(normalizePackageRecord);
  const visibleEcosystems = suppliedVisible ?? state.visibleEcosystems;
  const workers = suppliedWorkers ?? state.workers;
  const tasks = suppliedTasks ?? state.tasks;
  const available = useMemo(() => ecosystems.filter((ecosystem) => visibleEcosystems.includes(ecosystem) && isAvailable(workers[ecosystem])), [visibleEcosystems, workers]);
  const [internalSelected, setInternalSelected] = useState<Ecosystem | undefined>(selected ?? available[0]);
  const current = selected && available.includes(selected) ? selected : (internalSelected && available.includes(internalSelected) ? internalSelected : available[0]);
  const select = (ecosystem: Ecosystem) => {
    setInternalSelected(ecosystem);
    onSelect?.(ecosystem);
  };
  const scan = onScan ?? ((ecosystem: Ecosystem) => scanEcosystem(ecosystem));
  const updateAll = onUpdateAll ?? ((ecosystem: Ecosystem) => updateEcosystem(ecosystem));

  if (available.length === 0) return <EmptyState title={t("empty.no_ecosystems")} description={t("empty.no_ecosystems_description")} />;

  const currentTasks = current ? tasks.filter((task) => task.ecosystem === current) : [];
  const currentPackages = current ? packages.filter((item) => item.ecosystem === current) : [];
  const currentUpdateCount = currentPackages.filter((item) => item.update_available).length;
  const operationsDisabled = suppliedDisabled ?? (
    currentTasks.some((task) => task.status === "pending" || task.status === "running")
    || (current ? ["running", "scanning", "updating", "uninstalling"].includes((workers[current] ?? "").toLowerCase()) : false)
  );

  return (
    <section className="ecosystem-tabs" aria-label={t("navigation.ecosystems")}>
      <div className="tab-list" role="tablist" aria-label={t("navigation.ecosystems")}>
        {available.map((ecosystem) => {
          const active = ecosystem === current;
          const count = packages.filter((item) => item.ecosystem === ecosystem && item.update_available).length;
          return (
            <button
              type="button"
              role="tab"
              aria-selected={active}
              aria-controls={`tabpanel-${ecosystem}`}
              id={`tab-${ecosystem}`}
              className={`tab-button ${active ? "active" : ""}`}
              key={ecosystem}
              onClick={() => select(ecosystem)}
            >
              {t(`ecosystems.${ecosystem}`)}
              {count > 0 && <span className="tab-count">{count}</span>}
            </button>
          );
        })}
      </div>
      {current && (
        <div role="tabpanel" id={`tabpanel-${current}`} aria-labelledby={`tab-${current}`} className="ecosystem-panel">
          <div className="panel-heading">
            <div>
              <p className="eyebrow">{t("ecosystem.eyebrow")}</p>
              <h2>{t(`ecosystems.${current}`)}</h2>
            </div>
            <div className="panel-actions">
              <button type="button" className="button button-primary" disabled={operationsDisabled || currentUpdateCount === 0} onClick={() => void updateAll(current)}>
                <CircleArrowUp size={16} aria-hidden="true" />{t("buttons.update_all")}
              </button>
              <button type="button" className="button button-secondary" disabled={operationsDisabled} onClick={() => void scan(current)}>
                <RefreshCw size={16} aria-hidden="true" />{t("buttons.scan")}
              </button>
            </div>
          </div>
          <TaskProgressBar ecosystem={current} tasks={currentTasks} logs={state.logs} onCancel={onCancel} />
          <PackageTable packages={currentPackages} ecosystem={current} disabled={operationsDisabled} activeTaskIds={currentTasks.filter((task) => task.status === "pending" || task.status === "running").map((task) => task.task_id)} activeTasks={currentTasks} />
        </div>
      )}
    </section>
  );
}
