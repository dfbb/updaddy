import type { ThemeMode } from "../types";
export function resolveLocale(locale?: string) { const value = locale ?? navigator.language ?? "en"; return value === "zh-CN" || value.startsWith("zh") ? "zh_CN" : "en"; }
let removeSystemListener: (() => void) | undefined;

export function setTheme(mode: ThemeMode) {
  removeSystemListener?.();
  removeSystemListener = undefined;
  const query = typeof window !== "undefined" && typeof window.matchMedia === "function" ? window.matchMedia("(prefers-color-scheme: dark)") : undefined;
  const apply = () => {
    document.documentElement.dataset.theme = mode === "system" && query?.matches ? "dark" : mode === "system" ? "light" : mode;
  };
  apply();
  if (mode === "system" && query) {
    query.addEventListener("change", apply);
    removeSystemListener = () => query.removeEventListener("change", apply);
  }
  document.documentElement.dataset.themeMode = mode;
}
