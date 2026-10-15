import { useCallback, useEffect, useState } from "react";
import { useNavigate } from "react-router-dom";
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
import { ipcErrorMessage } from "../i18n/errors";
import MappingFileCell from "../components/MappingFileCell";
import MappingForm from "../components/MappingForm";
import type { Account, Mapping, MappingView } from "../types";

export default function Mappings() {
  const { t } = useTranslation();
  const { message } = AntApp.useApp();
  const navigate = useNavigate();
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
      void message.error(ipcErrorMessage(e, t));
    } finally {
      setLoading(false);
    }
  }, [message, t]);

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
      void message.error(ipcErrorMessage(e, t));
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
            title: t("mappings.name"),
            render: (_, view) => <MappingFileCell mapping={view.mapping} />,
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
            title: t("common.actions"),
            render: (_, view) => (
              <Space>
                <Switch
                  checked={view.mapping.enabled}
                  onChange={(checked) => void toggleEnabled(view, checked)}
                />
                <Button
                  size="small"
                  onClick={() => navigate(`/history?mappingId=${view.mapping.id}`)}
                >
                  {t("menu.history")}
                </Button>
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
                      void message.error(ipcErrorMessage(e, t));
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
