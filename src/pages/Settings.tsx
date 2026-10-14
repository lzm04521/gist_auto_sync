import { useEffect, useState } from "react";
import {
  App as AntApp,
  Button,
  Card,
  Form,
  InputNumber,
  Select,
  Switch,
} from "antd";
import { FolderOpenOutlined, SaveOutlined } from "@ant-design/icons";
import { useTranslation } from "react-i18next";
import { appOpenLogDir, settingsGet, settingsUpdate } from "../api/ipc";
import { ipcErrorMessage } from "../i18n/errors";
import type { GlobalSettings } from "../types";

interface FormValues {
  debounceSeconds: number;
  pollIntervalMinutes: number;
  autoStart: boolean;
  logLevel: string;
  language: string;
}

export default function Settings() {
  const { t, i18n } = useTranslation();
  const { message } = AntApp.useApp();
  const [form] = Form.useForm<FormValues>();
  const [loaded, setLoaded] = useState(false);

  useEffect(() => {
    settingsGet()
      .then((s) => {
        form.setFieldsValue(s);
        setLoaded(true);
      })
      .catch((e) => void message.error(ipcErrorMessage(e, t)));
  }, [form, message]);

  const onLanguageChange = (lang: string) => {
    // 即时切换显示语言（保存后才持久化）
    void i18n.changeLanguage(lang);
    form.setFieldValue("language", lang);
  };

  const save = async () => {
    const values = await form.validateFields().catch(() => null);
    if (!values) return; // 校验失败：antd 已在字段下方展示错误提示
    try {
      const settings: GlobalSettings = {
        debounceSeconds: values.debounceSeconds,
        pollIntervalMinutes: values.pollIntervalMinutes,
        autoStart: values.autoStart,
        logLevel: values.logLevel,
        language: values.language,
      };
      await settingsUpdate(settings);
      void message.success(t("settings.saved"));
    } catch (e) {
      void message.error(ipcErrorMessage(e, t));
    }
  };

  return (
    <Card title={t("menu.settings")} extra={
      <Button type="primary" icon={<SaveOutlined />} disabled={!loaded} onClick={() => void save()}>
        {t("common.save")}
      </Button>
    }>
      <Form form={form} layout="vertical" style={{ maxWidth: 560 }}>
        <Form.Item
          name="debounceSeconds"
          label={t("settings.debounce")}
          extra={t("settings.debounceExtra")}
          rules={[
            { required: true },
            {
              validator: (_, v: number) =>
                v >= 1 && v <= 3600
                  ? Promise.resolve()
                  : Promise.reject(new Error(t("settings.debounceRange"))),
            },
          ]}
        >
          {/* 不设 min/max：越界输入交给 validator 统一拦截并给出中文提示 */}
          <InputNumber step={1} style={{ width: "100%" }} />
        </Form.Item>
        <Form.Item
          name="pollIntervalMinutes"
          label={t("settings.poll")}
          extra={t("settings.pollExtra")}
          rules={[
            { required: true },
            {
              validator: (_, v: number) =>
                v >= 1 && v <= 1440
                  ? Promise.resolve()
                  : Promise.reject(new Error(t("settings.pollRange"))),
            },
          ]}
        >
          <InputNumber step={1} style={{ width: "100%" }} />
        </Form.Item>
        <Form.Item name="autoStart" label={t("settings.autoStart")} valuePropName="checked">
          <Switch />
        </Form.Item>
        <Form.Item name="logLevel" label={t("settings.logLevel")}>
          <Select
            options={[
              { value: "info", label: "INFO" },
              { value: "warn", label: "WARN" },
              { value: "error", label: "ERROR" },
            ]}
          />
        </Form.Item>
        <Form.Item name="language" label={t("settings.language")}>
          <Select
            onChange={onLanguageChange}
            options={[
              { value: "zh-CN", label: "简体中文" },
              { value: "en", label: "English" },
            ]}
          />
        </Form.Item>
        <Form.Item label={t("settings.logLevel")}>
          <Button icon={<FolderOpenOutlined />} onClick={() => void appOpenLogDir().catch((e) => void message.error(String(e)))}>
            {t("settings.openLogDir")}
          </Button>
        </Form.Item>
      </Form>
    </Card>
  );
}
