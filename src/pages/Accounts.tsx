import { Card } from "antd";
import { useTranslation } from "react-i18next";

/** 账号管理（Task 15 实现完整版） */
export default function Accounts() {
  const { t } = useTranslation();
  return <Card title={t("menu.accounts")}>{t("common.empty")}</Card>;
}
