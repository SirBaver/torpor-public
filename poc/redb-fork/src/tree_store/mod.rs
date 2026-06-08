use alloc::{vec::Vec, string::{String, ToString}, boxed::Box, borrow::ToOwned};
#[allow(unused_imports)] use alloc::format;
mod btree;
mod btree_base;
mod btree_iters;
mod btree_mutator;
pub(crate) mod page_store;
mod table_tree;
mod table_tree_base;

pub(crate) use btree::{
    Btree, BtreeMut, BtreeStats, PagePath, RawBtree, UntypedBtree, UntypedBtreeMut, btree_stats,
};
pub use btree_base::{AccessGuard, AccessGuardMut, AccessGuardMutInPlace};
pub(crate) use btree_base::{
    BRANCH, BranchAccessor, BranchMutator, BtreeHeader, Checksum, DEFERRED, LEAF, LeafAccessor,
    LeafMutator, RawLeafBuilder,
};
pub(crate) use btree_iters::{AllPageNumbersBtreeIter, BtreeExtractIf, BtreeRangeIter};
pub(crate) use page_store::ReadOnlyBackend;
pub(crate) use page_store::{
    FILE_FORMAT_VERSION3, MAX_PAIR_LENGTH, MAX_VALUE_LENGTH, PAGE_SIZE, Page, PageHint, PageNumber,
    PageTrackerPolicy, SerializedSavepoint, ShrinkPolicy, TransactionalMemory,
};
pub(crate) use table_tree::{PageListMut, TableTree, TableTreeMut};
pub(crate) use table_tree_base::{InternalTableDefinition, TableType};
pub use page_store::{InMemoryBackend, Savepoint};
