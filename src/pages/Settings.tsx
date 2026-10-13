import { Card } from "antd";
import { useTranslation } from "react-i18next";

/** 全局设置（Task 18 实现完整版） */
export default function Settings() {
  const { t } = useTranslation();
  return <Card title={t("menu.settings")}>{t("common.empty")}</Card>;
}
