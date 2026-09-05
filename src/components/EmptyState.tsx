import { useI18n } from "../i18n/useI18n";

export interface EmptyStateProps {
  title?: string;
  description?: string;
  className?: string;
}

export default function EmptyState({ title, description, className = "" }: EmptyStateProps) {
  const { t } = useI18n();
  return (
    <div className={`empty-state ${className}`.trim()} role="status">
      <div className="empty-state-icon" aria-hidden="true">
        ✓
      </div>
      <h3>{title ?? t("empty.no_packages")}</h3>
      <p>{description ?? t("empty.no_packages_description")}</p>
    </div>
  );
}

export { EmptyState };
