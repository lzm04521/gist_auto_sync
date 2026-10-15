import { useEffect, useState } from "react";
import { App as AntApp, Button, Form, Input, Modal, Radio, Select, Switch } from "antd";
import { FolderOpenOutlined } from "@ant-design/icons";
import { open as openFileDialog } from "@tauri-apps/plugin-dialog";
import { useTranslation } from "react-i18next";
import { accountList, gistListForAccount, mappingCreate, mappingUpdate } from "../api/ipc";
import { ipcErrorMessage } from "../i18n/errors";
import type { Account, GistSummary, Mapping } from "../types";

export interface FormValues {
  name?: string;
  localPath: string;
  accountId: string;
  gistMode: "select" | "manual";
  gistId: string;
  gistFilename: string;
  uploadEnabled: boolean;
  downloadEnabled: boolean;
}

/** 取路径最后一段作为文件名（兼容 / 与 \） */
export function fileNameOf(path: string): string {
  const parts = path.split(/[\\/]/).filter(Boolean);
  return parts.length > 0 ? parts[parts.length - 1] : path;
}

/** gist 下拉选项文案：标题 ?? 描述（超 30 字符截断）· 首文件名（多文件计数）· 日期 */
export function gistOptionLabel(g: GistSummary): string {
  const truncate = (s: string) => {
    const chars = [...s];
    return chars.length > 30 ? `${chars.slice(0, 30).join("")}…` : s;
  };
  const primary = g.title ?? g.description;
  const parts: string[] = [primary ? truncate(primary) : `#${g.id.slice(0, 8)}`];
  if (g.files.length > 0) {
    parts.push(g.files.length > 1 ? `${g.files[0]} +${g.files.length - 1}` : g.files[0]);
  }
  parts.push(g.updatedAt.slice(0, 10));
  return parts.join(" · ");
}

interface Props {
  open: boolean;
  editing: Mapping | null;
  onCancel: () => void;
  onSaved: () => void;
}

export default function MappingForm({ open, editing, onCancel, onSaved }: Props) {
  const { t } = useTranslation();
  const { message } = AntApp.useApp();
  const [accounts, setAccounts] = useState<Account[]>([]);
  const [gists, setGists] = useState<GistSummary[]>([]);
  const [loadingGists, setLoadingGists] = useState(false);
  // 名称是否被用户手动编辑过：true 后不再自动跟随文件名（用户意图优先）
  const [nameDirty, setNameDirty] = useState(false);
  const [form] = Form.useForm<FormValues>();
  const accountId = Form.useWatch("accountId", form);
  const gistMode = Form.useWatch("gistMode", form);
  const localPath = Form.useWatch("localPath", form);
  const gistFilename = Form.useWatch("gistFilename", form);

  useEffect(() => {
    void accountList()
      .then(setAccounts)
      .catch((e) => void message.error(ipcErrorMessage(e, t)));
  }, [message, t]);

  // 账号切换 → 拉取 gist 列表
  useEffect(() => {
    if (!open || !accountId) return;
    let cancelled = false;
    setLoadingGists(true);
    gistListForAccount(accountId)
      .then((list) => {
        if (!cancelled) setGists(list);
      })
      .catch((e) => {
        if (!cancelled) {
          setGists([]);
          void message.error(`${t("mappings.loadingGists")}: ${ipcErrorMessage(e, t)}`);
        }
      })
      .finally(() => {
        if (!cancelled) setLoadingGists(false);
      });
    return () => {
      cancelled = true;
    };
  }, [open, accountId, message, t]);

  useEffect(() => {
    if (!open) return;
    if (editing) {
      form.setFieldsValue({
        name: editing.name,
        localPath: editing.localPath,
        accountId: editing.accountId,
        gistMode: "manual",
        gistId: editing.gistId,
        gistFilename: editing.gistFilename,
        uploadEnabled: editing.uploadEnabled,
        downloadEnabled: editing.downloadEnabled,
      });
    } else {
      form.resetFields();
      setNameDirty(false);
      form.setFieldsValue({
        gistMode: "select",
        uploadEnabled: true,
        downloadEnabled: true,
      });
    }
  }, [open, editing, form]);

  // 本地路径变化时（未手改文件名）同步默认文件名
  useEffect(() => {
    if (!open || editing || !localPath) return;
    form.setFieldValue("gistFilename", fileNameOf(localPath));
  }, [localPath, open, editing, form]);

  // Gist 内文件名变化时，未被手改过的名称自动跟随
  useEffect(() => {
    if (!open || editing || !gistFilename || nameDirty) return;
    form.setFieldValue("name", gistFilename);
  }, [gistFilename, open, editing, nameDirty, form]);

  const chooseFile = async () => {
    try {
      const path = await openFileDialog({ multiple: false, directory: false });
      if (typeof path === "string" && path) {
        form.setFieldsValue({ localPath: path });
      }
    } catch (e) {
      void message.error(ipcErrorMessage(e, t));
    }
  };

  const submit = async () => {
    const values = await form.validateFields().catch(() => null);
    if (!values) return; // 校验失败：antd 已在字段下方展示错误提示
    try {
      const payload = {
        name: values.name ?? "",
        localPath: values.localPath,
        accountId: values.accountId,
        gistId: values.gistId,
        gistFilename: values.gistFilename ?? "",
        uploadEnabled: values.uploadEnabled,
        downloadEnabled: values.downloadEnabled,
        enabled: editing ? editing.enabled : true,
      };
      if (editing) {
        await mappingUpdate(editing.id, payload);
      } else {
        await mappingCreate(payload);
      }
      onSaved();
    } catch (e) {
      void message.error(ipcErrorMessage(e, t));
    }
  };

  return (
    <Modal
      title={editing ? t("mappings.edit") : t("mappings.create")}
      open={open}
      onCancel={onCancel}
      onOk={() => void submit()}
      destroyOnHidden
    >
      <Form form={form} layout="vertical">
        <Form.Item
          name="localPath"
          label={t("mappings.localPath")}
          rules={[{ required: true, message: t("mappings.localPathRequired") }]}
        >
          <Input
            addonAfter={
              <Button
                type="text"
                size="small"
                icon={<FolderOpenOutlined />}
                onClick={() => void chooseFile()}
              >
                {t("mappings.chooseFile")}
              </Button>
            }
          />
        </Form.Item>
        <Form.Item
          name="accountId"
          label={t("mappings.account")}
          rules={[{ required: true, message: t("mappings.accountRequired") }]}
        >
          <Select
            showSearch
            optionFilterProp="label"
            options={accounts.map((a) => ({
              value: a.id,
              label: `${a.name}（${a.provider === "github" ? "GitHub" : "Opengist"}）`,
            }))}
          />
        </Form.Item>
        <Form.Item name="gistMode" label={t("mappings.gist")}>
          <Radio.Group
            options={[
              { value: "select", label: t("mappings.gistSelect") },
              { value: "manual", label: t("mappings.gistManual") },
            ]}
          />
        </Form.Item>
        <Form.Item
          name="gistId"
          label={t("mappings.gist")}
          rules={[{ required: true, message: t("mappings.gistIdRequired") }]}
        >
          {gistMode === "manual" ? (
            <Input placeholder="gist id" />
          ) : (
            <Select
              showSearch
              loading={loadingGists}
              optionFilterProp="label"
              placeholder={t("mappings.loadingGists")}
              options={gists.map((g) => ({
                value: g.id,
                label: gistOptionLabel(g),
              }))}
            />
          )}
        </Form.Item>
        <Form.Item
          name="gistFilename"
          label={t("mappings.gistFilename")}
          extra={t("mappings.gistFilenameHint")}
        >
          <Input />
        </Form.Item>
        <Form.Item
          name="name"
          label={t("mappings.name")}
          extra={t("mappings.nameHint")}
        >
          <Input onChange={() => setNameDirty(true)} />
        </Form.Item>
        <Form.Item label={t("mappings.direction")} style={{ marginBottom: 0 }}>
          <Form.Item name="uploadEnabled" valuePropName="checked" style={{ display: "inline-block", marginRight: 24 }} noStyle={false}>
            <Switch checkedChildren={t("mappings.uploadEnabled")} unCheckedChildren={t("mappings.uploadEnabled")} />
          </Form.Item>
          <Form.Item name="downloadEnabled" valuePropName="checked" style={{ display: "inline-block" }} noStyle={false}>
            <Switch checkedChildren={t("mappings.downloadEnabled")} unCheckedChildren={t("mappings.downloadEnabled")} />
          </Form.Item>
        </Form.Item>
      </Form>
    </Modal>
  );
}
