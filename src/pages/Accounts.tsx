import { useCallback, useEffect, useState } from "react";
import {
  Alert,
  App as AntApp,
  Button,
  Card,
  Form,
  Input,
  Modal,
  Popconfirm,
  Select,
  Space,
  Table,
  Tag,
} from "antd";
import { PlusOutlined } from "@ant-design/icons";
import { useTranslation } from "react-i18next";
import {
  accountCreate,
  accountDelete,
  accountList,
  accountTest,
  accountUpdate,
  IpcError,
} from "../api/ipc";
import { ipcErrorMessage } from "../i18n/errors";
import type { Account, AccountInput, ProviderKind } from "../types";

interface FormValues {
  name: string;
  provider: ProviderKind;
  baseUrl: string;
  token: string;
}

export default function Accounts() {
  const { t } = useTranslation();
  const { message, modal } = AntApp.useApp();
  const [accounts, setAccounts] = useState<Account[]>([]);
  const [loading, setLoading] = useState(false);
  const [modalOpen, setModalOpen] = useState(false);
  const [editing, setEditing] = useState<Account | null>(null);
  const [form] = Form.useForm<FormValues>();
  const provider = Form.useWatch("provider", form);

  const reload = useCallback(async () => {
    setLoading(true);
    try {
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

  const openCreate = () => {
    setEditing(null);
    form.resetFields();
    form.setFieldsValue({ provider: "github", baseUrl: "", token: "" });
    setModalOpen(true);
  };

  const openEdit = (account: Account) => {
    setEditing(account);
    form.resetFields();
    form.setFieldsValue({
      name: account.name,
      provider: account.provider,
      baseUrl: account.baseUrl,
      token: "",
    });
    setModalOpen(true);
  };

  const submit = async () => {
    const values = await form.validateFields().catch(() => null);
    if (!values) return; // 校验失败：antd 已在字段下方展示错误提示
    const input: AccountInput = {
      name: values.name,
      provider: values.provider,
      baseUrl: values.baseUrl ?? "",
      token: values.token ?? "",
    };
    try {
      if (editing) {
        await accountUpdate(editing.id, input);
      } else {
        await accountCreate(input);
      }
      setModalOpen(false);
      await reload();
    } catch (e) {
      void message.error(ipcErrorMessage(e, t));
    }
  };

  const testConnection = async (account: Account) => {
    try {
      const user = await accountTest(account.id);
      void message.success(t("accounts.testOk", { login: user.login }));
    } catch (e) {
      void message.error(`${t("accounts.testFailed")}: ${ipcErrorMessage(e, t)}`);
    }
  };

  const doDelete = async (account: Account, force: boolean) => {
    try {
      await accountDelete(account.id, force);
      await reload();
    } catch (e) {
      // 结构化错误（IpcError）：按 code 分支，不匹配文案
      if (e instanceof IpcError && e.code === "ACCOUNT_IN_USE") {
        modal.confirm({
          title: t("accounts.forceDelete"),
          content: t("accounts.forceDeleteConfirm", {
            count: e.count ?? 0,
          }),
          okText: t("accounts.forceDelete"),
          okButtonProps: { danger: true },
          onOk: () => doDelete(account, true),
        });
        return;
      }
      void message.error(ipcErrorMessage(e, t));
    }
  };

  return (
    <Card
      title={t("menu.accounts")}
      extra={
        <Button type="primary" icon={<PlusOutlined />} onClick={openCreate}>
          {t("accounts.create")}
        </Button>
      }
    >
      <Table<Account>
        rowKey="id"
        loading={loading}
        dataSource={accounts}
        pagination={false}
        locale={{ emptyText: t("common.empty") }}
        columns={[
          { title: t("accounts.name"), dataIndex: "name" },
          {
            title: t("accounts.provider"),
            dataIndex: "provider",
            width: 140,
            render: (p: ProviderKind) =>
              p === "github" ? (
                <Tag color="blue">{t("accounts.github")}</Tag>
              ) : (
                <Tag color="green">{t("accounts.opengist")}</Tag>
              ),
          },
          {
            title: t("accounts.baseUrl"),
            dataIndex: "baseUrl",
            render: (url: string, record) =>
              record.provider === "opengist" ? url : "—",
          },
          {
            title: t("common.edit"),
            width: 260,
            render: (_, record) => (
              <Space>
                <Button size="small" onClick={() => openEdit(record)}>
                  {t("common.edit")}
                </Button>
                <Button size="small" onClick={() => void testConnection(record)}>
                  {t("accounts.testConnection")}
                </Button>
                <Popconfirm
                  title={t("accounts.deleteConfirm")}
                  onConfirm={() => void doDelete(record, false)}
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

      <Modal
        title={editing ? t("accounts.edit") : t("accounts.create")}
        open={modalOpen}
        onCancel={() => setModalOpen(false)}
        onOk={() => void submit()}
        destroyOnHidden
      >
        <Alert
          type="info"
          showIcon
          message={t("accounts.tokenHint")}
          style={{ marginBottom: 16 }}
        />
        <Form form={form} layout="vertical">
          <Form.Item
            name="name"
            label={t("accounts.name")}
            rules={[{ required: true, message: t("accounts.nameRequired") }]}
          >
            <Input />
          </Form.Item>
          <Form.Item name="provider" label={t("accounts.provider")}>
            <Select
              options={[
                { value: "github", label: t("accounts.github") },
                { value: "opengist", label: t("accounts.opengist") },
              ]}
            />
          </Form.Item>
          {provider === "opengist" && (
            <Form.Item
              name="baseUrl"
              label={t("accounts.baseUrl")}
              rules={[{ required: true, message: t("accounts.baseUrlRequired") }]}
            >
              <Input placeholder="https://gist.example.com" />
            </Form.Item>
          )}
          <Form.Item
            name="token"
            label={t("accounts.token")}
            rules={[
              {
                required: !editing,
                message: t("accounts.tokenRequired"),
              },
            ]}
            extra={editing ? t("accounts.keepToken") : undefined}
          >
            <Input.Password autoComplete="new-password" />
          </Form.Item>
        </Form>
      </Modal>
    </Card>
  );
}
