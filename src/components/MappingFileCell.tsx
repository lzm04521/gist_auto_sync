import type { CSSProperties } from "react";
import { CloudOutlined, FolderOpenOutlined } from "@ant-design/icons";
import { Space, Tooltip } from "antd";
import type { Mapping } from "../types";
import { fileNameOf } from "./MappingForm";

/** 显示名回退链：name → gist 文件名 → 本地文件名（与后端 display_name 同规则） */
export function mappingDisplayName(m: Mapping): string {
  return m.name || m.gistFilename || fileNameOf(m.localPath);
}

/** 胶囊块外壳：底色区分本地/远端，等宽字体 */
const pill = (background: string): CSSProperties => ({
  background,
  borderRadius: 6,
  padding: "2px 8px",
  fontFamily: "monospace",
  fontSize: 12,
  display: "flex",
  alignItems: "center",
  gap: 6,
  minWidth: 0,
  maxWidth: "100%",
  boxSizing: "border-box",
});

/** 块内文本：超长单行省略（完整内容走 Tooltip） */
const ellipsis: CSSProperties = {
  overflow: "hidden",
  textOverflow: "ellipsis",
  whiteSpace: "nowrap",
  maxWidth: "100%",
};

/** 映射"本地 ↔ 远端"信息展示：名称主行 + 本地路径块（浅蓝）+ gist 块（浅灰） */
export default function MappingFileCell({ mapping }: { mapping: Mapping }) {
  return (
    <Space direction="vertical" size={4} style={{ minWidth: 0, maxWidth: "100%" }}>
      <strong>{mappingDisplayName(mapping)}</strong>
      <Tooltip title={mapping.localPath}>
        <div style={pill("#e6f4ff")}>
          <FolderOpenOutlined style={{ flexShrink: 0 }} />
          <span style={ellipsis}>{mapping.localPath}</span>
        </div>
      </Tooltip>
      <Tooltip title={mapping.gistId}>
        <div style={pill("#f5f5f5")}>
          <CloudOutlined style={{ flexShrink: 0 }} />
          <span style={ellipsis}>{mapping.gistId.slice(0, 8)}/{mapping.gistFilename}</span>
        </div>
      </Tooltip>
    </Space>
  );
}
