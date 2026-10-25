type Window = { usedPercent?: number | null; windowMinutes?: number | null; resetsAt?: number | null };
type Usage = {
  fetchedAt?: number; primary?: Window | null; secondary?: Window | null;
  additionalRateLimits?: unknown[]; credits?: unknown; error?: string | null;
};
export type UsageRow = { label: string; used: number | null; remaining: number | null; resetsAt: number | null };
const object = (value: unknown): Record<string, unknown> => value && typeof value === "object" ? value as Record<string, unknown> : {};
const number = (value: unknown): number | null => typeof value === "number" && Number.isFinite(value) ? value : null;

function windowRow(window: Window, name: string): UsageRow {
  const minutes = number(window.windowMinutes);
  const duration = minutes && minutes > 0
    ? minutes % 1440 === 0 ? `${minutes / 1440}天` : minutes % 60 === 0 ? `${minutes / 60}小时` : `${minutes}分钟`
    : name;
  const percent = number(window.usedPercent);
  const used = percent == null ? null : Math.min(100, Math.max(0, percent));
  return { label: `${duration}额度`, used, remaining: used == null ? null : Math.round((100 - used) * 10) / 10, resetsAt: number(window.resetsAt) };
}

export function officialUsageRows(usage: Usage | null): UsageRow[] {
  if (!usage) return [];
  const rows: UsageRow[] = [];
  if (usage.primary) rows.push(windowRow(usage.primary, "主窗口"));
  if (usage.secondary) rows.push(windowRow(usage.secondary, "次窗口"));
  for (const [index, value] of (usage.additionalRateLimits || []).entries()) {
    const limit = object(value);
    const name = typeof limit.limit_name === "string" ? limit.limit_name : typeof limit.metered_feature === "string" ? limit.metered_feature : `附加限额 ${index + 1}`;
    const rate = object(limit.rate_limit);
    for (const key of ["primary_window", "secondary_window"]) {
      if (!rate[key]) continue;
      const w = object(rate[key]);
      const seconds = number(w.limit_window_seconds);
      const row = windowRow({ usedPercent: number(w.used_percent), windowMinutes: seconds == null ? null : Math.ceil(seconds / 60), resetsAt: number(w.reset_at) }, "窗口");
      rows.push({ ...row, label: `${name} · ${row.label}` });
    }
  }
  return rows;
}

export function officialUsageStatus(usage: Usage | null, now = Date.now() / 1000): string {
  if (!usage) return "用量未查询";
  if (usage.error) return "用量刷新失败，以下为上次查询结果";
  if (!usage.fetchedAt || now - usage.fetchedAt >= 300) return "缓存用量，等待刷新";
  return "用量已更新";
}

export function officialCreditText(credits: unknown): string {
  const value = object(credits);
  if (value.unlimited === true) return "额外积分：不限额（仅积分）";
  if (typeof value.balance === "string" || number(value.balance) != null) return `额外积分：${value.balance}`;
  return value.has_credits === false ? "无额外积分" : "";
}
