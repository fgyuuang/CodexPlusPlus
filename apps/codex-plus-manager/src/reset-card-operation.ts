/** 持久保存未确认的操作标识；后端另按官方账号落盘，不能靠重建页面开启另一次消费。 */
type Storage = Pick<globalThis.Storage, "getItem" | "setItem" | "removeItem">;
const key = (providerId: string) => `codexpp:reset-card-operation:${encodeURIComponent(providerId)}`;

export function pendingResetCardOperation(storage: Storage, providerId: string): string | null {
  const id = storage.getItem(key(providerId));
  if (id !== null && (!id.trim() || id !== id.trim() || id.length > 128 || /[\x00-\x1f\x7f]/.test(id))) {
    throw new Error("重置操作记录无效，请先核实上次结果");
  }
  return id;
}

export function beginResetCardOperation(storage: Storage, providerId: string, createId: () => string): string {
  const pending = pendingResetCardOperation(storage, providerId);
  if (pending) return pending;
  const id = createId();
  storage.setItem(key(providerId), id);
  if (pendingResetCardOperation(storage, providerId) !== id) throw new Error("无法保存重置操作，请稍后重试");
  return id;
}

export function finishResetCardOperation(storage: Storage, providerId: string, id: string): void {
  if (pendingResetCardOperation(storage, providerId) === id) storage.removeItem(key(providerId));
}
