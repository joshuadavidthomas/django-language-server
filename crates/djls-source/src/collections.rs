use std::hash::BuildHasherDefault;

use dashmap::DashMap;
use dashmap::DashSet;
use rustc_hash::FxHasher;

pub type FxDashMap<K, V> = DashMap<K, V, BuildHasherDefault<FxHasher>>;
pub type FxDashSet<K> = DashSet<K, BuildHasherDefault<FxHasher>>;
