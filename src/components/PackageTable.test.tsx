import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import PackageTable from "./PackageTable";
import type { PackageRecord } from "../types";

function packageRecord(overrides: Partial<PackageRecord> = {}): PackageRecord {
  return {
    id: "npm:package",
    ecosystem: "npm",
    resource_kind: "package",
    name: "package",
    current_version: "1.0.0",
    target_version: undefined,
    disk_usage: { bytes: 1024, scanned_at: 1, status: "ready" },
    update_available: false,
    ...overrides,
  };
}

describe("PackageTable", () => {
  it("默认把待更新软件包放在列表前面", () => {
    render(<PackageTable packages={[
      packageRecord({ id: "npm:prettier", name: "prettier" }),
      packageRecord({ id: "npm:eslint", name: "eslint", update_available: true, target_version: "2.0.0", disk_usage: { bytes: 2048, scanned_at: 1, status: "ready" } }),
    ]} />);

    expect(screen.getAllByRole("row")[1]).toHaveTextContent("eslint");
  });

  it("点击磁盘占用后按整个列表的占用从大到小排序", async () => {
    const user = userEvent.setup();
    render(<PackageTable packages={[
      packageRecord({ id: "npm:a", name: "a", update_available: true, disk_usage: { bytes: 100, scanned_at: 1, status: "ready" } }),
      packageRecord({ id: "npm:b", name: "b", disk_usage: { bytes: 10_000, scanned_at: 1, status: "ready" } }),
    ]} />);

    await user.click(screen.getByRole("button", { name: /disk usage|磁盘占用/i }));
    expect(screen.getAllByRole("row")[1]).toHaveTextContent("b");
  });

  it("点击名称后按字母排序并可切换方向", async () => {
    const user = userEvent.setup();
    render(<PackageTable packages={[
      packageRecord({ id: "npm:z", name: "zulu", update_available: true }),
      packageRecord({ id: "npm:a", name: "alpha" }),
    ]} />);

    await user.click(screen.getByRole("button", { name: /name|名称/i }));
    expect(screen.getAllByRole("row")[1]).toHaveTextContent("alpha");
    await user.click(screen.getByRole("button", { name: /name|名称/i }));
    expect(screen.getAllByRole("row")[1]).toHaveTextContent("zulu");
  });

  it("点击状态后按有更新优先排序并可切换方向", async () => {
    const user = userEvent.setup();
    render(<PackageTable packages={[
      packageRecord({ id: "npm:a", name: "alpha" }),
      packageRecord({ id: "npm:z", name: "zulu", update_available: true }),
    ]} />);

    await user.click(screen.getByRole("button", { name: /status|状态/i }));
    expect(screen.getAllByRole("row")[1]).toHaveTextContent("zulu");
    await user.click(screen.getByRole("button", { name: /status|状态/i }));
    expect(screen.getAllByRole("row")[1]).toHaveTextContent("alpha");
  });

  it("兼容后端 snake_case 与列表测试常用的 camelCase 磁盘字段", () => {
    render(<PackageTable packages={[{ id: "npm:camel", name: "camel", updateAvailable: true, diskBytes: 4096 }]} />);
    expect(screen.getByText("4 KB")).toBeInTheDocument();
    expect(screen.getByText(/update available/i)).toBeInTheDocument();
  });

  it("卸载必须先经过确认对话框", async () => {
    const user = userEvent.setup();
    const onUninstall = vi.fn();
    render(<PackageTable packages={[packageRecord({ name: "typescript" })]} onUninstall={onUninstall} />);

    await user.click(screen.getAllByRole("button", { name: /uninstall|卸载/i })[0]);
    expect(screen.getByRole("dialog")).toHaveTextContent("typescript");
    expect(onUninstall).not.toHaveBeenCalled();
    await user.click(within(screen.getByRole("dialog")).getByRole("button", { name: /uninstall|卸载/i }));
    expect(onUninstall).toHaveBeenCalledWith(expect.objectContaining({ name: "typescript" }));
  });

  it("全局任务运行时禁用更新和卸载，但由上层保留取消按钮", () => {
    render(<PackageTable packages={[packageRecord({ update_available: true })]} disabled />);
    expect(screen.getByRole("button", { name: /update|更新/i })).toBeDisabled();
    expect(screen.getByRole("button", { name: /uninstall|卸载/i })).toBeDisabled();
  });
});
