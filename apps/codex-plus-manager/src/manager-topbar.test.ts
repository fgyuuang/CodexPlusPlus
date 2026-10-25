import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";
import { parse } from "@babel/parser";

type AstNode = { type?: string; [key: string]: unknown };

function collect(value: unknown, accept: (node: AstNode) => boolean): AstNode[] {
  if (!value || typeof value !== "object") return [];
  if (Array.isArray(value)) return value.flatMap((item) => collect(item, accept));
  const node = value as AstNode;
  return [
    ...(accept(node) ? [node] : []),
    ...Object.values(node).flatMap((item) => collect(item, accept)),
  ];
}

function isTopbarRestart(node: AstNode): boolean {
  if (node.type !== "CallExpression") return false;
  const callee = node.callee as AstNode;
  const object = callee?.object as AstNode;
  const property = callee?.property as AstNode;
  const args = node.arguments as AstNode[];
  return callee?.type === "MemberExpression" && object?.name === "actions"
    && property?.name === "restart" && args[0]?.value === "topbar";
}

test("Manager 顶部重启控件必须位于实际渲染树中", async () => {
  const source = await readFile(new URL("./App.tsx", import.meta.url), "utf8");
  const tree = parse(source, { sourceType: "module", plugins: ["typescript", "jsx"] });
  const topbars = collect(tree, (node) => {
    if (node.type !== "JSXElement") return false;
    const opening = node.openingElement as AstNode;
    return (opening.attributes as AstNode[]).some((attr) =>
      (attr.name as AstNode)?.name === "className"
      && (attr.value as AstNode)?.value === "topbar-actions");
  });
  assert.equal(topbars.length, 1);
  assert.equal(collect(topbars[0], isTopbarRestart).length, 1,
    "重启调用必须属于顶部工具栏，不能成为异步操作中的未使用 JSX");
  assert.equal(collect(tree, isTopbarRestart).length, 1);
});
