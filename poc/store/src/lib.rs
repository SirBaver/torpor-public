// Content-addressed store (Merkle DAG) — valide H-rollback-latence
//
// Modèle : chaque snapshot d'état agent est un bloc identifié par son hash SHA-256.
// Un snapshot référence un parent (hash du snapshot précédent). Le rollback
// consiste à suivre la chaîne de parents jusqu'au point cible.
//
// S2 : la cohérence est garantie par l'immuabilité des blocs (hash = identité).
// P2 : le rollback est O(log N) en nombre de blocs modifiés, O(depth) en traversée.

use sha2::{Digest, Sha256};
use std::path::Path;

pub use rocksdb::Cache;

mod error;
pub use error::StoreError;

pub type BlockHash = [u8; 32];

/// En-tête d'un snapshot d'état agent.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SnapshotHeader {
    /// Hash du bloc de données associé (SHA-256).
    pub data_hash: BlockHash,
    /// Hash du snapshot parent (None pour le snapshot initial).
    pub parent: Option<BlockHash>,
    /// Numéro de séquence monotone dans la chaîne d'un agent.
    pub seq: u64,
    /// Timestamp Unix en microsecondes.
    pub ts_us: u64,
}

/// Identité d'un snapshot : hash de son en-tête sérialisé.
pub fn snapshot_id(header: &SnapshotHeader) -> BlockHash {
    let encoded = bincode::serialize(header).expect("serialization is infallible");
    let mut hasher = Sha256::new();
    hasher.update(&encoded);
    hasher.finalize().into()
}

/// Hash d'un bloc de données brutes.
pub fn data_hash(data: &[u8]) -> BlockHash {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hasher.finalize().into()
}

pub struct ContentStore {
    db: rocksdb::DB,
}

impl ContentStore {
    /// Ouvre (ou crée) un store dans `path`.
    ///
    /// `shared_cache` : cache LRU partagé avec CausalLog (P7). Si None, un cache local
    /// de 64 MB est créé. Passer `Some(cache.clone())` pour coordonner avec CausalLog.
    pub fn open(path: &Path, shared_cache: Option<Cache>) -> Result<Self, StoreError> {
        let cache = shared_cache.unwrap_or_else(|| Cache::new_lru_cache(64 * 1024 * 1024));

        let mut db_opts = rocksdb::Options::default();
        db_opts.create_if_missing(true);
        db_opts.create_missing_column_families(true);
        db_opts.set_bytes_per_sync(1_048_576);
        db_opts.set_wal_bytes_per_sync(1_048_576);
        db_opts.set_max_background_jobs(4); // P9 : aligner avec CausalLog

        let make_cf_opts = |cache: &Cache| {
            let mut block_opts = rocksdb::BlockBasedOptions::default();
            block_opts.set_bloom_filter(10.0, false);
            block_opts.set_block_cache(cache);
            block_opts.set_cache_index_and_filter_blocks(true);
            block_opts.set_pin_l0_filter_and_index_blocks_in_cache(true);

            let mut o = rocksdb::Options::default();
            o.set_block_based_table_factory(&block_opts);
            o.set_write_buffer_size(64 * 1024 * 1024);
            o.set_max_write_buffer_number(2);
            o.set_max_bytes_for_level_base(256 * 1024 * 1024);
            o.set_compression_per_level(&[
                rocksdb::DBCompressionType::None, rocksdb::DBCompressionType::None,
                rocksdb::DBCompressionType::Lz4,  rocksdb::DBCompressionType::Lz4,
                rocksdb::DBCompressionType::Lz4,  rocksdb::DBCompressionType::Zstd,
                rocksdb::DBCompressionType::Zstd,
            ]);
            o
        };

        let cfs = vec![
            rocksdb::ColumnFamilyDescriptor::new("blocks", make_cf_opts(&cache)),
            rocksdb::ColumnFamilyDescriptor::new("headers", make_cf_opts(&cache)),
        ];
        let db = rocksdb::DB::open_cf_descriptors(&db_opts, path, cfs)?;
        Ok(Self { db })
    }

    /// Écrit un bloc de données brutes. Idempotent (même contenu = même hash).
    pub fn put_block(&self, data: &[u8]) -> Result<BlockHash, StoreError> {
        let h = data_hash(data);
        let cf = self.db.cf_handle("blocks").unwrap();
        self.db.put_cf(cf, h, data)?;
        Ok(h)
    }

    /// Enregistre un snapshot et retourne son identifiant.
    pub fn put_snapshot(&self, header: SnapshotHeader) -> Result<BlockHash, StoreError> {
        let id = snapshot_id(&header);
        let encoded = bincode::serialize(&header).expect("serialization is infallible");
        let cf = self.db.cf_handle("headers").unwrap();
        self.db.put_cf(cf, id, encoded)?;
        Ok(id)
    }

    /// Lit un bloc de données.
    pub fn get_block(&self, hash: &BlockHash) -> Result<Option<Vec<u8>>, StoreError> {
        let cf = self.db.cf_handle("blocks").unwrap();
        Ok(self.db.get_cf(cf, hash)?)
    }

    /// Lit l'en-tête d'un snapshot.
    pub fn get_header(&self, id: &BlockHash) -> Result<Option<SnapshotHeader>, StoreError> {
        let cf = self.db.cf_handle("headers").unwrap();
        match self.db.get_cf(cf, id)? {
            None => Ok(None),
            Some(bytes) => {
                let header: SnapshotHeader = bincode::deserialize(&bytes)?;
                Ok(Some(header))
            }
        }
    }

    /// Vérifie la présence d'un snapshot sans désérialiser l'en-tête (F7).
    pub fn has_snapshot(&self, id: &BlockHash) -> Result<bool, StoreError> {
        let cf = self.db.cf_handle("headers").unwrap();
        Ok(self.db.get_cf(cf, id)?.is_some())
    }

    /// Retourne la chaîne de snapshots depuis `tip` jusqu'à `target_seq` (rollback).
    ///
    /// Complexité : O(tip.seq - target_seq) traversées de la chaîne de parents.
    /// Chaque traversée est un point lookup RocksDB.
    pub fn rollback_path(
        &self,
        tip: &BlockHash,
        target_seq: u64,
    ) -> Result<Vec<BlockHash>, StoreError> {
        let mut path = Vec::new();
        let mut current = *tip;

        loop {
            let header = self
                .get_header(&current)?
                .ok_or(StoreError::MissingBlock(current))?;

            path.push(current);

            if header.seq == target_seq {
                break;
            }
            if header.seq < target_seq {
                return Err(StoreError::TargetBeyondHistory {
                    current_seq: header.seq,
                    target_seq,
                });
            }

            current = header.parent.ok_or(StoreError::NoParent(current))?;
        }

        Ok(path)
    }

    /// Itère les `data_hash` référencés par tous les headers présents (phase mark du GC).
    /// Silencieusement ignore les entrées corrompues (ne doivent pas exister).
    pub fn iter_header_data_hashes(&self) -> impl Iterator<Item = BlockHash> + '_ {
        let cf = self.db.cf_handle("headers").unwrap();
        self.db
            .iterator_cf(&cf, rocksdb::IteratorMode::Start)
            .filter_map(|r| r.ok())
            .filter_map(|(_, value)| {
                bincode::deserialize::<SnapshotHeader>(&value)
                    .ok()
                    .map(|h| h.data_hash)
            })
    }

    /// Itère les hashes de tous les blocs présents (phase sweep du GC).
    /// Chaque item est la clé de la CF `blocks` = `data_hash` du bloc.
    pub fn iter_block_hashes(&self) -> impl Iterator<Item = BlockHash> + '_ {
        let cf = self.db.cf_handle("blocks").unwrap();
        self.db
            .iterator_cf(&cf, rocksdb::IteratorMode::Start)
            .filter_map(|r| r.ok())
            .filter_map(|(key, _)| {
                if key.len() == 32 {
                    let mut hash = [0u8; 32];
                    hash.copy_from_slice(&key);
                    Some(hash)
                } else {
                    None
                }
            })
    }

    /// Lit une propriété RocksDB entière pour une column family nommée.
    ///
    /// Exemples de propriétés utiles pour les benchmarks :
    ///   "rocksdb.cur-size-all-mem-tables"  — mémoire memtable courante (Ko → octets)
    ///   "rocksdb.num-files-at-level0"      — fichiers L0 (signal stall imminence)
    ///   "rocksdb.compaction-pending"        — 1 si une compaction est en attente
    pub fn get_rocksdb_int_property(&self, cf_name: &str, prop: &str) -> Option<u64> {
        let cf = self.db.cf_handle(cf_name)?;
        self.db.property_int_value_cf(&cf, prop).ok().flatten()
    }

    /// Mémoire totale des memtables sur toutes les column families (octets).
    /// À soustraire du RSS pour obtenir la croissance hors-LSM (critère ADR-0033).
    pub fn total_memtable_bytes(&self) -> u64 {
        ["blocks", "headers"]
            .iter()
            .filter_map(|cf| {
                self.get_rocksdb_int_property(cf, "rocksdb.cur-size-all-mem-tables")
            })
            .sum()
    }

    /// Mémoire utilisée par le block cache sur toutes les column families (octets).
    /// À inclure dans rss_adj = RSS − memtable − block_cache (TODO P3B).
    pub fn block_cache_usage_bytes(&self) -> u64 {
        ["blocks", "headers"]
            .iter()
            .filter_map(|cf| {
                self.get_rocksdb_int_property(cf, "rocksdb.block-cache-usage")
            })
            .sum()
    }

    /// Construit une chaîne synthétique de `n` snapshots avec des blocs de `block_size` bytes.
    /// Utilisé pour les benchmarks H-rollback-latence.
    pub fn build_chain(
        &self,
        n: u64,
        block_size: usize,
    ) -> Result<BlockHash, StoreError> {
        let data = vec![0xABu8; block_size];
        let data_h = self.put_block(&data)?;

        let mut parent: Option<BlockHash> = None;
        let mut tip = [0u8; 32];

        for seq in 0..n {
            let header = SnapshotHeader {
                data_hash: data_h,
                parent,
                seq,
                ts_us: seq * 1_000_000,
            };
            tip = self.put_snapshot(header)?;
            parent = Some(tip);
        }

        Ok(tip)
    }
}
