import { Card } from "antd";
import { useTranslation } from "react-i18next";

/** 同步映射（Task 16 实现完整版） */
export default function Mappings() {
  const { t } = useTranslation();
  return <Card title={t("menu.mappings")}>{t("common.empty")}</Card>;
}
