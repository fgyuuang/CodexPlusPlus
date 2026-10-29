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

function isRestart(node: AstNode): boolean {
  if (node.type !== "CallExpression") return false;
  const callee = node.callee as AstNode;
  return callee?.type === "MemberExpression"
    && (callee.object as AstNode)?.name === "actions"
    && (callee.property as AstNode)?.name === "restart";
}

test("Manager 重启操作属于实际命令菜单，顶部页面操作挂载点保留", async () => {
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
  assert.ok(collect(topbars[0], (node) => node.type === "JSXAttribute"
    && (node.name as AstNode)?.name === "data-topbar-actions").length === 1);
  const actions = collect(tree, (node) => node.type === "ObjectExpression"
    && (node.properties as AstNode[]).some((property) =>
      (property.key as AstNode)?.name === "id" && (property.value as AstNode)?.value === "action:codex"));
  assert.equal(actions.length, 1);
  assert.equal(collect(actions[0], isRestart).length, 1);
  const palettes = collect(tree, (node) => node.type === "JSXOpeningElement"
    && (node.name as AstNode)?.name === "CommandPalette");
  assert.equal(palettes.length, 1);
  assert.ok((palettes[0].attributes as AstNode[]).some((attribute) =>
    (attribute.name as AstNode)?.name === "commands"
    && ((attribute.value as AstNode)?.expression as AstNode)?.name === "paletteCommands"));
});
