import { useCallback, useEffect, useState } from "react";
import { App as AntApp, Button, Card, Space, Switch, Table, Tag, Tooltip } from "antd";
import { PauseCircleOutlined, PlayCircleOutlined } from "@ant-design/icons";
import { useTranslation } from "react-i18next";
import {
  mappingList,
  mappingSetEnabled,
  onStatusEvent,
  syncIsPaused,
  syncManual,
  syncSetPaused,
} from "../api/ipc";
import type { MappingStatus, MappingView } from "../types";
import ConflictDialog from "../components/ConflictDialog";

export function statusColor(status: MappingStatus): string {
  switch (status) {
    case "conflict":
      return "red";
    case "error":
      return "orange";
    case "missing":
      return "purple";
    case "uploading":
    case "downloading":
      return "blue";
    case "paused":
      return "default";
    default:
      return "green";
  }
}

function formatTime(iso: string | null): string {
  if (!iso) return "—";
  return iso.replace("T", " ").slice(0, 19);
}

export default function Dashboard() {
  const { t } = useTranslation();
  const { message } = AntApp.useApp();
  const [views, setViews] = useState<MappingView[]>([]);
  const [loading, setLoading] = useState(false);
  const [paused, setPaused] = useState(false);
  const [conflictView, setConflictView] = useState<MappingView | null>(null);

  const reload = useCallback(async () => {
    setLoading(true);
    try {
      setViews(await mappingList());
      setPaused(await syncIsPaused());
    } catch (e) {
      void message.error(String(e));
    } finally {
      setLoading(false);
    }
  }, [message]);

  useEffect(() => {
    void reload();
  }, [reload]);

  // 引擎状态事件 → 实时更新行内状态
  useEffect(() => {
    const unlisten = onStatusEvent((event) => {
      setViews((prev) =>
        prev.map((v) =>
          v.mapping.id === event.mappingId
            ? { ...v, status: event.status, message: event.message, lastSyncedAt: event.lastSyncedAt ?? v.lastSyncedAt }
            : v,
        ),
      );
    }).catch((e) => {
      // 非 Tauri 环境（浏览器/测试）无事件通道；订阅失败降级为手动刷新
      console.warn("status event subscribe failed:", e);
      return null;
    });
    return () => {
      void unlisten.then((fn) => fn?.());
    };
  }, []);

  const manual = async (view: MappingView, direction: "upload" | "download") => {
    try {
      await syncManual(view.mapping.id, direction);
      await reload();
    } catch (e) {
      void message.error(String(e));
    }
  };

  const toggleEnabled = async (view: MappingView) => {
    try {
      await mappingSetEnabled(view.mapping.id, !view.mapping.enabled);
      await reload();
    } catch (e) {
      void message.error(String(e));
    }
  };

  return (
    <Card
      title={t("menu.dashboard")}
      extra={
        <Space>
          <span>{t("dashboard.globalPause")}</span>
          <Switch
            checked={paused}
            checkedChildren={<PauseCircleOutlined />}
            unCheckedChildren={<PlayCircleOutlined />}
            onChange={async (checked) => {
              setPaused(checked);
              await syncSetPaused(checked);
            }}
          />
        </Space>
      }
    >
      <Table<MappingView>
        rowKey={(v) => v.mapping.id}
        loading={loading}
        dataSource={views}
        pagination={false}
        locale={{ emptyText: t("common.empty") }}
        columns={[
          {
            title: t("dashboard.file"),
            render: (_, v) => (
              <Space direction="vertical" size={0}>
                <span>{v.mapping.localPath}</span>
                <span>→ {v.mapping.gistId} / {v.mapping.gistFilename}</span>
              </Space>
            ),
          },
          {
            title: t("dashboard.status"),
            dataIndex: "status",
            width: 110,
            render: (_, v) => (
              <Tooltip title={v.message}>
                <Tag color={statusColor(v.status)}>{t(`status.${v.status}`)}</Tag>
              </Tooltip>
            ),
          },
          {
            title: t("dashboard.lastSynced"),
            dataIndex: "lastSyncedAt",
            width: 170,
            render: (iso: string | null) => formatTime(iso),
          },
          {
            title: t("common.actions"),
            width: 300,
            render: (_, v) => (
              <Space>
                <Button size="small" onClick={() => void manual(v, "upload")}>
                  {t("dashboard.uploadNow")}
                </Button>
                <Button size="small" onClick={() => void manual(v, "download")}>
                  {t("dashboard.downloadNow")}
                </Button>
                {(v.status === "conflict" || v.status === "missing") && (
                  <Button
                    size="small"
                    type="primary"
                    danger={v.status === "conflict"}
                    onClick={() => setConflictView(v)}
                  >
                    {v.status === "conflict"
                      ? t("dashboard.resolve")
                      : t("dashboard.recover")}
                  </Button>
                )}
                <Button size="small" onClick={() => void toggleEnabled(v)}>
                  {v.mapping.enabled ? t("dashboard.pause") : t("dashboard.resume")}
                </Button>
              </Space>
            ),
          },
        ]}
      />

      {conflictView && (
        <ConflictDialog
          view={conflictView}
          onClose={() => setConflictView(null)}
          onResolved={() => {
            setConflictView(null);
            void reload();
          }}
        />
      )}
    </Card>
  );
}
