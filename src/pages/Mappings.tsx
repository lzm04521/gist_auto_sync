import { useCallback, useEffect, useState } from "react";
import {
  App as AntApp,
  Button,
  Card,
  Popconfirm,
  Space,
  Switch,
  Table,
  Tag,
} from "antd";
import { PlusOutlined } from "@ant-design/icons";
import { useTranslation } from "react-i18next";
import {
  accountList,
  mappingDelete,
  mappingList,
  mappingSetEnabled,
} from "../api/ipc";
import MappingForm from "../components/MappingForm";
import type { Account, Mapping, MappingView } from "../types";

export default function Mappings() {
  const { t } = useTranslation();
  const { message } = AntApp.useApp();
  const [views, setViews] = useState<MappingView[]>([]);
  const [accounts, setAccounts] = useState<Account[]>([]);
  const [loading, setLoading] = useState(false);
  const [formOpen, setFormOpen] = useState(false);
  const [editing, setEditing] = useState<Mapping | null>(null);

  const reload = useCallback(async () => {
    setLoading(true);
    try {
      setViews(await mappingList());
      setAccounts(await accountList());
    } catch (e) {
      void message.error(String(e));
    } finally {
      setLoading(false);
    }
  }, [message]);

  useEffect(() => {
    void reload();
  }, [reload]);

  const accountName = (id: string) =>
    accounts.find((a) => a.id === id)?.name ?? id;

  const toggleEnabled = async (view: MappingView, enabled: boolean) => {
    try {
      await mappingSetEnabled(view.mapping.id, enabled);
      await reload();
    } catch (e) {
      void message.error(String(e));
    }
  };

  return (
    <Card
      title={t("menu.mappings")}
      extra={
        <Button
          type="primary"
          icon={<PlusOutlined />}
          onClick={() => {
            setEditing(null);
            setFormOpen(true);
          }}
        >
          {t("mappings.create")}
        </Button>
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
            title: t("mappings.localPath"),
            dataIndex: ["mapping", "localPath"],
            render: (_, view) => (
              <Space direction="vertical" size={0}>
                <span>{view.mapping.localPath}</span>
                <span>→ {view.mapping.gistId} / {view.mapping.gistFilename}</span>
              </Space>
            ),
          },
          {
            title: t("mappings.account"),
            dataIndex: ["mapping", "accountId"],
            render: (id: string) => accountName(id),
          },
          {
            title: t("mappings.direction"),
            render: (_, view) => (
              <Space>
                {view.mapping.uploadEnabled && <Tag color="blue">↑ {t("mappings.uploadEnabled")}</Tag>}
                {view.mapping.downloadEnabled && <Tag color="green">↓ {t("mappings.downloadEnabled")}</Tag>}
                {!view.mapping.uploadEnabled && !view.mapping.downloadEnabled && <Tag>—</Tag>}
              </Space>
            ),
          },
          {
            title: t("mappings.edit"),
            render: (_, view) => (
              <Space>
                <Switch
                  checked={view.mapping.enabled}
                  onChange={(checked) => void toggleEnabled(view, checked)}
                />
                <Button
                  size="small"
                  onClick={() => {
                    setEditing(view.mapping);
                    setFormOpen(true);
                  }}
                >
                  {t("common.edit")}
                </Button>
                <Popconfirm
                  title={t("common.delete")}
                  onConfirm={async () => {
                    try {
                      await mappingDelete(view.mapping.id);
                      await reload();
                    } catch (e) {
                      void message.error(String(e));
                    }
                  }}
                >
                  <Button size="small" danger>
                    {t("common.delete")}
                  </Button>
                </Popconfirm>
              </Space>
            ),
          },
        ]}
      />

      <MappingForm
        open={formOpen}
        editing={editing}
        onCancel={() => setFormOpen(false)}
        onSaved={() => {
          setFormOpen(false);
          void reload();
        }}
      />
    </Card>
  );
}
