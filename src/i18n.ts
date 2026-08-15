import { useEffect } from "react";
import en from "./locales/en.json";
import zhCN from "./locales/zh-CN.json";

type MessageKey = keyof typeof en;

// The boot screen hands the window over to dsh's own page as soon as the
// server is up, so it follows the system language rather than dsh's language
// preference — that preference lives behind the server that has not started.
const locale = navigator.languages.some((language) => language.toLowerCase().startsWith("zh")) ? "zh-CN" : "en";
const catalog: Record<MessageKey, string> = locale === "zh-CN" ? zhCN : en;

export function useI18n() {
  useEffect(() => {
    document.documentElement.lang = locale;
  }, []);
  return { t: (key: MessageKey) => catalog[key] ?? en[key] };
}
