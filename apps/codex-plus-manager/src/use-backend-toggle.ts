import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";
import { stringifyError } from "./app-shared";

/// 托盘快捷面板开关。值仍存在 gateway.json 的 `trayPanel`（老版本在网关设置里改过的选择照样有效），
/// 由后端命令读写，后端同时更新托盘行为和内存缓存。
/** 设置页直接读取和更新一个后端开关；读取失败时不允许从未知状态反向切换。 */
export function useBackendToggle(readCommand: string, writeCommand: string) {
  const [enabled, setEnabled] = useState<boolean | null>(null);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");
  useEffect(() => {
    let cancelled = false;
    void invoke<boolean>(readCommand)
      .then((value) => { if (!cancelled) setEnabled(value !== false); })
      .catch((failure) => { if (!cancelled) setError(stringifyError(failure)); });
    return () => { cancelled = true; };
  }, [readCommand]);
  const toggle = async () => {
    if (enabled === null || saving) return;
    setSaving(true);
    setError("");
    try {
      setEnabled(await invoke<boolean>(writeCommand, { enabled: !enabled }));
    } catch (failure) {
      setError(stringifyError(failure));
    } finally {
      setSaving(false);
    }
  };
  return { enabled, saving, error, toggle };
}
