// Shim no_std : ré-exporte alloc/core sous les chemins std.* attendus par redb.
// Technique "shadow std" — réduit les modifications sur les 38 fichiers sources.
//
// Ce module ne couvre PAS : std::fs, std::os, std::path (supprimés avec le file backend).
// std::io::Error est remplacé par StorageIoError (voir ci-dessous).

// ── Réexports core ────────────────────────────────────────────────────────────

pub use core::{cmp, convert, fmt, hash, marker, mem, ops, slice};
pub use core::result;
pub use core::option;

// ── Réexports alloc ───────────────────────────────────────────────────────────

pub use alloc::boxed;
pub use alloc::string;
pub use alloc::borrow;
pub use alloc::format;

pub mod collections {
    // HashMap/HashSet remplacés par BTreeMap/BTreeSet pour éviter le conflit
    // hashbrown 0.14 + build-std=alloc → E0464 "multiple candidates for alloc".
    // Les clés internes de redb (PageNumber=u32, TransactionId=u64) implémentent Ord.
    pub use alloc::collections::{BTreeMap as HashMap, BTreeMap, BTreeSet as HashSet, BTreeSet, VecDeque};
    pub use core::ops::Bound;
}

// ── std::sync → spin + alloc::sync ───────────────────────────────────────────

pub mod sync {
    pub use alloc::sync::Arc;
    pub use spin::{Mutex, MutexGuard, RwLock, RwLockReadGuard, RwLockWriteGuard};

    // Condvar : utilisé dans cached_file.rs — remplacé par spin
    // Sur seL4 single-threaded, Condvar est un no-op
    pub struct Condvar;
    impl Condvar {
        pub fn new() -> Self { Condvar }
        pub fn notify_one(&self) {}
        pub fn notify_all(&self) {}
        pub fn wait<'a, T>(&self, guard: MutexGuard<'a, T>) -> MutexGuard<'a, T> { guard }
    }

    // PoisonError simulé : spin::Mutex ne peut pas être empoisonné
    pub struct PoisonError<T>(T);
    impl<T> PoisonError<T> {
        pub fn into_inner(self) -> T { self.0 }
    }

    pub mod atomic {
        pub use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
    }
}

// ── std::io → type custom StorageIoError ─────────────────────────────────────
// std::io::Error est le bloqueur principal du portage.
// On fournit un type compatible minimal pour les besoins internes de redb.

pub mod io {
    use core::fmt;

    #[derive(Debug)]
    pub struct Error {
        pub kind: ErrorKind,
        msg: &'static str,
    }

    impl Error {
        pub fn new(kind: ErrorKind, msg: &'static str) -> Self {
            Self { kind, msg }
        }

        pub fn kind(&self) -> ErrorKind {
            self.kind
        }

        pub fn other(msg: &'static str) -> Self {
            Self { kind: ErrorKind::Other, msg }
        }
    }

    impl fmt::Display for Error {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "{:?}: {}", self.kind, self.msg)
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum ErrorKind {
        InvalidInput,
        InvalidData,
        UnexpectedEof,
        WriteZero,
        NotFound,
        AlreadyExists,
        PermissionDenied,
        Other,
    }

    pub type Result<T> = core::result::Result<T, Error>;


    impl From<ErrorKind> for Error {
        fn from(kind: ErrorKind) -> Self {
            Self { kind, msg: "" }
        }
    }

    // Traits Read/Write/Seek non fournis — non utilisés après suppression du file backend.
}

// ── std::thread → no-op pour seL4 single-threaded ────────────────────────────

pub mod thread {
    /// Toujours false sur seL4 — pas de panics inter-threads
    #[inline(always)]
    pub fn panicking() -> bool {
        false
    }
}

// ── Macros std non disponibles en no_std ─────────────────────────────────────
// println!/eprintln! : non disponibles. Les callers dans redb sont dans des
// chemins d'erreur ou de debug — accepter la perte silencieuse en no_std.
