import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { describe, it } from "node:test";
import { runInNewContext } from "node:vm";
import { parse } from "@babel/parser";
import { transformSync } from "esbuild";

const source = readFileSync(new URL("./App.tsx", import.meta.url), "utf8");
const ast = parse(source, { sourceType: "module", plugins: ["typescript", "jsx"] });
type AstNode = Record<string, unknown>;
const expressions: string[] = [];

function visit(value: unknown, parent: AstNode | null = null): void {
  if (!value || typeof value !== "object") return;
  if (Array.isArray(value)) {
    for (const child of value) visit(child, parent);
    return;
  }
  const node = value as AstNode;
  const opening = node.openingElement as { name?: { type?: string; name?: string } } | undefined;
  if (node.type === "JSXElement" && opening?.name?.type === "JSXIdentifier"
      && opening.name.name === "Button" && typeof node.start === "number" && typeof node.end === "number"
      && source.slice(node.start, node.end).includes("actions.switchOfficialAccount(account.id)")) {
    const expression = parent?.type === "ConditionalExpression" ? parent : node;
    assert.equal(typeof expression.start, "number");
    assert.equal(typeof expression.end, "number");
    expressions.push(source.slice(expression.start as number, expression.end as number));
  }
  for (const child of Object.values(node)) visit(child, node);
}
visit(ast);
assert.equal(expressions.length, 1, "use the actual account-row action, including any visibility condition");
const compiled = transformSync(`module.exports = (${expressions[0]});`, {
  loader: "tsx", jsxFactory: "jsx", jsxFragment: "fragment", target: "esnext",
}).code;

type Action = { props: { disabled: boolean; onClick: () => void }; children: string[] };
function render(active: boolean, busy = false, enabled = true) {
  const calls: string[] = [];
  const module = { exports: null as Action | null };
  runInNewContext(compiled, {
    module, active, busy, account: { id: "migrated-account", enabled }, Button: "button",
    t: (text: string) => text,
    actions: { switchOfficialAccount: (id: string) => { calls.push(id); } },
    jsx: (_type: unknown, props: Action["props"], ...children: string[]) => ({ props, children }),
  });
  return { action: module.exports, calls };
}

describe("official account explicit login action", () => {
  it("lets an active migrated account restore login only after a click", () => {
    const { action, calls } = render(true);
    assert.ok(action);
    assert.equal(action.children.join(""), "恢复登录");
    assert.equal(action.props.disabled, false);
    assert.deepEqual(calls, []);
    action.props.onClick();
    assert.deepEqual(calls, ["migrated-account"]);
  });

  it("keeps the existing explicit switch action for an inactive account", () => {
    const { action, calls } = render(false);
    assert.ok(action);
    assert.equal(action.children.join(""), "切换");
    assert.deepEqual(calls, []);
    action.props.onClick();
    assert.deepEqual(calls, ["migrated-account"]);
  });

  it("preserves busy and disabled-account safeguards for both actions", () => {
    for (const active of [false, true]) {
      for (const [busy, enabled] of [[true, true], [false, false], [true, false]]) {
        const { action, calls } = render(active, busy, enabled);
        assert.ok(action);
        assert.equal(action.props.disabled, true);
        assert.deepEqual(calls, []);
      }
    }
  });
});
