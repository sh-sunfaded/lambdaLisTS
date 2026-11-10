use std::rc::Rc;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::value::Value;

/// map/set のキー（design/interp-capability.md §1.1: str/sym/int のみ。現 SPEC には sym/int
/// が無いため string/number（リテラル文字列を含む）に写像。REPORT.md に記録）。
/// number キーは JS Map の SameValueZero に合わせて正規化する（-0 は +0、NaN は正規ビット）。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum MapKey {
    Str(Rc<str>),
    Num(u64),
}

fn canon_bits(n: f64) -> u64 {
    if n.is_nan() {
        return 0x7ff8_0000_0000_0000;
    }
    if n == 0.0 {
        return 0;
    }
    n.to_bits()
}

impl MapKey {
    pub fn from_value(v: &Value) -> Option<MapKey> {
        match v {
            Value::Str(s) => Some(MapKey::Str(s.clone())),
            Value::Num(n) => Some(MapKey::Num(canon_bits(*n))),
            _ => None,
        }
    }

    pub fn to_value(&self) -> Value {
        match self {
            MapKey::Str(s) => Value::Str(s.clone()),
            MapKey::Num(b) => Value::Num(f64::from_bits(*b)),
        }
    }
}

impl Serialize for MapKey {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.to_value().serialize(s)
    }
}

impl<'de> Deserialize<'de> for MapKey {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = Value::deserialize(d)?;
        MapKey::from_value(&v).ok_or_else(|| serde::de::Error::custom("invalid map key"))
    }
}

/// 挿入順を保持する単純永続木（AVL。キーで平衡、挿入順は seq で保持）。
/// 更新は path-copying で新しい木を返し、既存の共有構造は壊さない
/// （design/interp-capability.md §1.1「永続構造」「挿入順反復」）。
#[derive(Debug, Clone)]
struct Node {
    key: MapKey,
    val: Value,
    seq: u64,
    height: u8,
    left: Option<Rc<Node>>,
    right: Option<Rc<Node>>,
}

fn height(n: &Option<Rc<Node>>) -> u8 {
    n.as_ref().map_or(0, |x| x.height)
}

fn mk(key: MapKey, val: Value, seq: u64, left: Option<Rc<Node>>, right: Option<Rc<Node>>) -> Rc<Node> {
    let h = 1 + height(&left).max(height(&right));
    Rc::new(Node {
        key,
        val,
        seq,
        height: h,
        left,
        right,
    })
}

fn rotate_right(y: Rc<Node>) -> Rc<Node> {
    let x = y.left.as_ref().expect("rotate_right: left exists").clone();
    let t2 = x.right.clone();
    mk(
        x.key.clone(),
        x.val.clone(),
        x.seq,
        x.left.clone(),
        Some(mk(
            y.key.clone(),
            y.val.clone(),
            y.seq,
            t2,
            y.right.clone(),
        )),
    )
}

fn rotate_left(x: Rc<Node>) -> Rc<Node> {
    let y = x.right.as_ref().expect("rotate_left: right exists").clone();
    let t2 = y.left.clone();
    mk(
        y.key.clone(),
        y.val.clone(),
        y.seq,
        Some(mk(
            x.key.clone(),
            x.val.clone(),
            x.seq,
            x.left.clone(),
            t2,
        )),
        y.right.clone(),
    )
}

fn balance(n: Rc<Node>) -> Rc<Node> {
    let l = height(&n.left) as i16;
    let r = height(&n.right) as i16;
    if l - r > 1 {
        let ll = height(&n.left.as_ref().expect("left").left) as i16;
        let lr = height(&n.left.as_ref().expect("left").right) as i16;
        let x = n.left.as_ref().expect("left").clone();
        if ll < lr {
            let x = mk(
                x.key.clone(),
                x.val.clone(),
                x.seq,
                x.left.clone(),
                Some(rotate_right(x.right.as_ref().expect("right").clone())),
            );
            return rotate_right(mk(
                n.key.clone(),
                n.val.clone(),
                n.seq,
                Some(x),
                n.right.clone(),
            ));
        }
        return rotate_right(n);
    }
    if r - l > 1 {
        let rr = height(&n.right.as_ref().expect("right").right) as i16;
        let rl = height(&n.right.as_ref().expect("right").left) as i16;
        let y = n.right.as_ref().expect("right").clone();
        if rr < rl {
            let y = mk(
                y.key.clone(),
                y.val.clone(),
                y.seq,
                Some(rotate_left(y.left.as_ref().expect("left").clone())),
                y.right.clone(),
            );
            return rotate_left(mk(
                n.key.clone(),
                n.val.clone(),
                n.seq,
                n.left.clone(),
                Some(y),
            ));
        }
        return rotate_left(n);
    }
    n
}

fn insert_node(n: Option<Rc<Node>>, key: MapKey, val: Value, seq: u64) -> (Rc<Node>, Option<Value>) {
    match n {
        None => (mk(key, val, seq, None, None), None),
        Some(node) => {
            if key < node.key {
                let (l, old) = insert_node(node.left.clone(), key, val, seq);
                (balance(mk(node.key.clone(), node.val.clone(), node.seq, Some(l), node.right.clone())), old)
            } else if key > node.key {
                let (r, old) = insert_node(node.right.clone(), key, val, seq);
                (balance(mk(node.key.clone(), node.val.clone(), node.seq, node.left.clone(), Some(r))), old)
            } else {
                (mk(node.key.clone(), val, node.seq, node.left.clone(), node.right.clone()), Some(node.val.clone()))
            }
        }
    }
}

fn min_node(n: &Rc<Node>) -> Rc<Node> {
    match &n.left {
        Some(l) => min_node(l),
        None => n.clone(),
    }
}

fn remove_node(n: Option<Rc<Node>>, key: &MapKey) -> (Option<Rc<Node>>, Option<Value>) {
    let Some(node) = n else {
        return (None, None);
    };
    if *key < node.key {
        let (l, old) = remove_node(node.left.clone(), key);
        (Some(balance(mk(node.key.clone(), node.val.clone(), node.seq, l, node.right.clone()))), old)
    } else if *key > node.key {
        let (r, old) = remove_node(node.right.clone(), key);
        (Some(balance(mk(node.key.clone(), node.val.clone(), node.seq, node.left.clone(), r))), old)
    } else if node.left.is_some() && node.right.is_some() {
        let succ = min_node(node.right.as_ref().expect("right"));
        let (r, _) = remove_node(node.right.clone(), &succ.key);
        (
            Some(balance(mk(
                succ.key.clone(),
                succ.val.clone(),
                node.seq,
                node.left.clone(),
                r,
            ))),
            Some(node.val.clone()),
        )
    } else {
        let child = if node.left.is_some() {
            node.left.clone()
        } else {
            node.right.clone()
        };
        (child, Some(node.val.clone()))
    }
}

fn get_node<'a>(n: &'a Option<Rc<Node>>, key: &MapKey) -> Option<&'a Value> {
    let mut cur: Option<&'a Node> = n.as_deref();
    while let Some(node) = cur {
        if *key < node.key {
            cur = node.left.as_deref();
        } else if *key > node.key {
            cur = node.right.as_deref();
        } else {
            return Some(&node.val);
        }
    }
    None
}

fn collect(n: &Option<Rc<Node>>, out: &mut Vec<(u64, MapKey, Value)>) {
    if let Some(node) = n {
        collect(&node.left, out);
        out.push((node.seq, node.key.clone(), node.val.clone()));
        collect(&node.right, out);
    }
}

#[derive(Debug, Clone)]
pub struct PersistentMap {
    root: Option<Rc<Node>>,
    len: usize,
    next_seq: u64,
}

impl Default for PersistentMap {
    fn default() -> Self {
        Self::new()
    }
}

impl PartialEq for PersistentMap {
    fn eq(&self, other: &Self) -> bool {
        if self.len != other.len {
            return false;
        }
        self.entries().iter().all(|(k, v)| {
            other
                .get(k)
                .is_some_and(|ov| *ov == *v)
        })
    }
}

impl PersistentMap {
    pub fn new() -> Self {
        PersistentMap {
            root: None,
            len: 0,
            next_seq: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn get(&self, key: &MapKey) -> Option<&Value> {
        get_node(&self.root, key)
    }

    pub fn has(&self, key: &MapKey) -> bool {
        self.get(key).is_some()
    }

    /// 挿入または上書き。既存キーの上書きでは挿入順（seq）を維持する。
    /// 戻り値: (新しい map, 上書き前の値)。
    pub fn insert(&self, key: MapKey, val: Value) -> (PersistentMap, Option<Value>) {
        let existing = self.get(&key).cloned();
        let (root, replaced) = insert_node(self.root.clone(), key, val, self.next_seq);
        let len = if replaced.is_some() { self.len } else { self.len + 1 };
        (
            PersistentMap {
                root: Some(root),
                len,
                next_seq: if replaced.is_some() { self.next_seq } else { self.next_seq + 1 },
            },
            existing,
        )
    }

    pub fn remove(&self, key: &MapKey) -> (PersistentMap, Option<Value>) {
        let (root, removed) = remove_node(self.root.clone(), key);
        let len = if removed.is_some() { self.len - 1 } else { self.len };
        (
            PersistentMap {
                root,
                len,
                next_seq: self.next_seq,
            },
            removed,
        )
    }

    /// 挿入順の (key, value) 一覧。
    pub fn entries(&self) -> Vec<(MapKey, Value)> {
        let mut v = Vec::new();
        collect(&self.root, &mut v);
        v.sort_by_key(|(seq, _, _)| *seq);
        v.into_iter().map(|(_, k, val)| (k, val)).collect()
    }

    pub fn keys(&self) -> Vec<MapKey> {
        self.entries().into_iter().map(|(k, _)| k).collect()
    }
}

impl Serialize for PersistentMap {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let entries: Vec<(MapKey, Value)> = self.entries();
        entries.serialize(s)
    }
}

impl<'de> Deserialize<'de> for PersistentMap {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let entries: Vec<(MapKey, Value)> = Vec::deserialize(d)?;
        let mut m = PersistentMap::new();
        for (k, v) in entries {
            let (next, _) = m.insert(k, v);
            m = next;
        }
        Ok(m)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(s: &str) -> MapKey {
        MapKey::Str(Rc::from(s))
    }

    fn num_map(pairs: &[(&str, f64)]) -> PersistentMap {
        let mut m = PersistentMap::new();
        for (key, v) in pairs {
            let (next, _) = m.insert(k(key), Value::Num(*v));
            m = next;
        }
        m
    }

    #[test]
    fn insert_preserves_insertion_order_and_replaces_in_place() {
        let m = num_map(&[("b", 2.0), ("a", 1.0), ("c", 3.0)]);
        let keys: Vec<String> = m.keys().iter().map(|k| match k {
            MapKey::Str(s) => s.to_string(),
            MapKey::Num(_) => "#".to_string(),
        }).collect();
        assert_eq!(keys, ["b", "a", "c"]);
        let (m2, old) = m.insert(k("a"), Value::Num(99.0));
        assert_eq!(old, Some(Value::Num(1.0)));
        assert_eq!(m2.len(), 3);
        assert_eq!(m2.get(&k("a")), Some(&Value::Num(99.0)));
        let keys2: Vec<String> = m2.keys().iter().map(|k| match k {
            MapKey::Str(s) => s.to_string(),
            MapKey::Num(_) => "#".to_string(),
        }).collect();
        assert_eq!(keys2, ["b", "a", "c"], "上書きでは順序が変わらない");
        // 既存 map は不変（永続性）
        assert_eq!(m.get(&k("a")), Some(&Value::Num(1.0)));
        assert_eq!(m.len(), 3);
    }

    #[test]
    fn remove_keeps_order_of_remaining_and_old_map_unchanged() {
        let m = num_map(&[("b", 2.0), ("a", 1.0), ("c", 3.0)]);
        let (m2, removed) = m.remove(&k("a"));
        assert_eq!(removed, Some(Value::Num(1.0)));
        assert_eq!(m2.len(), 2);
        let keys: Vec<String> = m2.keys().into_iter().filter_map(|k| match k {
            MapKey::Str(s) => Some(s.to_string()),
            MapKey::Num(_) => None,
        }).collect();
        assert_eq!(keys, ["b", "c"]);
        assert_eq!(m.get(&k("a")), Some(&Value::Num(1.0)));
        let (m3, removed_none) = m2.remove(&k("zzz"));
        assert_eq!(removed_none, None);
        assert_eq!(m3.len(), 2);
    }

    #[test]
    fn number_keys_canonicalize_minus_zero_and_nan() {
        let m = PersistentMap::new();
        let (m, _) = m.insert(MapKey::from_value(&Value::Num(-0.0)).unwrap(), Value::Num(1.0));
        assert!(m.has(&MapKey::from_value(&Value::Num(0.0)).unwrap()));
        let (m, _) = m.insert(MapKey::from_value(&Value::Num(f64::NAN)).unwrap(), Value::Num(2.0));
        assert_eq!(m.len(), 2);
        assert!(m.has(&MapKey::from_value(&Value::Num(f64::NAN)).unwrap()));
        let m2 = PersistentMap::new();
        let (m2, _) = m2.insert(MapKey::from_value(&Value::Num(f64::NAN)).unwrap(), Value::Num(3.0));
        let (m2, old) = m2.insert(MapKey::from_value(&Value::Num(f64::NAN)).unwrap(), Value::Num(4.0));
        assert_eq!(old, Some(Value::Num(3.0)));
        assert_eq!(m2.len(), 1);
        let _ = m;
    }

    #[test]
    fn large_tree_stays_balanced_and_persistent() {
        let mut m = PersistentMap::new();
        for i in 0..2000u32 {
            let key = MapKey::Num(canon_bits(i as f64));
            let (next, _) = m.insert(key, Value::Num(i as f64));
            m = next;
        }
        assert_eq!(m.len(), 2000);
        assert!(m.root.as_ref().map_or(0, |r| r.height) < 20, "AVL の高さは O(log n)");
        let (m2, _) = m.remove(&MapKey::Num(canon_bits(1000.0)));
        assert_eq!(m2.len(), 1999);
        assert!(m.has(&MapKey::Num(canon_bits(1000.0))));
        assert!(!m2.has(&MapKey::Num(canon_bits(1000.0))));
        // seq 順（= 挿入順）
        let entries = m.entries();
        assert_eq!(entries[0].0, MapKey::Num(canon_bits(0.0)));
        assert_eq!(entries[1999].0, MapKey::Num(canon_bits(1999.0)));
    }

    #[test]
    fn serde_roundtrip_keeps_insertion_order() {
        let m = num_map(&[("b", 2.0), ("a", 1.0)]);
        let j = serde_json::to_string(&m).expect("ser");
        let back: PersistentMap = serde_json::from_str(&j).expect("de");
        let keys: Vec<String> = back.keys().into_iter().filter_map(|k| match k {
            MapKey::Str(s) => Some(s.to_string()),
            MapKey::Num(_) => None,
        }).collect();
        assert_eq!(keys, ["b", "a"]);
        assert_eq!(back, m);
    }

    #[test]
    fn equality_is_order_insensitive() {
        let a = num_map(&[("a", 1.0), ("b", 2.0)]);
        let b = num_map(&[("b", 2.0), ("a", 1.0)]);
        assert_eq!(a, b);
        let c = num_map(&[("a", 1.0), ("b", 3.0)]);
        assert_ne!(a, c);
    }
}
