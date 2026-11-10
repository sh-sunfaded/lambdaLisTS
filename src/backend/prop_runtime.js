// prop ランタイム（M22 の interp 実装 src/prop.rs の JS 写像）。
// runtime.js の export 3つ（ok/err/structuralEqual、SPEC §12.5 MUST）は不変のままとし、
// prop 用はこの別モジュールにまとめる（生成される .js から prop があるときだけ import される）。
// splitmix64 は src/rng.rs とビット一致（u64 演算は BigInt）。同じ seed なら同じ列（決定性 MUST）。

const MASK64 = 0xFFFFFFFFFFFFFFFFn;

export function makeRng(seed) {
  return { state: BigInt(seed) & MASK64 };
}

export function nextU64(r) {
  r.state = (r.state + 0x9E3779B97F4A7C15n) & MASK64;
  let z = r.state;
  z = ((z ^ (z >> 30n)) * 0xBF58476D1CE4E5B9n) & MASK64;
  z = ((z ^ (z >> 27n)) * 0x94D049BB133111EBn) & MASK64;
  return (z ^ (z >> 31n)) & MASK64;
}

// [lo, hi] 包含範囲の整数（hi < lo なら lo）— src/rng.rs と同一
export function genRange(r, lo, hi) {
  const l = BigInt(lo);
  const h = BigInt(hi);
  if (h <= l) return l;
  return l + (nextU64(r) % (h - l + 1n));
}

export function genBool(r) {
  return (nextU64(r) & 1n) === 1n;
}

const NUM_BOUNDARIES = [0.0, 1.0, -1.0, 9007199254740992.0, -9007199254740992.0];
const STR_CHARSET = ["a", "b", "0", "1", " ", "あ", "😀"];
const MAX_GEN_DEPTH = 6;
const DEC_S = 1000000000000000000n;
const DEC_MAX = 170141183460469231731687303715884105727n;

export function isGeneratable(ty) {
  switch (ty.t) {
    case "fn":
    case "var":
    case "ref":
      return false;
    case "array":
      return isGeneratable(ty.el);
    case "map":
      return isGeneratable(ty.k) && isGeneratable(ty.v);
    case "set":
      return isGeneratable(ty.k);
    case "union":
      return ty.members.every(isGeneratable);
    case "obj":
      return Object.keys(ty.fields).every((k) => isGeneratable(ty.fields[k]));
    case "result":
      return isGeneratable(ty.ok) && isGeneratable(ty.err);
    default:
      return true;
  }
}

function genValue(ty, rng, depth) {
  if (depth > MAX_GEN_DEPTH) return minValue(ty);
  switch (ty.t) {
    case "nil":
      return null;
    case "bool":
      return genBool(rng);
    case "decimal":
      if (genBool(rng)) {
        const b = [0n, 1n, -1n, DEC_S, -DEC_S, DEC_MAX, -DEC_MAX];
        return b[Number(genRange(rng, 0n, 6n))];
      }
      return genRange(rng, 0n, 2n * DEC_S) - DEC_S;
    case "number":
      if (genBool(rng)) return NUM_BOUNDARIES[Number(genRange(rng, 0n, BigInt(NUM_BOUNDARIES.length - 1)))];
      return Number(genRange(rng, 0n, 200n)) - 100;
    case "string": {
      const len = Number(genRange(rng, 0n, 8n));
      let s = "";
      for (let i = 0; i < len; i++) s += STR_CHARSET[Number(genRange(rng, 0n, BigInt(STR_CHARSET.length - 1)))];
      return s;
    }
    case "char":
      return STR_CHARSET[Number(genRange(rng, 0n, BigInt(STR_CHARSET.length - 1)))];
    case "lit":
      return ty.v;
    case "array": {
      const len = genRange(rng, 0n, 2n) === 0n ? 0 : Number(genRange(rng, 0n, 6n));
      const out = [];
      for (let i = 0; i < len; i++) {
        const g = genValue(ty.el, rng, depth + 1);
        if (g === undefined) return undefined;
        out.push(g);
      }
      return out;
    }
    case "obj": {
      const out = {};
      for (const k of Object.keys(ty.fields)) {
        const g = genValue(ty.fields[k], rng, depth + 1);
        if (g === undefined) return undefined;
        out[k] = g;
      }
      return out;
    }
    case "map":
    case "set": {
      const n = Number(genRange(rng, 0n, 4n));
      const m = ty.t === "map" ? new Map() : new Set();
      for (let i = 0; i < n; i++) {
        const k = genValue(ty.k, rng, depth + 1);
        if (k === undefined) return undefined;
        if (ty.t === "map") {
          const v = genValue(ty.v, rng, depth + 1);
          if (v === undefined) return undefined;
          m.set(k, v);
        } else {
          m.add(k);
        }
      }
      return m;
    }
    case "builder": {
      const n = Number(genRange(rng, 0n, 4n));
      const parts = [];
      for (let i = 0; i < n; i++) {
        const g = genValue({ t: "string" }, rng, depth + 1);
        if (typeof g === "string") parts.push(g);
      }
      return parts;
    }
    case "bytes": {
      const n = Number(genRange(rng, 0n, 5n));
      const b = new Uint8Array(n);
      for (let i = 0; i < n; i++) b[i] = Number(genRange(rng, 0n, 256n)) & 0xff;
      return b;
    }
    case "ref":
      return undefined;
    case "union": {
      const members = ty.members;
      const idx = Number(genRange(rng, 0n, BigInt(Math.max(1, members.length) - 1)));
      return genValue(members[idx], rng, depth + 1);
    }
    case "result": {
      if (genBool(rng)) {
        const g = genValue(ty.ok, rng, depth + 1);
        if (g === undefined) return undefined;
        return { tag: "Ok", value: g };
      }
      const g = genValue(ty.err, rng, depth + 1);
      if (g === undefined) return undefined;
      return { tag: "Err", value: g };
    }
    default:
      return undefined;
  }
}

function minValue(ty) {
  switch (ty.t) {
    case "nil":
      return null;
    case "bool":
      return false;
    case "number":
      return 0;
    case "decimal":
      return 0n;
    case "string":
      return "";
    case "char":
      return "a";
    case "lit":
      return ty.v;
    case "array":
      return [];
    case "map":
      return new Map();
    case "set":
      return new Set();
    case "builder":
      return [];
    case "bytes":
      return new Uint8Array(0);
    case "ref":
      return undefined;
    case "obj": {
      const out = {};
      for (const k of Object.keys(ty.fields)) {
        const g = minValue(ty.fields[k]);
        if (g === undefined) return undefined;
        out[k] = g;
      }
      return out;
    }
    case "union": {
      if (ty.members.some((m) => m.t === "nil")) return null;
      return minValue(ty.members[0]);
    }
    case "result": {
      const g = minValue(ty.err);
      if (g === undefined) return undefined;
      return { tag: "Err", value: g };
    }
    default:
      return undefined;
  }
}

function valueSize(v) {
  if (v === null || typeof v === "boolean") return 0;
  if (typeof v === "bigint") return v === 0n ? 0 : 1 + String(v < 0n ? -v : v).length;
  if (typeof v === "number") return v === 0 ? 0 : 1 + String(Math.abs(v)).length;
  if (typeof v === "string") return [...v].length;
  if (v instanceof Uint8Array) return 1 + v.length;
  if (Array.isArray(v)) return 1 + v.reduce((a, x) => a + valueSize(x), 0);
  if (v instanceof Map) {
    let s = 1;
    for (const [k, x] of v) s += valueSize(k) + valueSize(x);
    return s;
  }
  if (v instanceof Set) {
    let s = 1;
    for (const k of v) s += valueSize(k);
    return s;
  }
  if (typeof v === "object") {
    if (v.__lists_ref__ === true) return 9007199254740991;
    let s = 1;
    for (const k of Object.keys(v)) s += valueSize(v[k]);
    return s;
  }
  return 9007199254740991;
}

function byteLen(s) {
  return new TextEncoder().encode(s).length;
}

function shrinkCandidates(v, ty) {
  const out = [];
  const t = ty.t;
  if (t === "decimal" && typeof v === "bigint") {
    if (v !== 0n) {
      out.push(0n);
      const half = v / 2n;
      if (half !== v) out.push(half);
    }
  } else if (t === "number" && typeof v === "number") {
    if (v !== 0) {
      out.push(0);
      const half = v / 2;
      if (half !== v) out.push(half);
    }
  } else if (t === "string" && typeof v === "string") {
    const chars = [...v];
    if (chars.length > 0) {
      out.push("");
      const half = chars.slice(0, Math.floor(chars.length / 2)).join("");
      if (byteLen(half) < byteLen(v)) out.push(half);
      out.push(chars.slice(0, chars.length - 1).join(""));
    }
  } else if (t === "array" && Array.isArray(v)) {
    if (v.length > 0) {
      out.push([]);
      out.push(v.slice(0, v.length - 1));
      out.push(v.slice(1));
      const half = v.slice(0, Math.floor(v.length / 2));
      if (half.length < v.length) out.push(half);
      for (let i = 0; i < v.length; i++) {
        const cands = shrinkCandidates(v[i], ty.el);
        if (cands.length > 0) {
          const c = cands[0];
          if (valueSize(c) < valueSize(v[i])) {
            const xs2 = v.slice();
            xs2[i] = c;
            out.push(xs2);
          }
        }
      }
    }
  } else if (t === "bool" && v === true) {
    out.push(false);
  } else if (t === "obj" && typeof v === "object" && v !== null && !Array.isArray(v)) {
    for (const k of Object.keys(v)) {
      const ft = ty.fields ? ty.fields[k] : undefined;
      if (ft) {
        for (const c of shrinkCandidates(v[k], ft)) {
          if (valueSize(c) < valueSize(v[k])) {
            const m2 = Object.assign({}, v);
            m2[k] = c;
            out.push(m2);
          }
        }
      }
    }
  } else if (t === "result" && typeof v === "object" && v !== null && (v.tag === "Ok" || v.tag === "Err")) {
    const inner = v.tag === "Ok" ? ty.ok : ty.err;
    for (const c of shrinkCandidates(v.value, inner)) {
      if (valueSize(c) < valueSize(v.value)) out.push({ tag: v.tag, value: c });
    }
  } else if (t === "union") {
    for (const m of ty.members) {
      for (const c of shrinkCandidates(v, m)) {
        if (valueSize(c) < valueSize(v)) out.push(c);
      }
    }
  }
  return out;
}

async function shrinkProp(spec, args) {
  const fails = async (trial) => {
    try {
      return (await spec.fn(...trial)) !== true;
    } catch (e) {
      return true;
    }
  };
  while (true) {
    let improved = false;
    let restart = false;
    for (let i = 0; i < args.length && !restart; i++) {
      for (const cand of shrinkCandidates(args[i], spec.params[i].ty)) {
        if (valueSize(cand) >= valueSize(args[i])) continue;
        const trial = args.slice();
        trial[i] = cand;
        if (await fails(trial)) {
          args = trial;
          improved = true;
          restart = true;
          break;
        }
      }
    }
    if (!improved) break;
  }
  return args;
}

// ---- 印字（print_value / value_to_lit / print_core の写像） ----

function $cmpStr(a, b) {
  const pa = [...a];
  const pb = [...b];
  const n = Math.min(pa.length, pb.length);
  for (let i = 0; i < n; i++) {
    const d = pa[i].codePointAt(0) - pb[i].codePointAt(0);
    if (d !== 0) return d;
  }
  return pa.length - pb.length;
}

function $printNumVal(n) {
  if (Number.isNaN(n)) return "NaN";
  if (n === Infinity) return "inf";
  if (n === -Infinity) return "-inf";
  if (Object.is(n, -0)) return "-0";
  return String(n);
}

function $printDec(v) {
  const neg = v < 0n;
  const a = neg ? -v : v;
  const i = a / DEC_S;
  const f = a % DEC_S;
  return (neg ? "-" : "") + i.toString() + "." + f.toString().padStart(18, "0");
}

function printCharLit(c) {
  let out = "\\";
  const cp = c.codePointAt(0);
  if (c === '"') out += '\\"';
  else if (c === "\\") out += "\\\\";
  else if (c === "\n") out += "\\n";
  else if (c === "\t") out += "\\t";
  else if (c === "\r") out += "\\r";
  else if (cp < 0x20 || cp === 0x7f) out += "\\u" + cp.toString(16).padStart(4, "0");
  else out += c;
  return out;
}

export function $printVal(v) {
  if (v === null) return "nil";
  if (typeof v === "string") return v;
  if (typeof v === "number") return $printNumVal(v);
  if (typeof v === "boolean") return v ? "true" : "false";
  if (typeof v === "bigint") return $printDec(v);
  if (Array.isArray(v)) return "[" + v.map($printVal).join(" ") + "]";
  if (v instanceof Uint8Array) return "#u8[" + [...v].join(" ") + "]";
  if (v instanceof Map) return v.size === 0 ? "(map)" : "(map " + [...v].map(([k, x]) => "(" + $printVal(k) + " " + $printVal(x) + ")").join(" ") + ")";
  if (v instanceof Set) return v.size === 0 ? "(set)" : "(set " + [...v].map($printVal).join(" ") + ")";
  if (typeof v === "object") {
    if (v.__lists_ref__ === true) return "<ref>";
    if (v.tag === "Ok" && Object.keys(v).length === 2 && "value" in v) return "(ok " + $printVal(v.value) + ")";
    if (v.tag === "Err" && Object.keys(v).length === 2 && "value" in v) return "(err " + $printVal(v.value) + ")";
    const ks = Object.keys(v).sort($cmpStr);
    return "{" + ks.map((k) => k + ": " + $printVal(v[k])).join(", ") + "}";
  }
  return "<fn>";
}

function unwrapSerdeVal(v) {
  // codegen 埋め込みの core JSON 内の Lit は serde 形（{"Num":1} 等）なので平坦化する
  if (v !== null && typeof v === "object" && !Array.isArray(v) && !(v instanceof Map) && !(v instanceof Set) && !(v instanceof Uint8Array)) {
    const ks = Object.keys(v);
    if (ks.length === 1) {
      const k = ks[0];
      if (k === "Num" || k === "Dec") return v[k];
      if (k === "Str" || k === "Char") return v[k];
      if (k === "Bool") return v[k];
      if (k === "Nil") return null;
    }
  }
  return v;
}

function valueToLit(v0) {
  const v = unwrapSerdeVal(v0);
  if (typeof v === "string") return JSON.stringify(v);
  if (typeof v === "number") return $printNumVal(v);
  if (typeof v === "boolean") return v ? "true" : "false";
  if (v === null) return "nil";
  if (typeof v === "bigint") return $printDec(v);
  if (Array.isArray(v)) return "(array " + v.map(valueToLit).join(" ") + ")";
  if (v instanceof Uint8Array) return $printVal(v);
  if (v instanceof Map || v instanceof Set) return $printVal(v);
  if (typeof v === "object") {
    if (v.__lists_ref__ === true) return "<ref>";
    if (v.tag === "Ok" && Object.keys(v).length === 2 && "value" in v) return "(ok " + valueToLit(v.value) + ")";
    if (v.tag === "Err" && Object.keys(v).length === 2 && "value" in v) return "(err " + valueToLit(v.value) + ")";
    const ks = Object.keys(v).sort($cmpStr);
    return "(obj " + ks.map((k) => "(" + k + " " + valueToLit(v[k]) + ")").join(" ") + ")";
  }
  return "<fn>";
}

function substCore(e, map) {
  if (e.Var !== undefined && Object.prototype.hasOwnProperty.call(map, e.Var.name)) {
    return { Lit: { value: map[e.Var.name], span: e.Var.span } };
  }
  if (e.App !== undefined) return { App: { f: substCore(e.App.f, map), args: e.App.args.map((a) => substCore(a, map)), span: e.App.span } };
  if (e.Let !== undefined) return { Let: { name: e.Let.name, rhs: substCore(e.Let.rhs, map), body: substCore(e.Let.body, map), span: e.Let.span } };
  if (e.If !== undefined) return { If: { cond: substCore(e.If.cond, map), then: substCore(e.If.then, map), else_: substCore(e.If.else_, map), span: e.If.span } };
  if (e.Obj !== undefined) return { Obj: { fields: e.Obj.fields.map(([k, c]) => [k, substCore(c, map)]), span: e.Obj.span } };
  if (e.Field !== undefined) return { Field: { obj: substCore(e.Field.obj, map), name: e.Field.name, span: e.Field.span } };
  return e;
}

function printCore(e) {
  if (e.Lit !== undefined) return valueToLit(e.Lit.value);
  if (e.Var !== undefined) return e.Var.name;
  if (e.App !== undefined) return "(" + printCore(e.App.f) + " " + e.App.args.map(printCore).join(" ") + ")";
  if (e.If !== undefined) return "(if " + printCore(e.If.cond) + " " + printCore(e.If.then) + " " + printCore(e.If.else_) + ")";
  if (e.Let !== undefined) return "(let [" + e.Let.name + " " + printCore(e.Let.rhs) + "] " + printCore(e.Let.body) + ")";
  if (e.Obj !== undefined) return "(obj " + e.Obj.fields.map(([k, c]) => "(" + k + " " + printCore(c) + ")").join(" ") + ")";
  if (e.Field !== undefined) return printCore(e.Field.obj) + "." + e.Field.name;
  return "<expr>";
}

// ---- 結果 JSON（interp の prop_result_json と同一バイト列。オブジェクトキーは serde_json 同様ソート） ----

class Raw {
  constructor(s) {
    this.s = s;
  }
}

function $pnum(n) {
  if (!Number.isFinite(n)) return "null"; // serde_json: NaN/±Inf は null
  if (Object.is(n, -0)) return "-0.0";
  let s = String(n);
  if (!/[.eE]/.test(s)) s += ".0"; // serde_json(ryu) は整数 float に .0 を付ける
  return s;
}

function $pj(v) {
  if (v === null) return "null";
  if (v instanceof Raw) return v.s;
  if (typeof v === "boolean") return v ? "true" : "false";
  if (typeof v === "number") return $pnum(v);
  if (typeof v === "bigint") return '{"tag":"Dec","value":' + JSON.stringify($printDec(v)) + "}";
  if (typeof v === "string") return JSON.stringify(v);
  if (Array.isArray(v)) return "[" + v.map($pj).join(",") + "]";
  if (v instanceof Uint8Array) return "[" + [...v].join(",") + "]";
  if (v instanceof Map) return '{"entries":[' + [...v].map(([k, x]) => "[" + $pj(k) + "," + $pj(x) + "]").join(",") + '],"tag":"Map"}';
  if (v instanceof Set) return '{"items":[' + [...v].map($pj).join(",") + '],"tag":"Set"}';
  if (typeof v === "object") {
    const ks = Object.keys(v).sort();
    return "{" + ks.map((k) => JSON.stringify(k) + ":" + $pj(v[k])).join(",") + "}";
  }
  return "null";
}

function resultJson(pairs) {
  const sorted = pairs.slice().sort((a, b) => (a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : 0));
  return "{" + sorted.map(([k, v]) => JSON.stringify(k) + ":" + $pj(v)).join(",") + "}";
}

// props: [{ name, params: [{name, ty}], fn, core }]。seed は BigInt。maxTrials=100。
// 各 prop を `prop <i>: ok <json>` / `prop <i>: FAIL <json>` で stdout へ（interp の prop_result_json と同一 JSON）。
export async function runProps(props, seed, maxTrials) {
  for (let i = 0; i < props.length; i++) {
    const spec = props[i];
    let notGen = null;
    for (const p of spec.params) {
      if (!isGeneratable(p.ty)) {
        notGen = p.name;
        break;
      }
    }
    if (notGen !== null) {
      console.log(
        "prop " + i + ": FAIL " +
          resultJson([
            ["errors", [{ kind: "type-mismatch", message: "prop '" + spec.name + "' > forall パラメータ '" + notGen + "' は生成できない型です（関数型など）" }]],
            ["id", new Raw(String(i))],
            ["ok", false],
          ])
      );
      continue;
    }
    const rng = makeRng(seed);
    let trials = 0;
    let failed = false;
    while (trials < maxTrials) {
      trials++;
      const args = spec.params.map((p) => {
        const g = genValue(p.ty, rng, 0);
        return g === undefined ? null : g;
      });
      let r;
      let err = null;
      try {
        r = await spec.fn(...args);
      } catch (e) {
        err = String(e && e.message !== undefined ? e.message : e);
      }
      if (err === null && r === true) continue;
      const shrunk = await shrinkProp(spec, args);
      const cex = {};
      const shr = {};
      for (let j = 0; j < spec.params.length; j++) {
        cex[spec.params[j].name] = args[j];
        shr[spec.params[j].name] = shrunk[j];
      }
      const map = {};
      for (let j = 0; j < spec.params.length; j++) map[spec.params[j].name] = shrunk[j];
      const bodyLit = printCore(substCore(spec.core, map));
      const pairs = [
        ["counterexample", cex],
        ["fix", { op: "insert", path: "(prop " + spec.name + ")", template: "(example " + bodyLit + " => false)" }],
        ["id", new Raw(String(i))],
        ["ok", false],
        ["prop", spec.name],
        ["seed", new Raw(String(seed))],
        ["shrunk", shr],
        ["trials", new Raw(String(trials))],
      ];
      if (err !== null) pairs.push(["error", err]);
      else if (r !== false) pairs.push(["error", "prop 本体は bool を返すべきですが " + $printVal(r) + " が返りました"]);
      console.log("prop " + i + ": FAIL " + resultJson(pairs));
      failed = true;
      break;
    }
    if (!failed) {
      console.log(
        "prop " + i + ": ok " +
          resultJson([
            ["id", new Raw(String(i))],
            ["ok", true],
            ["prop", spec.name],
            ["seed", new Raw(String(seed))],
            ["trials", new Raw(String(trials))],
          ])
      );
    }
  }
}
