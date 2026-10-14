import i18n from "i18next";
import { initReactI18next } from "react-i18next";
import en from "./en.json";
import zhCN from "./zh-CN.json";

void i18n.use(initReactI18next).init({
  resources: {
    "zh-CN": { translation: zhCN },
    en: { translation: en },
  },
  lng: "zh-CN",
  fallbackLng: "en",
  interpolation: { escapeValue: false },
});

/** 启动时按全局设置切换语言（失败静默保持默认 zh-CN） */
export async function initLanguage(): Promise<void> {
  try {
    const { settingsGet } = await import("../api/ipc");
    const settings = await settingsGet();
    await i18n.changeLanguage(settings.language);
  } catch {
    // 非 Tauri 环境（测试）或读取失败：保持默认
  }
}

export default i18n;
