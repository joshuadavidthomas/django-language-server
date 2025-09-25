use dashmap::{DashMap, DashSet};
use rustc_hash::FxHasher;
use std::hash::BuildHasherDefault;

pub type FxDashMap<K, V> = DashMap<K, V, BuildHasherDefault<FxHasher>>;
pub type FxDashSet<K> = DashSet<K, BuildHasherDefault<FxHasher>>;
