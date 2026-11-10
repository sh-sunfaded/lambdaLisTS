export const ok = (value) => ({ tag: "Ok", value });
export const err = (value) => ({ tag: "Err", value });

const CYCLIC_EQ_MESSAGE = "lists runtime-error: cyclic structure in `=` (SPEC 8.3/8.6)";

export function structuralEqual(a, b) {
  return eqVisit(a, b, new WeakMap());
}

function eqVisit(a, b, path) {
  if (typeof a === "number" && typeof b === "number") return a === b;
  if (typeof a === "bigint" || typeof b === "bigint") return a === b;
  if (a === null || b === null) return a === b;
  if (typeof a !== typeof b) return false;
  // design/interp-capability.md §1.5: ref は参照等価（Rust 側は Rc::ptr_eq と同一）
  if (a !== null && typeof a === "object" && a.__lists_ref__ === true) return a === b;
  if (b !== null && typeof b === "object" && b.__lists_ref__ === true) return a === b;
  if (Array.isArray(a) || Array.isArray(b)) {
    if (!Array.isArray(a) || !Array.isArray(b) || a.length !== b.length) return false;
    return eqComposite(a, b, path, () => a.every((x, i) => eqVisit(x, b[i], path)));
  }
  // design/interp-capability.md §1.1/§7: map は内部 Map、set は内部 Set。
  // 構造的等価はキー集合と値で決まり、挿入順は無視する（deep-equal）。
  if (a instanceof Map || b instanceof Map) {
    if (!(a instanceof Map) || !(b instanceof Map) || a.size !== b.size) return false;
    return eqComposite(a, b, path, () => {
      for (const [k, v] of a) {
        if (!b.has(k) || !eqVisit(v, b.get(k), path)) return false;
      }
      return true;
    });
  }
  if (a instanceof Set || b instanceof Set) {
    if (!(a instanceof Set) || !(b instanceof Set) || a.size !== b.size) return false;
    return eqComposite(a, b, path, () => {
      for (const x of a) if (!b.has(x)) return false;
      return true;
    });
  }
  if (typeof a === "object" && typeof b === "object") {
    const ka = Object.keys(a);
    const kb = Object.keys(b);
    if (ka.length !== kb.length) return false;
    return eqComposite(a, b, path, () => ka.every((k) => k in b && eqVisit(a[k], b[k], path)));
  }
  return a === b;
}

function eqComposite(a, b, path, children) {
  let partners = path.get(a);
  if (partners !== undefined && partners.has(b)) throw new Error(CYCLIC_EQ_MESSAGE);
  if (partners === undefined) {
    partners = new WeakSet();
    path.set(a, partners);
  }
  partners.add(b);
  try {
    return children();
  } finally {
    partners.delete(b);
  }
}
