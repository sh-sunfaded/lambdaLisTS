import { ok, err, structuralEqual } from "./runtime.js";
import { runProps } from "./prop-runtime.js";
const $stdlib_add = (a, b) => a + b;
const $stdlib_eq = (a, b) => structuralEqual(a, b);
const $stdlib_str_concat = (...a) => a.join("");

export function zero() {
  return { amount: 0, currency: "JPY" };
}
export function add_money(a, b) {
  if ($stdlib_eq((a).currency, (b).currency)) {
    return ok({ amount: $stdlib_add((a).amount, (b).amount), currency: (a).currency });

  } else {
    return err($stdlib_str_concat("currency mismatch: ", (a).currency, " vs ", (b).currency));

  }
}
{
  const actual = add_money({ amount: 1, currency: "JPY" }, { amount: 2, currency: "JPY" });
  const expected = ok({ amount: 3, currency: "JPY" });
  if (!structuralEqual(actual, expected)) throw new Error("example 0 failed: " + JSON.stringify(actual));
  console.log("example 0: ok " + JSON.stringify(actual));
}
function $prop_0(a, b) {
  return $stdlib_eq(add_money({ amount: a, currency: "JPY" }, { amount: b, currency: "JPY" }), add_money({ amount: b, currency: "JPY" }, { amount: a, currency: "JPY" }));
}
const $SEED = 6840152529647541583n;
const $PROPS = [{ name: "add-money commutative", params: [{ name: "a", ty: {"t":"number"} }, { name: "b", ty: {"t":"number"} }], fn: $prop_0, core: {"App":{"f":{"Var":{"name":"=","span":{"file":"/workspace/github__KennedySilverDollar__lambdaLisTS/tests/fixtures/money.lts","start":624,"end":830,"line":20,"col":5}}},"args":[{"App":{"f":{"Var":{"name":"add-money","span":{"file":"/workspace/github__KennedySilverDollar__lambdaLisTS/tests/fixtures/money.lts","start":627,"end":724,"line":20,"col":8}}},"args":[{"Obj":{"fields":[["amount",{"Var":{"name":"a","span":{"file":"/workspace/github__KennedySilverDollar__lambdaLisTS/tests/fixtures/money.lts","start":651,"end":652,"line":20,"col":32}}}],["currency",{"Lit":{"value":{"Str":"JPY"},"span":{"file":"/workspace/github__KennedySilverDollar__lambdaLisTS/tests/fixtures/money.lts","start":664,"end":669,"line":20,"col":45}}}]],"span":{"file":"/workspace/github__KennedySilverDollar__lambdaLisTS/tests/fixtures/money.lts","start":638,"end":671,"line":20,"col":19}}},{"Obj":{"fields":[["amount",{"Var":{"name":"b","span":{"file":"/workspace/github__KennedySilverDollar__lambdaLisTS/tests/fixtures/money.lts","start":703,"end":704,"line":21,"col":32}}}],["currency",{"Lit":{"value":{"Str":"JPY"},"span":{"file":"/workspace/github__KennedySilverDollar__lambdaLisTS/tests/fixtures/money.lts","start":716,"end":721,"line":21,"col":45}}}]],"span":{"file":"/workspace/github__KennedySilverDollar__lambdaLisTS/tests/fixtures/money.lts","start":690,"end":723,"line":21,"col":19}}}],"span":{"file":"/workspace/github__KennedySilverDollar__lambdaLisTS/tests/fixtures/money.lts","start":627,"end":724,"line":20,"col":8}}},{"App":{"f":{"Var":{"name":"add-money","span":{"file":"/workspace/github__KennedySilverDollar__lambdaLisTS/tests/fixtures/money.lts","start":732,"end":829,"line":22,"col":8}}},"args":[{"Obj":{"fields":[["amount",{"Var":{"name":"b","span":{"file":"/workspace/github__KennedySilverDollar__lambdaLisTS/tests/fixtures/money.lts","start":756,"end":757,"line":22,"col":32}}}],["currency",{"Lit":{"value":{"Str":"JPY"},"span":{"file":"/workspace/github__KennedySilverDollar__lambdaLisTS/tests/fixtures/money.lts","start":769,"end":774,"line":22,"col":45}}}]],"span":{"file":"/workspace/github__KennedySilverDollar__lambdaLisTS/tests/fixtures/money.lts","start":743,"end":776,"line":22,"col":19}}},{"Obj":{"fields":[["amount",{"Var":{"name":"a","span":{"file":"/workspace/github__KennedySilverDollar__lambdaLisTS/tests/fixtures/money.lts","start":808,"end":809,"line":23,"col":32}}}],["currency",{"Lit":{"value":{"Str":"JPY"},"span":{"file":"/workspace/github__KennedySilverDollar__lambdaLisTS/tests/fixtures/money.lts","start":821,"end":826,"line":23,"col":45}}}]],"span":{"file":"/workspace/github__KennedySilverDollar__lambdaLisTS/tests/fixtures/money.lts","start":795,"end":828,"line":23,"col":19}}}],"span":{"file":"/workspace/github__KennedySilverDollar__lambdaLisTS/tests/fixtures/money.lts","start":732,"end":829,"line":22,"col":8}}}],"span":{"file":"/workspace/github__KennedySilverDollar__lambdaLisTS/tests/fixtures/money.lts","start":624,"end":830,"line":20,"col":5}}} }];
await runProps($PROPS, $SEED, 100);

//# sourceMappingURL=money.js.map
