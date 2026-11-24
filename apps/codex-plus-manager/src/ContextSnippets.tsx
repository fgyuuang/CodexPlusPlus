import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { ChevronDown, ChevronRight, Eye, EyeOff } from "lucide-react";
import { t } from "./i18n";

type Snippet = { role: "user" | "assistant"; text: string; truncated: boolean };
type Preview = { active: boolean; snippets: Snippet[] };

export function ContextSnippets({ agent, session, requestId }: { agent: string; session: string; requestId: string }) {
  const [status, setStatus] = useState<Preview>();
  const [reveal, setReveal] = useState<string>();
  const [shown, setShown] = useState<{ id: string; snippets: Snippet[] }>();
  const [confirm, setConfirm] = useState(false);
  const [busy, setBusy] = useState(false);
  const [unavailable, setUnavailable] = useState(false);
  const expanded = reveal === requestId && status?.active;

  useEffect(() => {
    if (busy) return;
    let done = false;
    let timer: number;
    const read = async () => {
      try {
        const result = await invoke<Preview>("request_context_preview", {
          request: { action: "read", agent, session, requestId: expanded ? requestId : null },
        });
        if (done) return;
        setStatus(result);
        setShown(expanded && result.active ? { id: requestId, snippets: result.snippets } : undefined);
        setUnavailable(false);
        if (result.active) timer = window.setTimeout(() => void read(), 2500);
        else setReveal(undefined);
      } catch {
        if (done) return;
        setShown(undefined);
        setStatus(undefined);
        setUnavailable(true);
      }
    };
    void read();
    return () => { done = true; window.clearTimeout(timer); };
  }, [agent, session, requestId, expanded, busy]);

  async function change(action: "enable" | "stop") {
    setBusy(true);
    setReveal(undefined);
    setShown(undefined);
    try {
      setStatus(await invoke<Preview>("request_context_preview", { request: { action, agent, session, requestId: null } }));
      setConfirm(false);
      setUnavailable(false);
    } catch {
      setUnavailable(true);
    } finally {
      setBusy(false);
    }
  }

  return <div className="route-context-snippets">
    <div className="route-context-title-row">
      <button className="route-snippet-trigger" type="button" aria-expanded={Boolean(expanded || confirm)} disabled={busy} onClick={() => {
        if (status?.active) {
          setShown(undefined);
          setReveal(expanded ? undefined : requestId);
        } else setConfirm(!confirm);
      }}>
        {expanded || confirm ? <ChevronDown /> : <ChevronRight />}{t("原文片段")}
      </button>
      <small>{t(status?.active ? "本会话 · 仅内存" : "默认不采集")}</small>
      {status?.active && <button className="hub-routing-more" type="button" disabled={busy} onClick={() => void change("stop")}><EyeOff />{t("停止并清除")}</button>}
    </div>
    {confirm && !status?.active && <div className="route-snippet-consent">
      <small>{t("仅暂存本会话新请求里的用户和助手文本，每条请求最多 8 段。最多保留 10 条请求、10 分钟，不写入日志。片段可能包含敏感信息。")}</small>
      <button className="hub-routing-more" type="button" disabled={busy} onClick={() => void change("enable")}><Eye />{t("开启片段暂存")}</button>
    </div>}
    {unavailable && <small>{t("本地网关的片段预览当前不可用")}</small>}
    {expanded && <div className="route-snippet-list">
      {shown?.id !== requestId ? <small>{t("正在读取片段")}</small> : shown.snippets.length === 0
        ? <small>{t("本次请求没有可用片段；开启前的请求不会补录。")}</small>
        : shown.snippets.map(({ role, text, truncated }, index) => <details key={index} open={index === shown.snippets.length - 1 || undefined}>
          <summary>{t(role === "user" ? "用户消息" : "助手消息")} <small>{truncated ? t("已截断") : ""}</small></summary>
          <pre>{text}{truncated && "…"}</pre>
        </details>)}
    </div>}
  </div>;
}
