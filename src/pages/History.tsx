import { Card } from "antd";
import { useTranslation } from "react-i18next";

/** 历史页（接线占位，完整实现见 V0.3 Task 7） */
export default function History() {
  const { t } = useTranslation();
  return (
    <Card title={t("history.title")}>{t("history.empty")}</Card>
  );
}
