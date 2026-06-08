// fast_hash.rs — portage no_std seL4 C.5
// hashbrown avec hasher custom remplacé par BTreeMap/BTreeSet (pas de conflit alloc).
// FastHashMapU64 et PageNumberHashSet conservent leurs noms pour que les callers
// n'aient pas à changer.

use crate::tree_store::PageNumber;
use alloc::collections::{BTreeMap, BTreeSet};

pub(crate) type FastHashMapU64<V> = BTreeMap<u64, V>;
pub(crate) type PageNumberHashSet = BTreeSet<PageNumber>;
