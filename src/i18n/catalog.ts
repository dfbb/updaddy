import yaml from "js-yaml";
import enText from "../../lang/en.yaml?raw";
import zhText from "../../lang/zh_CN.yaml?raw";
export type Catalog = Record<string, any>;
export const catalogs: Record<string, Catalog> = { en: yaml.load(enText) as Catalog, zh_CN: yaml.load(zhText) as Catalog };
const lookup = (obj: any, key: string) => key.split(".").reduce((v, p) => v && typeof v === "object" ? v[p] : undefined, obj);
export function createTranslator(source: Record<string, Catalog> = catalogs) { return { t(key: string, locale = "en", vars: Record<string, string | number> = {}) { const value = lookup(source[locale], key) ?? lookup(source.en, key) ?? key; return Object.entries(vars).reduce((s, [k, v]) => s.replaceAll(`{${k}}`, String(v)), String(value)); } }; }
export const translator = createTranslator();
