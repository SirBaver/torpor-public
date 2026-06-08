#![no_std]
#![allow(
    clippy::default_trait_access,
    clippy::if_not_else,
    clippy::iter_not_returning_iterator,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::module_name_repetitions,
    clippy::must_use_candidate,
    clippy::needless_pass_by_value,
    clippy::redundant_closure_for_method_calls,
    clippy::similar_names,
    clippy::too_many_lines,
    clippy::unnecessary_wraps,
    clippy::unreadable_literal,
    // no_std port — certaines impls sont incomplètes
    clippy::panic,
    dead_code,
    unused_imports,
)]
#[macro_use]
extern crate alloc;

mod compat;
/// Module io exposé pour les implémenteurs externes de StorageBackend (C.5).
pub use compat::io;
// Doc crate supprimée — fork no_std seL4 C.5 (ADR-0042)

pub use db::{
    Builder, CacheStats, Database, MultimapTableDefinition, MultimapTableHandle, ReadOnlyDatabase,
    ReadableDatabase, RepairSession, StorageBackend, TableDefinition, TableHandle,
    UntypedMultimapTableHandle, UntypedTableHandle,
};
pub use error::{
    CommitError, CompactionError, DatabaseError, Error, SavepointError, SetDurabilityError,
    StorageError, TableError, TransactionError,
};
pub use multimap_table::{
    MultimapRange, MultimapTable, MultimapValue, ReadOnlyMultimapTable,
    ReadOnlyUntypedMultimapTable, ReadableMultimapTable,
};
pub use table::{
    ExtractIf, Range, ReadOnlyTable, ReadOnlyUntypedTable, ReadableTable, ReadableTableMetadata,
    Table, TableStats,
};
pub use transactions::{DatabaseStats, Durability, ReadTransaction, WriteTransaction};
pub use tree_store::{AccessGuard, AccessGuardMut, AccessGuardMutInPlace, Savepoint};
pub use types::{Key, MutInPlaceValue, TypeName, Value};

pub type Result<T = (), E = StorageError> = core::result::Result<T, E>;

pub mod backends;
mod complex_types;
mod db;
mod error;
mod multimap_table;
mod sealed;
mod table;
mod transaction_tracker;
mod transactions;
mod tree_store;
mod tuple_types;
mod types;

#[cfg(test)]
fn create_tempfile() -> tempfile::NamedTempFile {
    if cfg!(target_os = "wasi") {
        tempfile::NamedTempFile::new_in("/tmp").unwrap()
    } else {
        tempfile::NamedTempFile::new().unwrap()
    }
}
