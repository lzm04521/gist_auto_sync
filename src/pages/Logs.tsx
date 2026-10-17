import { useCallback, useEffect, useState } from "react";
import { App as AntApp, Button, Card, Space, Table, Tag } from "antd";
import { ReloadOutlined } from "@ant-design/icons";
import { useTranslation } from "react-i18next";
import { syncLogList } from "../api/ipc";
import { ipcErrorMessage } from "../i18n/errors";
import type { LogDirection, LogOutcome, LogTrigger, SyncLogEntry } from "../types";
import { formatTime } from "../time";

const isManual = (t: LogTrigger) => t === "manualUpload" || t === "manualDownload";

const triggerNs: Record<LogTrigger, string> = {
  localSettled: "logs.autoLocal",
  poll: "logs.autoPoll",
  manualUpload: "logs.manual",
  manualDownload: "logs.manual",
};

function DirectionTag({ direction, label }: { direction: LogDirection | null; label: (k: string) => string }) {
  if (direction === "upload") return <Tag color="blue">↑ {label("logs.upload")}</Tag>;
  if (direction === "download") return <Tag color="green">↓ {label("logs.download")}</Tag>;
  return <Tag>—</Tag>;
}

/** 结果：+N 绿 / -M 红；增删均空（超限未统计）只显示"有变化" */
function OutcomeView({ outcome, label }: { outcome: LogOutcome; label: (k: string) => string }) {
  switch (outcome.kind) {
    case "changed":
      if (outcome.added == null && outcome.removed == null) {
        return <span>{label("logs.changed")}</span>;
      }
      return (
        <Space size={6}>
          {outcome.added != null && <span style={{ color: "#389e0d" }}>+{outcome.added}</span>}
          {outcome.removed != null && <span style={{ color: "#cf1322" }}>-{outcome.removed}</span>}
        </Space>
      );
    case "noChange":
      return <Tag>{label("logs.noChange")}</Tag>;
    case "conflict":
      return <Tag color="red">{label("logs.conflict")}</Tag>;
    case "error":
      return (
        <Tag color="orange" title={outcome.code}>
          {label("logs.failed")}
        </Tag>
      );
  }
}

/** 同步操作日志：上传/下载流水，区分自动（文件变更/轮询）与手动 */
export default function Logs() {
  const { t } = useTranslation();
  const { message } = AntApp.useApp();
  const [entries, setEntries] = useState<SyncLogEntry[]>([]);
  const [loading, setLoading] = useState(false);

  const reload = useCallback(async () => {
    setLoading(true);
    try {
      setEntries(await syncLogList());
    } catch (e) {
      void message.error(ipcErrorMessage(e, t));
    } finally {
      setLoading(false);
    }
  }, [message, t]);

  useEffect(() => {
    void reload();
  }, [reload]);

  return (
    <Card
      title={t("logs.title")}
      extra={
        <Button icon={<ReloadOutlined />} onClick={() => void reload()}>
          {t("logs.refresh")}
        </Button>
      }
    >
      <Table<SyncLogEntry>
        rowKey={(e) => `${e.ts}-${e.mappingId}`}
        loading={loading}
        dataSource={entries}
        pagination={{ pageSize: 50, showSizeChanger: false }}
        locale={{ emptyText: t("logs.empty") }}
        columns={[
          {
            title: t("logs.time"),
            dataIndex: "ts",
            width: 170,
            render: (iso: string) => formatTime(iso),
          },
          {
            title: t("logs.mapping"),
            dataIndex: "label",
          },
          {
            title: t("logs.direction"),
            width: 110,
            render: (_, e) => <DirectionTag direction={e.direction} label={t} />,
          },
          {
            title: t("logs.trigger"),
            width: 150,
            filters: [
              { text: t("logs.autoLocal"), value: "localSettled" },
              { text: t("logs.autoPoll"), value: "poll" },
              { text: t("logs.manual"), value: "manual" },
            ],
            onFilter: (value, e) =>
              value === "manual" ? isManual(e.trigger) : e.trigger === value,
            render: (_, e) => (
              <Tag color={isManual(e.trigger) ? "purple" : "cyan"}>{t(triggerNs[e.trigger])}</Tag>
            ),
          },
          {
            title: t("logs.result"),
            width: 130,
            filters: [
              { text: t("logs.changed"), value: "changed" },
              { text: t("logs.noChange"), value: "noChange" },
              { text: t("logs.conflict"), value: "conflict" },
              { text: t("logs.failed"), value: "error" },
            ],
            onFilter: (value, e) => e.outcome.kind === value,
            render: (_, e) => <OutcomeView outcome={e.outcome} label={t} />,
          },
        ]}
      />
    </Card>
  );
}
