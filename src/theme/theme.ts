import type { ThemeMode } from "../types";
export function resolveLocale(locale?: string) { const value = locale ?? navigator.language ?? "en"; return value === "zh-CN" || value.startsWith("zh") ? "zh_CN" : "en"; }
export function setTheme(mode: ThemeMode) { const resolved = mode === "system" ? (matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light") : mode; document.documentElement.dataset.theme = resolved; document.documentElement.dataset.themeMode = mode; }
