import { useCallback, useEffect, useRef, useState } from "react";
import { check as pluginCheck, type Update } from "@tauri-apps/plugin-updater";
import { relaunch as pluginRelaunch } from "@tauri-apps/plugin-process";
import { settingsGet } from "../api/ipc";

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
        setNotes(update.body ?? null);
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
      // 插件签名：downloadAndInstall(onEvent, options)，回调为第一个位置参数
      await update.downloadAndInstall((evt) => {
        if (evt.event === "Started" && typeof evt.data.contentLength === "number") {
          contentLength = evt.data.contentLength;
        } else if (evt.event === "Progress" && typeof evt.data.chunkLength === "number") {
          received += evt.data.chunkLength;
          if (contentLength > 0) {
            setProgress(Math.min(100, Math.round((received / contentLength) * 100)));
          }
        }
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

/**
 * 启动后延迟自动检查一次新版本（默认 10 秒，受设置 autoCheckUpdate 控制）。
 * 不经 useUpdater 状态机：自动路径失败一律静默（console.debug 留痕，不打扰用户），
 * 发现新版本仅回调 onAvailable，由调用方决定提示方式。无轮询，每次启动最多一次。
 */
export function useStartupAutoCheck(onAvailable: (version: string) => void) {
  const cbRef = useRef(onAvailable);
  cbRef.current = onAvailable;

  useEffect(() => {
    let timer: number | undefined;
    let cancelled = false;
    settingsGet()
      .then((s) => {
        if (cancelled || !s.autoCheckUpdate) return;
        timer = window.setTimeout(() => {
          pluginCheck()
            .then((update) => {
              if (update) cbRef.current(update.version);
            })
            .catch((e) => console.debug("启动自动检查更新失败：", e));
        }, 10_000);
      })
      .catch((e) => console.debug("读取设置失败，跳过启动自动检查：", e));
    return () => {
      cancelled = true;
      if (timer !== undefined) window.clearTimeout(timer);
    };
  }, []);
}
