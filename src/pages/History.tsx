import { useCallback, useEffect, useState } from "react";
import { useSearchParams } from "react-router-dom";
import {
  App as AntApp,
  Button,
  Card,
  Modal,
  Select,
  Space,
  Table,
  Tabs,
  Tag,
} from "antd";
import { useTranslation } from "react-i18next";
import {
  gistCommitList,
  gistCommitPreview,
  historyList,
  historyPreview,
  historyRollbackLocal,
  historyRollbackRemote,
  mappingList,
} from "../api/ipc";
import { ipcErrorMessage } from "../i18n/errors";
import DiffView from "../components/DiffView";
import { mappingDisplayName } from "../components/MappingFileCell";
import type { GistCommit, HistoryEntry, HistoryReason, MappingView } from "../types";

interface PreviewText {
  historical: string;
  currentLocal: string;
}

/** 历史页：本地归档 + 远端 commits 双 Tab，行级对比与一键回滚（V0.3） */
export default function History() {
  const { t } = useTranslation();
  const { message, modal } = AntApp.useApp();
  const [searchParams] = useSearchParams();
  const [views, setViews] = useState<MappingView[]>([]);
  const [mappingId, setMappingId] = useState<string | null>(null);
  const [entries, setEntries] = useState<HistoryEntry[]>([]);
  const [commits, setCommits] = useState<GistCommit[]>([]);
  const [loadingLists, setLoadingLists] = useState(false);
  const [previewOpen, setPreviewOpen] = useState(false);
  const [previewText, setPreviewText] = useState<PreviewText | null>(null);

  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const v = await mappingList();
        if (cancelled) return;
        setViews(v);
        const fromQuery = searchParams.get("mappingId");
        setMappingId(
          v.some((x) => x.mapping.id === fromQuery)
            ? fromQuery
            : (v[0]?.mapping.id ?? null),
        );
      } catch (e) {
        void message.error(ipcErrorMessage(e, t));
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [message, t, searchParams]);

  const reloadLists = useCallback(
    async (id: string) => {
      setLoadingLists(true);
      // 两个数据源独立容错：远端不可达不影响本地归档展示
      try {
        setEntries(await historyList(id));
      } catch (e) {
        void message.error(ipcErrorMessage(e, t));
      }
      try {
        setCommits(await gistCommitList(id));
      } catch (e) {
        void message.error(ipcErrorMessage(e, t));
      }
      setLoadingLists(false);
    },
    [message, t],
  );

  useEffect(() => {
    if (mappingId) void reloadLists(mappingId);
  }, [mappingId, reloadLists]);

  const fmtTime = (iso: string) =>
    new Date(iso).toISOString().replace("T", " ").slice(0, 19);

  const fmtBytes = (n: number) =>
    n < 1024
      ? `${n} B`
      : n < 1024 * 1024
        ? `${(n / 1024).toFixed(1)} KB`
        : `${(n / 1024 / 1024).toFixed(1)} MB`;

  const sideTag = (side: "local" | "remote") =>
    side === "local" ? (
      <Tag>{t("history.sideLocal")}</Tag>
    ) : (
      <Tag>{t("history.sideRemote")}</Tag>
    );

  const reasonNs: Record<HistoryReason, string> = {
    download: "history.reasonDownload",
    upload: "history.reasonUpload",
    rollback: "history.reasonRollback",
    legacy: "history.reasonLegacy",
  };

  const openPreview = async (fetcher: () => Promise<PreviewText>) => {
    try {
      setPreviewText(await fetcher());
      setPreviewOpen(true);
    } catch (e) {
      void message.error(ipcErrorMessage(e, t));
    }
  };

  const doRollback = (rollback: () => Promise<string>) => {
    modal.confirm({
      title: t("history.rollbackConfirmTitle"),
      content: t("history.rollbackConfirmBody"),
      okText: t("common.ok"),
      cancelText: t("common.cancel"),
      onOk: async () => {
        try {
          const status = await rollback();
          if (status === "error") {
            // 引擎级失败已记入映射状态（rollback.failed 等）：不弹成功，提示查看状态
            void message.error(t("history.rollbackFailed"));
          } else {
            void message.success(t("history.rollbackOk"));
          }
          if (mappingId) await reloadLists(mappingId);
        } catch (e) {
          void message.error(ipcErrorMessage(e, t));
        }
      },
    });
  };

  return (
    <Card
      title={t("history.title")}
      extra={
        <Select
          style={{ minWidth: 320 }}
          value={mappingId ?? undefined}
          placeholder={t("history.mapping")}
          showSearch
          optionFilterProp="label"
          onChange={(v) => setMappingId(v)}
          options={views.map((v) => ({
            value: v.mapping.id,
            label: mappingDisplayName(v.mapping),
          }))}
        />
      }
    >
      <Tabs
        items={[
          {
            key: "history",
            label: t("history.tabLocal"),
            children: (
              <Table<HistoryEntry>
                rowKey="file"
                loading={loadingLists}
                dataSource={entries}
                pagination={{ pageSize: 20, showSizeChanger: false }}
                locale={{ emptyText: t("history.empty") }}
                columns={[
                  {
                    title: t("history.colTime"),
                    render: (_, e) => fmtTime(e.ts),
                  },
                  {
                    title: t("history.colSide"),
                    render: (_, e) => sideTag(e.side),
                  },
                  {
                    title: t("history.colReason"),
                    render: (_, e) => <Tag>{t(reasonNs[e.reason])}</Tag>,
                  },
                  {
                    title: t("history.colSize"),
                    render: (_, e) => fmtBytes(e.sizeBytes),
                  },
                  {
                    title: t("history.colFile"),
                    render: (_, e) => (
                      <span style={{ fontSize: 12 }}>{e.file}</span>
                    ),
                  },
                  {
                    title: t("common.actions"),
                    render: (_, e) => (
                      <Space>
                        <Button
                          size="small"
                          onClick={() =>
                            void openPreview(() =>
                              historyPreview(mappingId ?? "", e.file),
                            )
                          }
                        >
                          {t("history.compare")}
                        </Button>
                        <Button
                          size="small"
                          onClick={() =>
                            doRollback(() =>
                              historyRollbackLocal(mappingId ?? "", e.file),
                            )
                          }
                        >
                          {t("history.rollback")}
                        </Button>
                      </Space>
                    ),
                  },
                ]}
              />
            ),
          },
          {
            key: "commits",
            label: t("history.tabCommits"),
            children: (
              <Table<GistCommit>
                rowKey="revision"
                loading={loadingLists}
                dataSource={commits}
                pagination={{ pageSize: 20, showSizeChanger: false }}
                locale={{ emptyText: t("history.empty") }}
                columns={[
                  {
                    title: t("history.colRevision"),
                    render: (_, c) => <Tag>{c.revision.slice(0, 7)}</Tag>,
                  },
                  {
                    title: t("history.colCommittedAt"),
                    render: (_, c) => fmtTime(c.committedAt),
                  },
                  {
                    title: t("common.actions"),
                    render: (_, c) => (
                      <Space>
                        <Button
                          size="small"
                          onClick={() =>
                            void openPreview(() =>
                              gistCommitPreview(mappingId ?? "", c.revision),
                            )
                          }
                        >
                          {t("history.compare")}
                        </Button>
                        <Button
                          size="small"
                          onClick={() =>
                            doRollback(() =>
                              historyRollbackRemote(mappingId ?? "", c.revision),
                            )
                          }
                        >
                          {t("history.rollback")}
                        </Button>
                      </Space>
                    ),
                  },
                ]}
              />
            ),
          },
        ]}
      />

      <Modal
        open={previewOpen}
        title={t("history.compare")}
        footer={null}
        width={820}
        onCancel={() => setPreviewOpen(false)}
      >
        {previewText && (
          <>
            <div style={{ display: "flex", gap: 8, marginBottom: 8 }}>
              <span style={{ flex: 1, fontWeight: 600 }}>
                {t("history.previewHistorical")}
              </span>
              <span style={{ flex: 1, fontWeight: 600 }}>
                {t("history.previewCurrent")}
              </span>
            </div>
            <DiffView
              local={previewText.historical}
              remote={previewText.currentLocal}
            />
          </>
        )}
      </Modal>
    </Card>
  );
}
