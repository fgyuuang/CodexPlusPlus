export function summarizeWeixinFailures<T extends { peerId: string; state: string }>(messages: readonly T[]) {
  const failedMessages: T[] = [];
  const failedPeers = new Set<string>();
  let blockedCount = 0;
  // Inbox preserves the worker's queue order. Only earlier failures for this peer block an item.
  for (const message of messages) {
    if (message.state === "failed") {
      failedMessages.push(message);
      failedPeers.add(message.peerId);
    } else if (message.state === "pending" && failedPeers.has(message.peerId)) {
      blockedCount++;
    }
  }
  return { failedMessages, blockedCount };
}
