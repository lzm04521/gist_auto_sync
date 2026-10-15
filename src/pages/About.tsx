import { useEffect, useRef, useState } from "react";
import { App as AntApp, Button, Card, Modal, Progress, Switch, Typography } from "antd";
import { SyncOutlined } from "@ant-design/icons";
import { useTranslation } from "react-i18next";
import { getVersion } from "@tauri-apps/api/app";
import { settingsGet, settingsUpdate } from "../api/ipc";
import { ipcErrorMessage } from "../i18n/errors";
import { useUpdater } from "../hooks/useUpdater";
import type { GlobalSettings } from "../types";

export default function About() {
  const { t } = useTranslation();
  const { message } = AntApp.useApp();
  const updater = useUpdater();
  const [version, setVersion] = useState("");
  const [settings, setSettings] = useState<GlobalSettings | null>(null);
  const [modalOpen, setModalOpen] = useState(false);
  // failed 提示需区分来源（检查失败 / 下载失败），await 后闭包拿不到最新状态，用 ref 记录
  const lastActionRef = useRef<"check" | "download">("check");

  useEffect(() => {
    getVersion()
      .then((v) => setVersion(v))
      .catch(() => setVersion("unknown"));
    settingsGet()
      .then((s) => setSettings(s))
      .catch((e) => void message.error(ipcErrorMessage(e, t)));
  }, [message, t]);

  // 手动检查的结果驱动（启动自动检查独立走 useStartupAutoCheck，不经此处）
  useEffect(() => {
    if (updater.status === "available" || updater.status === "downloaded") {
      // downloaded 也重开弹窗：下载中关闭弹窗的用户不失去"立即重启"入口
      setModalOpen(true);
    } else if (updater.status === "upToDate") {
      void message.success(t("about.upToDate"));
    } else if (updater.status === "failed") {
      const key =
        lastActionRef.current === "download" ? "about.downloadFailed" : "about.checkFailed";
      void message.error(updater.error ? `${t(key)}: ${updater.error}` : t(key));
    }
  }, [updater.status, updater.error, message, t]);

  const onCheck = async () => {
    // 已下载待重启：重开弹窗即可，不重复检查下载同一版本（插件按运行版本比较，拦不住重复）
    if (updater.status === "downloaded") {
      setModalOpen(true);
      return;
    }
    lastActionRef.current = "check";
    await updater.check();
  };

  const onUpdate = async () => {
    lastActionRef.current = "download";
    // 成功/失败提示由 status effect 统一给出，避免双重提示
    await updater.downloadAndInstall();
  };

  const closeModal = () => {
    setModalOpen(false);
    // 下载中/已下载不重置：在飞下载完成后由 effect 重开弹窗，避免与后台更新脱钩
    if (updater.status !== "downloading" && updater.status !== "downloaded") {
      updater.reset();
    }
  };

  const onAutoCheckChange = async (checked: boolean) => {
    if (!settings) return;
    const next = { ...settings, autoCheckUpdate: checked };
    try {
      await settingsUpdate(next);
      setSettings(next);
      void message.success(t("settings.saved"));
    } catch (e) {
      void message.error(ipcErrorMessage(e, t));
    }
  };

  const downloading = updater.status === "downloading" || updater.status === "downloaded";

  return (
    <Card title={t("about.title")} style={{ margin: 24, maxWidth: 560 }}>
      <Typography.Paragraph style={{ fontSize: 20, fontWeight: 600 }}>
        Gist Auto Sync
      </Typography.Paragraph>
      <Typography.Paragraph>
        {t("about.version")}：<span>{version || "…"}</span>
      </Typography.Paragraph>
      <div style={{ display: "flex", alignItems: "center", gap: 12 }}>
        <Button
          type="primary"
          icon={<SyncOutlined />}
          loading={updater.status === "checking"}
          onClick={() => void onCheck()}
        >
          {t("about.checkUpdate")}
        </Button>
        <Switch
          aria-label={t("about.autoCheck")}
          checked={settings?.autoCheckUpdate ?? false}
          loading={!settings}
          onChange={(checked) => void onAutoCheckChange(checked)}
        />
        <span>{t("about.autoCheck")}</span>
      </div>

      <Modal
        title={t("about.newVersion", { version: updater.version ?? "" })}
        open={modalOpen}
        onCancel={closeModal}
        footer={null}
        destroyOnClose
      >
        {/* 更新说明一律纯文本呈现，防止远端内容注入 */}
        <Typography.Paragraph style={{ whiteSpace: "pre-wrap" }}>
          {updater.notes ?? ""}
        </Typography.Paragraph>
        {downloading ? <Progress percent={updater.progress ?? 0} /> : null}
        {updater.status === "downloaded" ? (
          <Typography.Paragraph>{t("about.relaunchHint")}</Typography.Paragraph>
        ) : null}
        <div style={{ display: "flex", gap: 12, justifyContent: "flex-end" }}>
          {updater.status === "downloaded" ? (
            <Button type="primary" onClick={() => updater.relaunch()}>
              {t("about.relaunchNow")}
            </Button>
          ) : (
            <Button
              type="primary"
              loading={updater.status === "downloading"}
              onClick={() => void onUpdate()}
            >
              {t("about.updateNow")}
            </Button>
          )}
          <Button onClick={closeModal}>{t("about.later")}</Button>
        </div>
      </Modal>
    </Card>
  );
}
