export type ResetCardOperation = {
  status: "ready" | "pending" | "damaged";
  confirmation: string;
};

export function parseResetCardOperation(value: unknown): ResetCardOperation | null {
  if (value === null || typeof value !== "object") return null;
  const { status, confirmation } = value as Record<string, unknown>;
  if (status !== "ready" && status !== "pending" && status !== "damaged") return null;
  if (typeof confirmation !== "string" || confirmation.length !== 64 || /[^0-9a-f]/.test(confirmation)) return null;
  return { status, confirmation };
}

export function resetCardAction(
  shown: ResetCardOperation | null,
  current: ResetCardOperation,
  action: "consume" | "abandon",
): "start" | "retry" | "abandon" | null {
  if (shown?.confirmation !== current.confirmation || shown.status !== current.status) return null;
  if (action === "abandon") return current.status === "ready" ? null : "abandon";
  if (current.status === "pending") return "retry";
  return current.status === "ready" ? "start" : null;
}
