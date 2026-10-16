import { Button, Form, Input, Segmented, Switch } from "antd";
import { FolderOpenOutlined } from "@ant-design/icons";
import { open as openFileDialog } from "@tauri-apps/plugin-dialog";
import { useTranslation } from "react-i18next";
import type { EventHook } from "../types";

export type HookField = "preUpload" | "postUpload" | "preDownload" | "postDownload";

/** 表单里单个钩子的缺省形状（新建/未配置时），与后端 serde 缺省一致（两开关默认开） */
export function emptyEventHook(): EventHook {
  return {
    mode: "file",
    scriptPath: "",
    scriptContent: "",
    inlineKind: "powershell",
    onlyWhenChanged: true,
    blockOnFailure: true,
  };
}

interface Props {
  field: HookField;
  /** 前置事件（上传前/下载前）才渲染"失败时阻止本次同步"开关 */
  isPre: boolean;
}

/** 映射表单内单个事件钩子的配置节（四个钩子复用同一组件） */
export default function EventHookSection({ field, isPre }: Props) {
  const { t } = useTranslation();
  const form = Form.useFormInstance();
  const mode = Form.useWatch(["hooks", field, "mode"], form);

  const chooseScript = async () => {
    try {
      const path = await openFileDialog({
        multiple: false,
        directory: false,
        filters: [
          { name: t("mappings.hooks.scriptFilterName"), extensions: ["ps1", "bat", "cmd"] },
        ],
      });
      if (typeof path === "string" && path) {
        form.setFieldValue(["hooks", field, "scriptPath"], path);
      }
    } catch {
      // 对话框取消/失败不打扰表单
    }
  };

  return (
    <div style={{ border: "1px solid #f0f0f0", borderRadius: 6, padding: "12px 12px 4px", marginBottom: 12 }}>
      <strong>{t(`mappings.hooks.${field}`)}</strong>
      <Form.Item name={["hooks", field, "mode"]} label={t("mappings.hooks.mode")} style={{ marginTop: 8 }}>
        <Segmented
          options={[
            { value: "file", label: t("mappings.hooks.modeFile") },
            { value: "inline", label: t("mappings.hooks.modeInline") },
          ]}
        />
      </Form.Item>
      {mode === "inline" ? (
        <>
          <Form.Item name={["hooks", field, "inlineKind"]} label={t("mappings.hooks.inlineKind")}>
            <Segmented
              options={[
                { value: "powershell", label: t("mappings.hooks.powershell") },
                { value: "bat", label: t("mappings.hooks.bat") },
              ]}
            />
          </Form.Item>
          <Form.Item
            name={["hooks", field, "scriptContent"]}
            label={t("mappings.hooks.scriptContent")}
            extra={t("mappings.hooks.inlineBatHint")}
          >
            <Input.TextArea style={{ fontFamily: "monospace" }} autoSize={{ minRows: 8 }} />
          </Form.Item>
        </>
      ) : (
        <Form.Item name={["hooks", field, "scriptPath"]} label={t("mappings.hooks.scriptPath")}>
          <Input
            addonAfter={
              <Button
                type="text"
                size="small"
                icon={<FolderOpenOutlined />}
                onClick={() => void chooseScript()}
              >
                {t("mappings.hooks.browse")}
              </Button>
            }
          />
        </Form.Item>
      )}
      <Form.Item
        name={["hooks", field, "onlyWhenChanged"]}
        valuePropName="checked"
        label={t("mappings.hooks.onlyWhenChanged")}
        extra={t("mappings.hooks.onlyChangedHint")}
      >
        <Switch />
      </Form.Item>
      {isPre && (
        <Form.Item
          name={["hooks", field, "blockOnFailure"]}
          valuePropName="checked"
          label={t("mappings.hooks.blockOnFailure")}
          extra={t("mappings.hooks.blockOnFailureHint")}
        >
          <Switch />
        </Form.Item>
      )}
    </div>
  );
}
