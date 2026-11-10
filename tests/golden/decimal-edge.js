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
const $stdlib_mul = (a, b) => typeof a === "bigint" ? $dec_mul(a, b) : a * b;
const $stdlib_add = (a, b) => typeof a === "bigint" ? $dec_add(a, b) : a + b;
const $stdlib_sub = (a, b) => typeof a === "bigint" ? $dec_sub(a, b) : a - b;
const $stdlib_div = (a, b) => typeof a === "bigint" ? $dec_div(a, b) : a / b;
const $stdlib_lt = (a, b) => a < b;
const $stdlib_le = (a, b) => a <= b;
const $stdlib_eq = (a, b) => structuralEqual(a, b);
const $stdlib_gt = (a, b) => a > b;
const $stdlib_str = (a) => typeof a === "bigint" ? $print_dec(a) : $print(a);
const $stdlib_to_dec = (a) => $to_dec(a);
const $stdlib_to_num = (a) => Number(a) / 1e18;
const $DEC_S = 10n ** 18n;
const $DEC_MAX = 170141183460469231731687303715884105727n;
function $dec_check(r) { if (r > $DEC_MAX || r < -$DEC_MAX) throw new Error("decimal-overflow"); return r; }
const $dec_add = (a, b) => $dec_check(a + b);
const $dec_sub = (a, b) => $dec_check(a - b);
const $dec_mul = (a, b) => { const p = a * b; const neg = p < 0n; const ap = neg ? -p : p; let q = ap / $DEC_S; const r = ap % $DEC_S; if (2n * r >= $DEC_S) q += 1n; return $dec_check(neg ? -q : q); };
const $dec_div = (a, b) => { if (b === 0n) throw new Error("decimal-division-by-zero"); const neg = (a < 0n) !== (b < 0n); const num = (a < 0n ? -a : a) * $DEC_S; const den = b < 0n ? -b : b; let q = num / den; const r = num % den; if (2n * r >= den) q += 1n; return $dec_check(neg ? -q : q); };
function $print_dec(v) { const neg = v < 0n; const a = neg ? -v : v; const i = a / $DEC_S; const f = a % $DEC_S; return (neg ? "-" : "") + i.toString() + "." + f.toString().padStart(18, "0"); }
const $to_dec = (a) => { if (typeof a !== "number" || Number.isNaN(a) || !Number.isFinite(a)) throw new Error("decimal-overflow"); const m = /^([+-]?)(\d+)(?:\.(\d+))?e([+-]?\d+)$/.exec(a.toExponential()); if (!m) throw new Error("decimal-overflow"); const sign = m[1] === "-" ? -1n : 1n; const digits = m[2] + (m[3] || ""); const exp = parseInt(m[4], 10) - (m[3] || "").length + 18; const mant = BigInt(digits); let r; if (exp >= 0) { r = mant * 10n ** BigInt(exp); } else { const den = 10n ** BigInt(-exp); let q = mant / den; const rem = mant % den; if (2n * rem >= den) q += 1n; r = q; } return $dec_check(sign * r); };
function $json(v) { if (typeof v === "bigint") return JSON.stringify({ tag: "Dec", value: $print_dec(v) }); if (Array.isArray(v)) return "[" + v.map($json).join(",") + "]"; if (v !== null && typeof v === "object") { const ks = Object.keys(v).sort(); return "{" + ks.map((k) => JSON.stringify(k) + ":" + $json(v[k])).join(",") + "}"; } return JSON.stringify(v); }

function add_price(a, b) {
  return { amount: $stdlib_add((a).amount, (b).amount), currency: (a).currency };
}
{
  const actual = 1500000000000000000n;
  const expected = 1500000000000000000n;
  if (!structuralEqual(actual, expected)) throw new Error("example 0 failed: " + $json(actual));
  console.log("example 0: ok " + $json(actual));
}
{
  const actual = $stdlib_add(1500000000000000000n, 2250000000000000000n);
  const expected = 3750000000000000000n;
  if (!structuralEqual(actual, expected)) throw new Error("example 1 failed: " + $json(actual));
  console.log("example 1: ok " + $json(actual));
}
{
  const actual = $stdlib_sub(1000000000000000000n, 2500000000000000000n);
  const expected = -1500000000000000000n;
  if (!structuralEqual(actual, expected)) throw new Error("example 2 failed: " + $json(actual));
  console.log("example 2: ok " + $json(actual));
}
{
  const actual = $stdlib_mul(1500000000000000000n, 2000000000000000000n);
  const expected = 3000000000000000000n;
  if (!structuralEqual(actual, expected)) throw new Error("example 3 failed: " + $json(actual));
  console.log("example 3: ok " + $json(actual));
}
{
  const actual = $stdlib_mul(100000000000000000n, 100000000000000000n);
  const expected = 10000000000000000n;
  if (!structuralEqual(actual, expected)) throw new Error("example 4 failed: " + $json(actual));
  console.log("example 4: ok " + $json(actual));
}
{
  const actual = $stdlib_div(1000000000000000000n, 3000000000000000000n);
  const expected = 333333333333333333n;
  if (!structuralEqual(actual, expected)) throw new Error("example 5 failed: " + $json(actual));
  console.log("example 5: ok " + $json(actual));
}
{
  const actual = $stdlib_div(2000000000000000000n, 3000000000000000000n);
  const expected = 666666666666666667n;
  if (!structuralEqual(actual, expected)) throw new Error("example 6 failed: " + $json(actual));
  console.log("example 6: ok " + $json(actual));
}
{
  const actual = $stdlib_div(-1000000000000000000n, 2000000000000000000n);
  const expected = -500000000000000000n;
  if (!structuralEqual(actual, expected)) throw new Error("example 7 failed: " + $json(actual));
  console.log("example 7: ok " + $json(actual));
}
{
  const actual = $stdlib_lt(1000000000000000000n, 2000000000000000000n);
  const expected = true;
  if (!structuralEqual(actual, expected)) throw new Error("example 8 failed: " + $json(actual));
  console.log("example 8: ok " + $json(actual));
}
{
  const actual = $stdlib_le(2000000000000000000n, 2000000000000000000n);
  const expected = true;
  if (!structuralEqual(actual, expected)) throw new Error("example 9 failed: " + $json(actual));
  console.log("example 9: ok " + $json(actual));
}
{
  const actual = $stdlib_gt(2000000000000000000n, 1000000000000000000n);
  const expected = true;
  if (!structuralEqual(actual, expected)) throw new Error("example 10 failed: " + $json(actual));
  console.log("example 10: ok " + $json(actual));
}
{
  const actual = $stdlib_eq(1500000000000000000n, 1500000000000000000n);
  const expected = true;
  if (!structuralEqual(actual, expected)) throw new Error("example 11 failed: " + $json(actual));
  console.log("example 11: ok " + $json(actual));
}
{
  const actual = $stdlib_eq(1500000000000000000n, 1510000000000000000n);
  const expected = false;
  if (!structuralEqual(actual, expected)) throw new Error("example 12 failed: " + $json(actual));
  console.log("example 12: ok " + $json(actual));
}
{
  const actual = $stdlib_to_dec(1.5);
  const expected = 1500000000000000000n;
  if (!structuralEqual(actual, expected)) throw new Error("example 13 failed: " + $json(actual));
  console.log("example 13: ok " + $json(actual));
}
{
  const actual = $stdlib_to_num(2500000000000000000n);
  const expected = 2.5;
  if (!structuralEqual(actual, expected)) throw new Error("example 14 failed: " + $json(actual));
  console.log("example 14: ok " + $json(actual));
}
{
  const actual = $stdlib_str(1500000000000000000n);
  const expected = "1.500000000000000000";
  if (!structuralEqual(actual, expected)) throw new Error("example 15 failed: " + $json(actual));
  console.log("example 15: ok " + $json(actual));
}
{
  const actual = $stdlib_str(-250000000000000000n);
  const expected = "-0.250000000000000000";
  if (!structuralEqual(actual, expected)) throw new Error("example 16 failed: " + $json(actual));
  console.log("example 16: ok " + $json(actual));
}
{
  const actual = add_price({ amount: 1100000000000000000n, currency: "JPY" }, { amount: 2200000000000000000n, currency: "JPY" });
  const expected = { amount: 3300000000000000000n, currency: "JPY" };
  if (!structuralEqual(actual, expected)) throw new Error("example 17 failed: " + $json(actual));
  console.log("example 17: ok " + $json(actual));
}

//# sourceMappingURL=decimal-edge.js.map
