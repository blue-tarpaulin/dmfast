//! Hash maps for indexes over BYOND's data: Fx hashing, fast and not DoS-resistant (the keys are BYOND's own data).

pub use rustc_hash::{FxBuildHasher, FxHashMap as Map, FxHashSet as Set, FxHasher};

/// An empty [`Map`], usable in a `static`: `static INDEX: Mutex<Map<K, V>> = Mutex::new(hash::map());`.
pub const fn map<K, V>() -> Map<K, V> {
    Map::with_hasher(FxBuildHasher)
}

/// An empty [`Set`], usable in a `static`.
pub const fn set<K>() -> Set<K> {
    Set::with_hasher(FxBuildHasher)
}
