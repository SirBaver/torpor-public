use crate::BlockHash;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("RocksDB: {0}")]
    Rocks(#[from] rocksdb::Error),

    #[error("deserialization: {0}")]
    Deserialize(#[from] bincode::Error),

    #[error("bloc manquant: {}", hex::encode(.0))]
    MissingBlock(BlockHash),

    #[error("pas de parent pour le bloc: {}", hex::encode(.0))]
    NoParent(BlockHash),

    #[error("rollback demandé à seq={target_seq} mais le bloc courant est à seq={current_seq}")]
    TargetBeyondHistory { current_seq: u64, target_seq: u64 },
}
