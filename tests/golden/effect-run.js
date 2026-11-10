import { ok, err, structuralEqual } from "./runtime.js";
function $print(v) {
  if (v === null) return "nil";
  if (typeof v === "string") return v;
  if (typeof v === "number") return $print_num(v);
  if (typeof v === "boolean") return v ? "true" : "false";
  if (Array.isArray(v)) return "[" + v.map($print).join(" ") + "]";
  if (v instanceof Uint8Array) return "#u8[" + [...v].join(" ") + "]";
  if (typeof v === "object") {
    if (v.tag === "Ok" && Object.keys(v).length === 2 && "value" in v) return "(ok " + $print(v.value) + ")";
    if (v.tag === "Err" && Object.keys(v).length === 2 && "value" in v) return "(err " + $print(v.value) + ")";
    const ks = Object.keys(v).sort($cmp_str);
    return "{" + ks.map((k) => k + ": " + $print(v[k])).join(", ") + "}";
  }
  return "<fn>";
}
function $print_num(n) {
  if (Number.isNaN(n)) return "NaN";
  if (n === Infinity) return "Infinity";
  if (n === -Infinity) return "-Infinity";
  if (Object.is(n, -0)) return "-0";
  const s = String(n);
  if (s.indexOf("e") < 0 && s.indexOf("E") < 0) return s;
  const m = /^(-?)(\d+)(?:\.(\d+))?[eE]([+-]?\d+)$/.exec(s);
  if (!m) return s;
  const sign = m[1];
  const digits = m[2] + (m[3] || "");
  const point = m[2].length + parseInt(m[4], 10);
  if (point <= 0) return sign + "0." + "0".repeat(-point) + digits;
  if (point >= digits.length) return sign + digits + "0".repeat(point - digits.length);
  return sign + digits.slice(0, point) + "." + digits.slice(point);
}
function $cmp_str(a, b) {
  const pa = [...a];
  const pb = [...b];
  const n = Math.min(pa.length, pb.length);
  for (let i = 0; i < n; i++) {
    const d = pa[i].codePointAt(0) - pb[i].codePointAt(0);
    if (d !== 0) return d;
  }
  return pa.length - pb.length;
}
const $stdlib_add = (a, b) => a + b;
const $stdlib_sub = (a, b) => a - b;
const $stdlib_eq = (a, b) => structuralEqual(a, b);
const $stdlib_str = (a) => $print(a);
const $stdlib_str_concat = (...a) => a.join("");

export async function run(host, x) {
  const $r1 = (await host.log((await $stdlib_str_concat("run:", (await $stdlib_str(x))))));
  if ($r1 !== null && $r1.tag === "Err") return $r1;
  const $r2 = (await host.db("get", (await $stdlib_str(x))));
  if ($r2 !== null && $r2.tag === "Err") return $r2;
  const v = ($r2 !== null && $r2.tag === "Ok") ? $r2.value : $r2;
  const $s3 = v;
  if ($s3 === null) {
    return (await ok(x));

  }
  else {
    const s = $s3;
    return (await ok((await $stdlib_add(x, 1))));

  }
}
export async function count_down(host, n) {
  while (true) {
    if ((await $stdlib_eq(n, 0))) {
      return (await ok(0));

    } else {
      const $r4 = (await host.log((await $stdlib_str(n))));
      if ($r4 !== null && $r4.tag === "Err") return $r4;
      const $a5 = host;
      const $a6 = (await $stdlib_sub(n, 1));
      host = $a5;
      n = $a6;
      continue;

    }
  }
}
{
  const actual = (await run({ log: ((s) => {
    return null;
  }), db: ((op, key) => {
    return null;
  }) }, 1));
  const expected = (await ok(1));
  if (!structuralEqual(actual, expected)) throw new Error("example 0 failed: " + JSON.stringify(actual));
  console.log("example 0: ok " + JSON.stringify(actual));
}
{
  const actual = (await run({ log: ((s) => {
    return null;
  }), db: ((op, key) => {
    return "v";
  }) }, 1));
  const expected = (await ok(2));
  if (!structuralEqual(actual, expected)) throw new Error("example 1 failed: " + JSON.stringify(actual));
  console.log("example 1: ok " + JSON.stringify(actual));
}
{
  const actual = (await count_down({ log: ((s) => {
    return null;
  }), db: ((op, key) => {
    return null;
  }) }, 100000));
  const expected = (await ok(0));
  if (!structuralEqual(actual, expected)) throw new Error("example 2 failed: " + JSON.stringify(actual));
  console.log("example 2: ok " + JSON.stringify(actual));
}

//# sourceMappingURL=effect-run.js.map
