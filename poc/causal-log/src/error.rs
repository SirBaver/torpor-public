use crate::ActionId;

#[derive(Debug, thiserror::Error)]
pub enum LogError {
    #[error("RocksDB: {0}")]
    Rocks(#[from] rocksdb::Error),

    #[error("deserialization: {0}")]
    Deserialize(#[from] bincode::Error),

    #[error("action inconnue: {}", hex::encode(.0))]
    UnknownAction(ActionId),
}
