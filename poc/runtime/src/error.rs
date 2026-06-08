#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    #[error("wasmtime: {0}")]
    Wasmtime(#[from] wasmtime::Error),
    #[error("store: {0}")]
    Store(#[from] os_poc_store::StoreError),
    #[error("log: {0}")]
    Log(#[from] os_poc_causal_log::LogError),
    #[error("message dépasse la capacité mémoire WASM (max 64 KiB)")]
    MemoryOutOfBounds,
    #[error("message trop grand: {0} bytes")]
    MessageTooLarge(usize),
    #[error("spawn_child: envoi du message initial échoué (inbox fermée)")]
    SpawnFailed,
}
