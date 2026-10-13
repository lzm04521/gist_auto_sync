import { useMemo } from "react";
import { diffLines } from "diff";
import { Empty } from "antd";
import { useTranslation } from "react-i18next";

interface Props {
  local: string;
  remote: string;
}

interface RenderedLine {
  text: string;
  kind: "same" | "removed" | "added";
}

/** 简单双栏行级 diff（V0.1）：左=本地（含删除行高亮），右=远端（含新增行高亮） */
export default function DiffView({ local, remote }: Props) {
  const { t } = useTranslation();
  const { left, right } = useMemo(() => {
    const parts = diffLines(local, remote);
    const left: RenderedLine[] = [];
    const right: RenderedLine[] = [];
    for (const part of parts) {
      const lines = part.value.replace(/\n$/, "").split("\n");
      for (const text of lines) {
        if (part.added) {
          right.push({ text, kind: "added" });
        } else if (part.removed) {
          left.push({ text, kind: "removed" });
        } else {
          left.push({ text, kind: "same" });
          right.push({ text, kind: "same" });
        }
      }
    }
    return { left, right };
  }, [local, remote]);

  if (local === remote) {
    return <Empty description={t("common.empty")} />;
  }

  return (
    <div style={{ display: "flex", gap: 8 }}>
      <DiffColumn lines={left} />
      <DiffColumn lines={right} />
    </div>
  );
}

function DiffColumn({ lines }: { lines: RenderedLine[] }) {
  return (
    <pre
      style={{
        flex: 1,
        margin: 0,
        padding: 8,
        background: "#fafafa",
        border: "1px solid #f0f0f0",
        borderRadius: 4,
        overflow: "auto",
        maxHeight: 360,
        fontSize: 12,
        lineHeight: "20px",
      }}
    >
      {lines.map((line, i) => (
        <div
          key={i}
          data-diff={line.kind}
          style={{
            background:
              line.kind === "removed"
                ? "#fff1f0"
                : line.kind === "added"
                  ? "#f6ffed"
                  : undefined,
          }}
        >
          {line.text || "\u00A0"}
        </div>
      ))}
    </pre>
  );
}
