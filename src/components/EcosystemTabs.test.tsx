import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import EcosystemTabs from "./EcosystemTabs";

describe("EcosystemTabs", () => {
  it("updates only the ecosystem selected by the current tab", async () => {
    const updateAll = vi.fn();
    render(<EcosystemTabs
      selected="npm"
      packages={[
        { id: "npm:a", ecosystem: "npm", name: "a", updateAvailable: true },
        { id: "pip:b", ecosystem: "pip", name: "b", updateAvailable: true },
      ]}
      visibleEcosystems={["npm", "pip"]}
      workers={{ npm: "idle", pip: "idle" }}
      tasks={[]}
      operationsDisabled={false}
      onUpdateAll={updateAll}
    />);

    await userEvent.click(screen.getByRole("button", { name: /update all|全部更新/i }));
    expect(updateAll).toHaveBeenCalledWith("npm");
  });

  it("disables update all when the current ecosystem is already up to date", () => {
    render(<EcosystemTabs
      selected="npm"
      packages={[{ id: "npm:a", ecosystem: "npm", name: "a", updateAvailable: false }]}
      visibleEcosystems={["npm"]}
      workers={{ npm: "idle" }}
      tasks={[]}
      operationsDisabled={false}
    />);

    expect(screen.getByRole("button", { name: /update all|全部更新/i })).toBeDisabled();
  });

  it("keeps Homebrew actions enabled while Pip is busy", () => {
    render(<EcosystemTabs
      selected="homebrew"
      packages={[{ id: "homebrew:a", ecosystem: "homebrew", name: "a", updateAvailable: true }]}
      visibleEcosystems={["homebrew", "pip"]}
      workers={{ homebrew: "idle", pip: "running" }}
      tasks={[{ task_id: "pip-scan", ecosystem: "pip", name: "*", operation: "scan", status: "running" }]}
    />);

    expect(screen.getByRole("button", { name: /update all|全部更新/i })).toBeEnabled();
    expect(screen.getByRole("button", { name: /scan|扫描/i })).toBeEnabled();
  });
});
