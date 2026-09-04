import { useCallback } from "react";
import { translator } from "./catalog";
import { useAppStore } from "../state/appStore";
export function useI18n() { const locale = useAppStore((s) => s.locale); return { locale, t: useCallback((key: string, vars?: Record<string, string | number>) => translator.t(key, locale, vars), [locale]) }; }
