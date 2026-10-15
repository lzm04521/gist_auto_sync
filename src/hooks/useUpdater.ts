import { useCallback, useRef, useState } from "react";
import { check as pluginCheck, type Update } from "@tauri-apps/plugin-updater";
import { relaunch as pluginRelaunch } from "@tauri-apps/plugin-process";

export type UpdaterStatus =
  | "idle"
  | "checking"
  | "available"
  | "upToDate"
  | "failed"
  | "downloading"
  | "downloaded";

/**
 * 应用更新状态机：检查 → 有更新 → 下载安装 → 重启。
 * 错误不吞：失败置 failed 并记录 error，由调用方决定提示方式
 * （关于页手动检查展示 message.error，启动自动检查静默）。
 */
export function useUpdater() {
  const [status, setStatus] = useState<UpdaterStatus>("idle");
  const [version, setVersion] = useState<string | null>(null);
  const [notes, setNotes] = useState<string | null>(null);
  const [progress, setProgress] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);
  const updateRef = useRef<Update | null>(null);

  const check = useCallback(async () => {
    setStatus("checking");
    setError(null);
    try {
      const update = await pluginCheck();
      if (update) {
        updateRef.current = update;
        setVersion(update.version);
        setNotes(update.notes ?? null);
        setStatus("available");
      } else {
        setStatus("upToDate");
      }
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
      setStatus("failed");
    }
  }, []);

  const downloadAndInstall = useCallback(async (): Promise<boolean> => {
    const update = updateRef.current;
    if (!update) return false;
    setStatus("downloading");
    setError(null);
    // 插件事件只给字节量，百分比由累计 chunkLength / contentLength 得出
    let contentLength = 0;
    let received = 0;
    try {
      await update.downloadAndInstall({
        onEvent: ({ event, data }) => {
          const len = data as { contentLength?: number; chunkLength?: number };
          if (event === "Started" && typeof len.contentLength === "number") {
            contentLength = len.contentLength;
          } else if (event === "Progress" && typeof len.chunkLength === "number") {
            received += len.chunkLength;
            if (contentLength > 0) {
              setProgress(Math.min(100, Math.round((received / contentLength) * 100)));
            }
          }
        },
      });
      setProgress(100);
      setStatus("downloaded");
      return true;
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
      setStatus("failed");
      return false;
    }
  }, []);

  const relaunch = useCallback(() => {
    void pluginRelaunch();
  }, []);

  /** 回到初始态（关闭更新弹窗用），清空已缓存的更新对象 */
  const reset = useCallback(() => {
    updateRef.current = null;
    setStatus("idle");
    setVersion(null);
    setNotes(null);
    setProgress(null);
    setError(null);
  }, []);

  return { status, version, notes, progress, error, check, downloadAndInstall, relaunch, reset };
}
