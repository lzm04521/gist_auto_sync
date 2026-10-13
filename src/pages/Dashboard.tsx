import { Card } from "antd";
import { useTranslation } from "react-i18next";

/** 仪表盘（Task 17 实现完整版） */
export default function Dashboard() {
  const { t } = useTranslation();
  return <Card title={t("menu.dashboard")}>{t("common.empty")}</Card>;
}
