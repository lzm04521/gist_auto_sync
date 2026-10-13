import { useEffect, useState } from "react";
import { Alert, App as AntApp, Button, Modal, Space } from "antd";
import { useTranslation } from "react-i18next";
import { conflictPreview, syncManual } from "../api/ipc";
import type { ConflictPreview } from "../api/ipc";
import type { MappingView } from "../types";
import DiffView from "./DiffView";

interface Props {
  view: MappingView;
  onClose: () => void;
  onResolved: () => void;
}

/** 冲突三选一：本地覆盖远端 / 远端覆盖本地 / 取消（覆盖一侧自动存历史） */
export default function ConflictDialog({ view, onClose, onResolved }: Props) {
  const { t } = useTranslation();
  const { message } = AntApp.useApp();
  const [preview, setPreview] = useState<ConflictPreview | null>(null);

  useEffect(() => {
    let cancelled = false;
    setPreview(null);
    conflictPreview(view.mapping.id)
      .then((p) => {
        if (!cancelled) setPreview(p);
      })
      .catch((e) => void message.error(String(e)));
    return () => {
      cancelled = true;
    };
  }, [view.mapping.id, message]);

  const resolve = async (direction: "upload" | "download") => {
    try {
      await syncManual(view.mapping.id, direction);
      onResolved();
    } catch (e) {
      void message.error(String(e));
    }
  };

  return (
    <Modal
      title={`${t("conflict.title")}：${view.mapping.gistFilename}`}
      open
      onCancel={onClose}
      footer={
        <Space>
          <Button onClick={onClose}>{t("common.cancel")}</Button>
          <Button danger onClick={() => void resolve("upload")}>
            {t("conflict.useLocal")}
          </Button>
          <Button type="primary" onClick={() => void resolve("download")}>
            {t("conflict.useRemote")}
          </Button>
        </Space>
      }
      width={760}
    >
      <Alert type="warning" showIcon message={t("conflict.desc")} style={{ marginBottom: 12 }} />
      <div style={{ display: "flex", gap: 8, marginBottom: 4, fontWeight: 600 }}>
        <span style={{ flex: 1 }}>{t("conflict.local")}</span>
        <span style={{ flex: 1 }}>{t("conflict.remote")}</span>
      </div>
      <DiffView local={preview?.local ?? ""} remote={preview?.remote ?? ""} />
    </Modal>
  );
}
