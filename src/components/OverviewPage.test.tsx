import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import OverviewPage from "./OverviewPage";
import type { PackageRecord } from "../types";

const pkg = (ecosystem: PackageRecord["ecosystem"], name: string, update_available = false): PackageRecord => ({
  id: `${ecosystem}:${name}`,
  ecosystem,
  resource_kind: "package",
  name,
  current_version: "1.0.0",
  target_version: update_available ? "2.0.0" : undefined,
  disk_usage: { bytes: 1, scanned_at: 1, status: "ready" },
  update_available,
});

describe("OverviewPage", () => {
  it("显示可见生态合计更新数，并触发一键更新", async () => {
    const user = userEvent.setup();
    const onUpdateAll = vi.fn();
    render(<OverviewPage packages={[pkg("npm", "a", true), pkg("pip", "b", true), pkg("gem", "c")] } visibleEcosystems={["npm", "pip"]} onUpdateAll={onUpdateAll} />);

    expect(screen.getByText("2", { selector: ".overview-total" })).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: /update all|全部更新/i }));
    expect(onUpdateAll).toHaveBeenCalledTimes(1);
  });

  it("点击生态卡片后通知上层导航", async () => {
    const user = userEvent.setup();
    const onSelectEcosystem = vi.fn();
    render(<OverviewPage packages={[pkg("gem", "rails")] } visibleEcosystems={["gem"]} onSelectEcosystem={onSelectEcosystem} />);

    await user.click(screen.getByRole("button", { name: /RubyGems|gem/i }));
    expect(onSelectEcosystem).toHaveBeenCalledWith("gem");
  });
});
