import type { PackageRecord } from "../types";
import { useI18n } from "../i18n/useI18n";

export interface ConfirmUninstallDialogProps {
  packageRecord: PackageRecord | null;
  open?: boolean;
  disabled?: boolean;
  onConfirm: (packageRecord: PackageRecord) => void | Promise<void>;
  onCancel: () => void;
}

export default function ConfirmUninstallDialog({
  packageRecord,
  open = true,
  disabled = false,
  onConfirm,
  onCancel,
}: ConfirmUninstallDialogProps) {
  const { t } = useI18n();
  if (!open || !packageRecord) return null;

  const ecosystem = t(`ecosystems.${packageRecord.ecosystem}`);
  const version = packageRecord.current_version ?? t("package.unknown_version");
  const resourceKind = t(`resource.${packageRecord.resource_kind}`);

  return (
    <div className="dialog-backdrop" role="presentation" onMouseDown={(event) => {
      if (event.target === event.currentTarget) onCancel();
    }}>
      <section
        className="confirm-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="uninstall-dialog-title"
        aria-describedby="uninstall-dialog-description"
      >
        <h2 id="uninstall-dialog-title">{t("confirm_uninstall.title")}</h2>
        <p id="uninstall-dialog-description">{t("confirm_uninstall.description")}</p>
        <dl className="package-details">
          <div>
            <dt>{t("package.ecosystem")}</dt>
            <dd>{ecosystem}</dd>
          </div>
          <div>
            <dt>{t("package.name")}</dt>
            <dd>{packageRecord.name}</dd>
          </div>
          <div>
            <dt>{t("package.version")}</dt>
            <dd>{version}</dd>
          </div>
          <div>
            <dt>{t("package.resource_type")}</dt>
            <dd>{resourceKind}</dd>
          </div>
        </dl>
        <div className="dialog-actions">
          <button type="button" className="button button-secondary" onClick={onCancel}>
            {t("buttons.cancel")}
          </button>
          <button
            type="button"
            className="button button-danger"
            disabled={disabled}
            onClick={() => void onConfirm(packageRecord)}
            autoFocus
          >
            {t("buttons.uninstall")}
          </button>
        </div>
      </section>
    </div>
  );
}

export { ConfirmUninstallDialog };
