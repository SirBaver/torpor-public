// Layer 0 — log causal append-only sur RocksDB.
//
// Deux column families :
//   - `default` : clé = action_id (SHA-256, 32B), valeur = LogEntry bincode.
//     Lookup O(1) par action_id (P3 stricte).
//   - `agent_ts` : clé = agent_id(16B) || ts_ms_BE(8B) || action_id(32B), valeur vide.
//     Scan de préfixe O(k) pour les range queries (P3b). ADR-0011 §index.
//
// Atomicité cross-CF garantie par WriteBatch (append écrit dans les deux CFs en un seul batch).
// Voir ADR-0002, ADR-0009, ADR-0010, ADR-0011.

use rocksdb::{
    BlockBasedOptions, Cache, ColumnFamilyDescriptor, DBCompressionType,
    Direction, IteratorMode, Options, SliceTransform, WriteBatch, WriteOptions, DB,
};
use sha2::{Digest, Sha256};

mod error;
pub use error::LogError;

// ── Types d'émission (ADR-0010 §2) ────────────────────────────────────────────

/// Type d'une émission publiée via `emit()`.
/// Encodé sur u8 dans l'enveloppe MessagePack.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[repr(u8)]
pub enum EmitType {
    ActionResult = 0x01,
    StateDelta   = 0x02,
    Event        = 0x03,
    Proposal     = 0x04,
    Lifecycle    = 0x05,
    /// A1 (02c) : résultat d'un appel agent_introspect — payload = INTROSPECT_PAYLOAD_LEN bytes.
    Introspect   = 0x06,
    /// A2 (02c) : rollback borné initié par l'agent — payload = [depth u8, target_seq u64 LE].
    SelfRollback      = 0x07,
    /// A3 (02c) : demande de validation émise par l'agent — payload = [risk_level u8].
    ValidationRequest  = 0x08,
    /// A3 (02c) : réponse de validation du superviseur — payload = [verdict u8].
    ValidationResponse = 0x09,
    /// Session bornée (ADR-0012) : frontière de session — payload = [session_id u64 LE, action_count u64 LE].
    SessionBoundary    = 0x0A,
    /// D5 : rollback initié par le scheduler — payload = [distance u8, target_seq u64 LE, caps_invalidated u8].
    /// Distinct de SelfRollback (0x07) pour auditer la provenance (agent vs superviseur).
    SchedulerRollback  = 0x0B,
    /// ADR-0019 : entrée dans agent_infer — payload = [prompt_hash 32B | model_id_len u8 | model_id [u8;N]
    ///   | timeout_ms_requested u32 LE | timeout_ms_effective u32 LE]. Ne fait pas avancer seq (Q7).
    InferenceRequest   = 0x0C,
    /// ADR-0019 : réponse LLM reçue — payload = [response_hash 32B | tokens_estimated u32 LE
    ///   | duration_ms u32 LE | truncated u8]. Ne fait pas avancer seq (Q7).
    InferenceResponse  = 0x0D,
    /// ADR-0019 : future d'inférence interrompue — payload = [cancel_ts_ms u64 LE | cause u8]
    ///   (cause 0x01=Rollback, 0x02=Terminate). Ne fait pas avancer seq (Q7).
    InferenceCancelled = 0x0E,
    /// ADR-0019 : erreur d'inférence (timeout, réseau, JSON malformé) —
    ///   payload = [error_code u8 | message_len u8 | message [u8;N≤255]]. Ne fait pas avancer seq (Q7).
    InferenceFailed    = 0x0F,
    /// ADR-0024 : ouverture d'un journal de compensation — payload = [agent_id 16B | expected_inference_event_id 32B].
    /// Émis par Scheduler::rollback() AVANT d'appeler cancel().
    /// Un CompensationOpen sans CompensationClose correspondant indique un crash entre 0x0E et 0x0B.
    CompensationOpen   = 0x11,
    /// ADR-0024 : fermeture d'un journal de compensation — payload = [agent_id 16B].
    /// Émis après que le snapshot a été appliqué (0x0B SchedulerRollback reçu par l'agent).
    CompensationClose  = 0x12,
    /// ADR-0015 : terminaison anormale d'un agent — payload =
    ///   [cause u8 | parent_agent_id 16B | last_action_id 32B].
    /// cause : 0x01 = ProcessFailed (process_one / SessionResume::process_one a renvoyé Err),
    ///         0x02 = ContentStoreBroken (rollback_path a renvoyé Err),
    ///         0x03 = WatchdogTrap (epoch deadline dépassée, ADR-0025),
    ///         0x04 = HostPanic (capture par run_loop si activée).
    /// Émis par `run_loop` AVANT le `Lifecycle Terminated` correspondant.
    /// `parent_agent_id` = parent direct (spawn_child) ou [0u8;16] sentinelle racine.
    /// `last_action_id` = `last_action` au moment du crash ou [0u8;32] si aucune action.
    AgentCrash         = 0x13,
    /// SEF-3 / P4 (isolation par capabilities) : accès refusé par enforcement.
    /// Payload : [agent_id 16B | cap_id u64 LE 8B | resource_len u8 | resource [u8;N≤255]
    ///            | perm_flags u8 | rate_limited u8]
    ///   perm_flags bits : 0=read, 1=write, 2=execute, 3=delegate
    ///   rate_limited : 0x00 = événement individuel, 0x01 = événement agrégé (rate-limit atteint)
    ///   Si rate_limited=0x01, le champ resource est remplacé par un compteur u32 LE
    ///   (nombre de refus dans la fenêtre) et resource_len = 4.
    /// Émis directement par la host function `agent_store_get` / `agent_store_put`
    /// via `emit_cap_denied` (hors cycle commit_barrier/emit de l'agent).
    CapabilityDenied   = 0x14,
}

impl TryFrom<u8> for EmitType {
    type Error = u8;
    fn try_from(v: u8) -> Result<Self, u8> {
        match v {
            0x01 => Ok(Self::ActionResult),
            0x02 => Ok(Self::StateDelta),
            0x03 => Ok(Self::Event),
            0x04 => Ok(Self::Proposal),
            0x05 => Ok(Self::Lifecycle),
            0x06 => Ok(Self::Introspect),
            0x07 => Ok(Self::SelfRollback),
            0x08 => Ok(Self::ValidationRequest),
            0x09 => Ok(Self::ValidationResponse),
            0x0A => Ok(Self::SessionBoundary),
            0x0B => Ok(Self::SchedulerRollback),
            0x0C => Ok(Self::InferenceRequest),
            0x0D => Ok(Self::InferenceResponse),
            0x0E => Ok(Self::InferenceCancelled),
            0x0F => Ok(Self::InferenceFailed),
            0x11 => Ok(Self::CompensationOpen),
            0x12 => Ok(Self::CompensationClose),
            0x13 => Ok(Self::AgentCrash),
            0x14 => Ok(Self::CapabilityDenied),
            other => Err(other),
        }
    }
}

/// Enveloppe d'une émission agent — sérialisée en MessagePack fixarray.
///
/// Format compact : version(u8) + emit_type(u8) + agent_id([u8;16]) +
///                  seq(u64) + ts_us(u64) + payload(bytes).
/// Le payload est opaque au niveau du log ; son interprétation dépend de emit_type.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct EmitEnvelope {
    pub version:   u8,
    pub emit_type: u8,
    pub agent_id:  AgentId,
    pub seq:       u64,
    pub ts_us:     u64,
    #[serde(with = "serde_bytes")]
    pub payload:   Vec<u8>,
}

impl EmitEnvelope {
    pub fn new(emit_type: EmitType, agent_id: AgentId, seq: u64, ts_us: u64, payload: Vec<u8>) -> Self {
        Self { version: 1, emit_type: emit_type as u8, agent_id, seq, ts_us, payload }
    }

    /// Sérialise l'enveloppe en MessagePack (tuple-style, compact).
    pub fn to_msgpack(&self) -> Vec<u8> {
        rmp_serde::to_vec(self).expect("EmitEnvelope::to_msgpack infaillible")
    }

    /// Désérialise depuis MessagePack.
    pub fn from_msgpack(bytes: &[u8]) -> Result<Self, rmp_serde::decode::Error> {
        rmp_serde::from_slice(bytes)
    }
}

/// Identifiant content-addressed d'une action (SHA-256 de l'entrée sérialisée).
/// C'est la clé RocksDB — ne figure PAS dans la valeur stockée.
pub type ActionId = [u8; 32];

/// Identifiant opaque d'un agent (128 bits).
pub type AgentId = [u8; 16];

/// Hash SHA-256 de l'état mémoire à un instant donné.
pub type StateHash = [u8; 32];

/// Entrée Layer 0 — valeur stockée dans RocksDB.
///
/// Taille typique en bincode (sans payload) :
///   - agent_id (16) + ts_ms (8) + parent_ids (4 + N×32) + hash_before (32) + hash_after (32)
///   → ~108 bytes pour un parent unique.
///
/// Avec emit_payload : + 1 (Option tag) + taille de l'enveloppe MessagePack.
/// Pour une émission action_result courte (~50 bytes payload) : ~190 bytes total.
///
/// Support DAG complet : parent_ids peut contenir 0 (racine), 1 (action séquentielle)
/// ou plusieurs parents (nœud de merge — cf. ADR-0003).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LogEntry {
    /// Agent émetteur.
    pub agent_id: AgentId,
    /// Timestamp de création en millisecondes Unix (compatible UUIDv7).
    pub ts_ms: u64,
    /// Parents causaux dans le DAG.
    pub parent_ids: Vec<ActionId>,
    /// Hash de l'état mémoire AVANT cette action.
    pub hash_before: StateHash,
    /// Hash de l'état mémoire APRÈS cette action.
    pub hash_after: StateHash,
    /// Payload de l'émission (MessagePack-sérialisé EmitEnvelope).
    /// None = commit_barrier pur sans emit (checkpoint sans publication d'effet).
    pub emit_payload: Option<Vec<u8>>,
}

impl LogEntry {
    /// Calcule l'action_id content-addressed de cette entrée.
    /// Déterministe et idempotent : même contenu → même id.
    pub fn action_id(&self) -> ActionId {
        let encoded = bincode::serialize(self).expect("sérialisation bincode infaillible");
        let mut h = Sha256::new();
        h.update(&encoded);
        h.finalize().into()
    }
}

pub struct CausalLog {
    db: DB,
}

impl CausalLog {
    /// Ouvre (ou crée) le log dans `path`.
    ///
    /// CF `default` — optimisée pour les point lookups P3 (bloom 10 bits/clé, cache partagé).
    /// CF `agent_ts` — optimisée pour les range queries P3b (prefix extractor 16B, bloom sur préfixes).
    /// `shared_cache` : cache LRU partagé entre CausalLog et ContentStore (P7). Si None,
    ///   un cache local de 256 MB est créé. Passer `Some(cache.clone())` pour partager avec ContentStore.
    pub fn open(path: &std::path::Path, shared_cache: Option<Cache>) -> Result<Self, LogError> {
        let cache = shared_cache.unwrap_or_else(|| Cache::new_lru_cache(256 * 1024 * 1024));

        // CF default : point lookups (P3 stricte)
        // P6 : cache_index_and_filter_blocks_with_high_priority est le défaut C++ (=true) ;
        //      rocksdb 0.22 n'expose pas le setter.
        let mut block_opts = BlockBasedOptions::default();
        block_opts.set_bloom_filter(10.0, false);
        block_opts.set_block_cache(&cache);
        block_opts.set_cache_index_and_filter_blocks(true);
        block_opts.set_pin_l0_filter_and_index_blocks_in_cache(true);

        let mut default_opts = Options::default();
        default_opts.set_block_based_table_factory(&block_opts);
        default_opts.set_write_buffer_size(64 * 1024 * 1024);
        default_opts.set_max_write_buffer_number(2);
        // min_write_buffer_number_to_merge=1 (défaut) : chaque memtable → un SST L0 distinct.
        // Correct pour notre workload append-only séquentiel — pas de fusion avant flush.
        default_opts.set_max_bytes_for_level_base(256 * 1024 * 1024);
        // target_file_size_base = max_bytes_for_level_base / 10 (recommandation RocksDB).
        default_opts.set_target_file_size_base(32 * 1024 * 1024);
        default_opts.set_max_background_jobs(4);
        // Seuils write-stall L0 : slowdown=20, stop=36 (défauts RocksDB) — acceptés.
        // Avec max_write_buffer_number=2 le flush arrive avant tout stall en régime normal.
        default_opts.set_compression_per_level(&[
            DBCompressionType::None, DBCompressionType::None,
            DBCompressionType::Lz4,  DBCompressionType::Lz4, DBCompressionType::Lz4,
            DBCompressionType::Zstd, DBCompressionType::Zstd,
        ]);

        // CF agent_ts : range queries par (agent_id, ts_ms) — P3b
        // Clé = agent_id(16B) || ts_ms_BE(8B) || action_id(32B)
        // Prefix extractor 16B → bloom filter sur les préfixes agent_id (P5).
        let mut agent_ts_block_opts = BlockBasedOptions::default();
        agent_ts_block_opts.set_bloom_filter(10.0, false);
        agent_ts_block_opts.set_block_cache(&cache);
        agent_ts_block_opts.set_cache_index_and_filter_blocks(true);
        agent_ts_block_opts.set_pin_l0_filter_and_index_blocks_in_cache(true);

        let mut agent_ts_opts = Options::default();
        agent_ts_opts.set_block_based_table_factory(&agent_ts_block_opts);
        agent_ts_opts.set_prefix_extractor(SliceTransform::create_fixed_prefix(16));
        agent_ts_opts.set_compression_per_level(&[
            DBCompressionType::None, DBCompressionType::None,
            DBCompressionType::Lz4,  DBCompressionType::Lz4, DBCompressionType::Lz4,
            DBCompressionType::Zstd, DBCompressionType::Zstd,
        ]);

        let mut db_opts = Options::default();
        db_opts.create_if_missing(true);
        db_opts.create_missing_column_families(true);
        db_opts.set_bytes_per_sync(1_048_576);
        db_opts.set_wal_bytes_per_sync(1_048_576);

        let cfs = vec![
            ColumnFamilyDescriptor::new("default", default_opts),
            ColumnFamilyDescriptor::new("agent_ts", agent_ts_opts),
        ];
        let db = DB::open_cf_descriptors(&db_opts, path, cfs)?;
        Ok(Self { db })
    }

    /// Ouvre un log **existant** en lecture/écriture, `create_if_missing(false)`.
    ///
    /// Destiné aux **vérificateurs tiers** (cold reopen, process distinct de l'écrivain) :
    /// échoue bruyamment si le path n'existe pas, au lieu de créer une DB vide — ce qui
    /// produirait un faux négatif silencieux « 0 entrée, aucune corruption » (piège RocksDB).
    /// Ouverture en read-write (et non read-only) pour garantir le **replay du WAL** : un
    /// vérificateur voit ainsi les écritures de l'écrivain même non flushées en SST.
    pub fn open_existing(path: &std::path::Path, shared_cache: Option<Cache>) -> Result<Self, LogError> {
        let cache = shared_cache.unwrap_or_else(|| Cache::new_lru_cache(256 * 1024 * 1024));

        let mut block_opts = BlockBasedOptions::default();
        block_opts.set_bloom_filter(10.0, false);
        block_opts.set_block_cache(&cache);
        block_opts.set_cache_index_and_filter_blocks(true);
        block_opts.set_pin_l0_filter_and_index_blocks_in_cache(true);

        let mut default_opts = Options::default();
        default_opts.set_block_based_table_factory(&block_opts);

        let mut agent_ts_block_opts = BlockBasedOptions::default();
        agent_ts_block_opts.set_bloom_filter(10.0, false);
        agent_ts_block_opts.set_block_cache(&cache);

        let mut agent_ts_opts = Options::default();
        agent_ts_opts.set_block_based_table_factory(&agent_ts_block_opts);
        agent_ts_opts.set_prefix_extractor(SliceTransform::create_fixed_prefix(16));

        let mut db_opts = Options::default();
        db_opts.create_if_missing(false);
        db_opts.create_missing_column_families(false);

        let cfs = vec![
            ColumnFamilyDescriptor::new("default", default_opts),
            ColumnFamilyDescriptor::new("agent_ts", agent_ts_opts),
        ];
        let db = DB::open_cf_descriptors(&db_opts, path, cfs)?;
        Ok(Self { db })
    }

    /// Itère **toutes les entrées de la CF `default`** en octets bruts `(clé, valeur)`.
    ///
    /// Mécanisme pur (aucune politique d'intégrité ici) destiné à un auditeur tiers :
    /// la vérification `clé == SHA256(valeur)` se fait côté appelant, sur les octets bruts,
    /// **sans désérialiser** (le test le plus robuste — indépendant de toute re-sérialisation).
    /// Contrairement à `entries_by_agent`, ne fait **pas** `.flatten()` : les erreurs I/O de
    /// l'itérateur sont propagées (un vérificateur d'intégrité ne doit jamais les avaler).
    pub fn iter_default_raw(
        &self,
    ) -> impl Iterator<Item = Result<(Box<[u8]>, Box<[u8]>), LogError>> + '_ {
        self.db
            .iterator(IteratorMode::Start)
            .map(|item| item.map_err(LogError::Rocks))
    }

    /// **Test/démo uniquement.** Corrompt la *valeur* d'une entrée sous la *même clé*
    /// (flip de l'octet `byte_idx % len`), cassant l'invariant content-addressing
    /// `clé == SHA256(valeur)`. Flush immédiat pour durabilité cross-process : un
    /// vérificateur lancé ensuite, dans un autre process, voit la corruption.
    ///
    /// Corruption en couche **logique** (`db.put`, checksum de bloc RocksDB régénéré
    /// valide) : RocksDB rend la valeur mutée sans erreur — seul le recalcul SHA256
    /// applicatif la détecte. Retourne `Ok(false)` si la clé n'existe pas.
    #[cfg(feature = "test-utils")]
    pub fn corrupt_value_at(&self, id: &ActionId, byte_idx: usize) -> Result<bool, LogError> {
        let value = match self.db.get(id)? {
            Some(v) => v,
            None => return Ok(false),
        };
        if value.is_empty() {
            return Ok(false);
        }
        let mut v = value;
        let i = byte_idx % v.len();
        v[i] ^= 0xFF;
        self.db.put(id, &v)?;
        self.db.flush()?;
        Ok(true)
    }

    /// Variante de `open` pour T5-ter Mode A : désactive la compaction automatique.
    ///
    /// Utiliser `compact_all()` manuellement avant la mesure pour partir d'un état stable.
    /// Donne le p99 "intrinsèque" (sans stall de compaction) — P3b-intrinsèque (ADR-0032 §D4).
    pub fn open_no_autocompact(path: &std::path::Path, shared_cache: Option<Cache>) -> Result<Self, LogError> {
        let cache = shared_cache.unwrap_or_else(|| Cache::new_lru_cache(256 * 1024 * 1024));

        let mut block_opts = BlockBasedOptions::default();
        block_opts.set_bloom_filter(10.0, false);
        block_opts.set_block_cache(&cache);
        block_opts.set_cache_index_and_filter_blocks(true);
        block_opts.set_pin_l0_filter_and_index_blocks_in_cache(true);

        let mut default_opts = Options::default();
        default_opts.set_block_based_table_factory(&block_opts);
        default_opts.set_write_buffer_size(64 * 1024 * 1024);
        default_opts.set_max_write_buffer_number(2);
        // min_write_buffer_number_to_merge=1 (défaut) : chaque memtable → un SST L0 distinct.
        default_opts.set_max_bytes_for_level_base(256 * 1024 * 1024);
        default_opts.set_target_file_size_base(32 * 1024 * 1024);
        default_opts.set_max_background_jobs(4);
        // Seuils write-stall L0 : slowdown=20, stop=36 (défauts RocksDB) — acceptés.
        default_opts.set_compression_per_level(&[
            DBCompressionType::None, DBCompressionType::None,
            DBCompressionType::Lz4,  DBCompressionType::Lz4, DBCompressionType::Lz4,
            DBCompressionType::Zstd, DBCompressionType::Zstd,
        ]);
        default_opts.set_disable_auto_compactions(true);

        let mut agent_ts_block_opts = BlockBasedOptions::default();
        agent_ts_block_opts.set_bloom_filter(10.0, false);
        agent_ts_block_opts.set_block_cache(&cache);
        agent_ts_block_opts.set_cache_index_and_filter_blocks(true);
        agent_ts_block_opts.set_pin_l0_filter_and_index_blocks_in_cache(true);

        let mut agent_ts_opts = Options::default();
        agent_ts_opts.set_block_based_table_factory(&agent_ts_block_opts);
        agent_ts_opts.set_prefix_extractor(SliceTransform::create_fixed_prefix(16));
        agent_ts_opts.set_compression_per_level(&[
            DBCompressionType::None, DBCompressionType::None,
            DBCompressionType::Lz4,  DBCompressionType::Lz4, DBCompressionType::Lz4,
            DBCompressionType::Zstd, DBCompressionType::Zstd,
        ]);
        agent_ts_opts.set_disable_auto_compactions(true);

        let mut db_opts = Options::default();
        db_opts.create_if_missing(true);
        db_opts.create_missing_column_families(true);
        db_opts.set_bytes_per_sync(1_048_576);
        db_opts.set_wal_bytes_per_sync(1_048_576);

        let cfs = vec![
            ColumnFamilyDescriptor::new("default", default_opts),
            ColumnFamilyDescriptor::new("agent_ts", agent_ts_opts),
        ];
        let db = DB::open_cf_descriptors(&db_opts, path, cfs)?;
        Ok(Self { db })
    }

    /// Compacte toutes les column families (blocant). À appeler après population en Mode A.
    pub fn compact_all(&self) {
        self.db.compact_range(None::<&[u8]>, None::<&[u8]>);
        if let Some(cf) = self.db.cf_handle("agent_ts") {
            self.db.compact_range_cf(&cf, None::<&[u8]>, None::<&[u8]>);
        }
    }

    /// Lit une propriété RocksDB entière pour une column family nommée.
    ///
    /// Propriétés utiles pour T5-ter Mode B :
    ///   "rocksdb.num-running-compactions"  — > 0 si une compaction est en cours
    ///   "rocksdb.num-files-at-level0"      — monte avant stall, chute après compaction
    ///   "rocksdb.is-write-stalled"         — 1 si les writes sont throttlés
    pub fn get_rocksdb_int_property(&self, cf_name: &str, prop: &str) -> Option<u64> {
        let cf = self.db.cf_handle(cf_name)?;
        self.db.property_int_value_cf(&cf, prop).ok().flatten()
    }

    /// Mémoire totale des memtables sur toutes les column families (octets).
    /// À soustraire du RSS conjointement avec ContentStore::total_memtable_bytes()
    /// pour obtenir la croissance hors-LSM (critère ADR-0033).
    pub fn total_memtable_bytes(&self) -> u64 {
        ["default", "agent_ts"]
            .iter()
            .filter_map(|cf| {
                self.get_rocksdb_int_property(cf, "rocksdb.cur-size-all-mem-tables")
            })
            .sum()
    }

    /// Mémoire utilisée par le block cache sur toutes les column families (octets).
    /// CF `default` : cache explicite 256 MB. CF `agent_ts` : cache par défaut 8 MB.
    /// À inclure dans rss_adj = RSS − memtable − block_cache (TODO P3B).
    pub fn block_cache_usage_bytes(&self) -> u64 {
        ["default", "agent_ts"]
            .iter()
            .filter_map(|cf| {
                self.get_rocksdb_int_property(cf, "rocksdb.block-cache-usage")
            })
            .sum()
    }

    /// Insère une entrée dans le log.
    /// Écrit atomiquement dans CF `default` (clé = action_id) et CF `agent_ts`
    /// (clé = agent_id || ts_ms_BE || action_id) via WriteBatch.
    /// Idempotent : deux appels avec la même entrée n'insèrent qu'une fois.
    ///
    /// **Durabilité :** cette méthode n'active pas `WriteOptions::set_sync(true)`.
    /// Le WriteAheadLog RocksDB est écrit sur le fd OS mais le fsync n'est pas forcé.
    /// En cas de crash brutal du noyau ou du matériel, les N dernières entrées peuvent
    /// être perdues (typiquement N ≤ quelques milliers selon `bytes_per_sync`).
    /// Pour la garantie de durabilité requise par P3b et par ADR-0024 (compensation
    /// atomicity), utiliser `append_durable()`.
    pub fn append(&self, entry: &LogEntry) -> Result<ActionId, LogError> {
        let id = entry.action_id();
        let encoded = bincode::serialize(entry).expect("sérialisation bincode infaillible");

        let cf_agent_ts = self.db.cf_handle("agent_ts").expect("CF agent_ts");
        let index_key = Self::agent_ts_key(&entry.agent_id, entry.ts_ms, &id);

        let mut batch = WriteBatch::default();
        batch.put(id, encoded);
        batch.put_cf(&cf_agent_ts, index_key, []);
        self.db.write(batch)?;
        Ok(id)
    }

    /// Insère une entrée dans le log avec **durabilité fsync garantie**.
    ///
    /// Identique à `append()` mais avec `WriteOptions::set_sync(true)` : la primitive
    /// retourne uniquement après que le WAL RocksDB a été fsynced sur le périphérique
    /// de stockage. C'est le contrat requis par P3b (cf. `spec/02-properties.md §P3b`) :
    /// « pour toute action émise via `CausalLog::append` avec durabilité garantie
    /// (`WriteBatch` + WAL fsync) ».
    ///
    /// Coût observable typique sur NVMe :
    ///   - NVMe avec PLP (data center) : 0,1–0,5 ms par fsync
    ///   - NVMe consumer (sans PLP, ex. WD SN530) : 1–15 ms par fsync
    ///   - SSD SATA : 5–20 ms par fsync
    ///
    /// La borne P3b (p99 ≤ 20 ms) est conçue pour absorber ce coût sur hardware
    /// commodité, avec marge. Mesurée par T5-bis.
    pub fn append_durable(&self, entry: &LogEntry) -> Result<ActionId, LogError> {
        let id = entry.action_id();
        let encoded = bincode::serialize(entry).expect("sérialisation bincode infaillible");

        let cf_agent_ts = self.db.cf_handle("agent_ts").expect("CF agent_ts");
        let index_key = Self::agent_ts_key(&entry.agent_id, entry.ts_ms, &id);

        let mut batch = WriteBatch::default();
        batch.put(id, encoded);
        batch.put_cf(&cf_agent_ts, index_key, []);

        let mut write_opts = WriteOptions::default();
        write_opts.set_sync(true);
        self.db.write_opt(batch, &write_opts)?;
        Ok(id)
    }

    /// Construit la clé de l'index `agent_ts` : agent_id(16B) || ts_ms_BE(8B) || action_id(32B).
    fn agent_ts_key(agent_id: &AgentId, ts_ms: u64, action_id: &ActionId) -> [u8; 56] {
        let mut key = [0u8; 56];
        key[..16].copy_from_slice(agent_id);
        key[16..24].copy_from_slice(&ts_ms.to_be_bytes());
        key[24..].copy_from_slice(action_id);
        key
    }

    /// Lookup O(1) par action_id. Retourne None si l'action est inconnue.
    pub fn get(&self, id: &ActionId) -> Result<Option<LogEntry>, LogError> {
        match self.db.get(id)? {
            None => Ok(None),
            Some(bytes) => Ok(Some(bincode::deserialize(&bytes)?)),
        }
    }

    /// Range query P3b : retourne les action_ids de `agent_id` dans l'ordre temporel.
    /// Utilise le scan de préfixe O(k) sur la CF `agent_ts` (index secondaire).
    /// `from_ts_ms` et `to_ts_ms` sont inclusifs ; None = pas de borne.
    pub fn query_by_agent_range(
        &self,
        agent_id: &AgentId,
        from_ts_ms: Option<u64>,
        to_ts_ms: Option<u64>,
    ) -> Result<Vec<ActionId>, LogError> {
        let cf = self.db.cf_handle("agent_ts").expect("CF agent_ts");
        let ts_start = from_ts_ms.unwrap_or(0);
        let mut seek_key = [0u8; 24]; // agent_id(16) + ts_ms_BE(8)
        seek_key[..16].copy_from_slice(agent_id);
        seek_key[16..].copy_from_slice(&ts_start.to_be_bytes());

        let iter = self.db.iterator_cf(&cf, IteratorMode::From(&seek_key, Direction::Forward));
        let mut results = Vec::new();
        for item in iter {
            let (key, _) = item.map_err(LogError::Rocks)?;
            if key.len() != 56 || &key[..16] != agent_id { break; }
            let ts_ms = u64::from_be_bytes(key[16..24].try_into().unwrap());
            if let Some(to) = to_ts_ms { if ts_ms > to { break; } }
            let mut action_id = [0u8; 32];
            action_id.copy_from_slice(&key[24..]);
            results.push(action_id);
        }
        Ok(results)
    }

    /// Retourne toutes les entrées dont l'agent_id correspond à `agent_id`.
    /// Scan linéaire O(N) — **tests et diagnostics uniquement**.
    /// En production, utiliser `query_by_agent_range` (index secondaire CF `agent_ts`).
    #[cfg(any(test, feature = "test-utils"))]
    pub fn entries_by_agent(&self, agent_id: &AgentId) -> Vec<(ActionId, LogEntry)> {
        let mut result = Vec::new();
        for item in self.db.iterator(rocksdb::IteratorMode::Start).flatten() {
            let (key, value) = item;
            if key.len() != 32 { continue; }
            if let Ok(entry) = bincode::deserialize::<LogEntry>(&value) {
                if &entry.agent_id == agent_id {
                    let mut id = [0u8; 32];
                    id.copy_from_slice(&key);
                    result.push((id, entry));
                }
            }
        }
        result
    }

    /// Peuple le log avec `n` entrées synthétiques en chaîne linéaire.
    ///
    /// Écrit par batches de `BATCH_SIZE` pour maximiser le débit LSM (réduit les flushes
    /// memtable de N à N/BATCH_SIZE). Pour N=10⁸ et BATCH_SIZE=10_000 : 10_000 flushes.
    ///
    /// Retourne un échantillon uniforme de `sample_size` action_ids pour les benchmarks.
    pub fn populate_synthetic(
        &self,
        n: u64,
        sample_size: usize,
    ) -> Result<Vec<ActionId>, LogError> {
        let agent_id = [0xAAu8; 16];
        let cf_agent_ts = self.db.cf_handle("agent_ts").expect("CF agent_ts");
        let mut prev: Option<ActionId> = None;
        let mut samples = Vec::with_capacity(sample_size);
        let step = (n / sample_size.max(1) as u64).max(1);
        let mut batch = WriteBatch::default();
        const BATCH_SIZE: u64 = 10_000;

        for i in 0..n {
            let entry = LogEntry {
                agent_id,
                ts_ms: i,
                parent_ids: match prev {
                    None => vec![],
                    Some(id) => vec![id],
                },
                hash_before: [0xAAu8; 32],
                hash_after: [0xBBu8; 32],
                emit_payload: None,
            };
            let id = entry.action_id();
            let encoded = bincode::serialize(&entry).expect("sérialisation bincode infaillible");
            let index_key = Self::agent_ts_key(&agent_id, i, &id);
            batch.put(id, encoded);
            batch.put_cf(&cf_agent_ts, index_key, []);
            prev = Some(id);

            if sample_size > 0 && i % step == 0 {
                samples.push(id);
            }

            if (i + 1) % BATCH_SIZE == 0 {
                self.db.write(std::mem::take(&mut batch))?;
                batch = WriteBatch::default();
            }
        }
        if !batch.is_empty() {
            self.db.write(batch)?;
        }

        Ok(samples)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_log() -> (CausalLog, TempDir) {
        let dir = TempDir::new().unwrap();
        let log = CausalLog::open(dir.path(), None).unwrap();
        (log, dir)
    }

    fn entry(agent_id: AgentId, ts_ms: u64, prev: Option<ActionId>) -> LogEntry {
        LogEntry {
            agent_id,
            ts_ms,
            parent_ids: prev.into_iter().collect(),
            hash_before: [0u8; 32],
            hash_after: [0u8; 32],
            emit_payload: None,
        }
    }

    /// P3b — query_by_agent_range retourne les action_ids dans l'ordre temporel.
    #[test]
    fn query_range_ordered_by_ts() {
        let (log, _dir) = make_log();
        let agent_a = [0x01u8; 16];
        let agent_b = [0x02u8; 16];

        let id1 = log.append(&entry(agent_a, 100, None)).unwrap();
        let id2 = log.append(&entry(agent_a, 200, Some(id1))).unwrap();
        let id3 = log.append(&entry(agent_a, 300, Some(id2))).unwrap();
        let _   = log.append(&entry(agent_b, 150, None)).unwrap(); // autre agent

        let results = log.query_by_agent_range(&agent_a, None, None).unwrap();
        assert_eq!(results, vec![id1, id2, id3], "ordre temporel respecté");
    }

    /// P3b — borne inférieure et supérieure filtrées correctement.
    #[test]
    fn query_range_with_bounds() {
        let (log, _dir) = make_log();
        let agent = [0x03u8; 16];

        let id1 = log.append(&entry(agent, 100, None)).unwrap();
        let id2 = log.append(&entry(agent, 200, Some(id1))).unwrap();
        let id3 = log.append(&entry(agent, 300, Some(id2))).unwrap();
        let _   = log.append(&entry(agent, 400, Some(id3))).unwrap();

        let results = log.query_by_agent_range(&agent, Some(150), Some(300)).unwrap();
        assert_eq!(results, vec![id2, id3], "seuls ts 200 et 300 dans [150, 300]");
    }

    /// P3b — append_durable produit le même action_id que append.
    /// La sémantique de durabilité ne doit pas modifier le contenu de l'entrée,
    /// uniquement le moment où la fonction retourne (post-fsync WAL).
    #[test]
    fn append_durable_same_id_as_append() {
        let (log_a, _dir_a) = make_log();
        let (log_b, _dir_b) = make_log();
        let agent = [0x05u8; 16];

        let id_a = log_a.append(&entry(agent, 7, None)).unwrap();
        let id_b = log_b.append_durable(&entry(agent, 7, None)).unwrap();

        assert_eq!(id_a, id_b, "action_id content-addressed indépendant du mode de durabilité");

        // Lookup post-durable doit récupérer l'entrée
        let fetched = log_b.get(&id_b).unwrap().unwrap();
        assert_eq!(fetched.ts_ms, 7);
    }

    /// Atomicité — un append échoué ne laisse pas de clé orpheline dans agent_ts.
    /// (Test structurel : vérifie juste que append réussit et les deux CFs sont cohérentes.)
    #[test]
    fn append_coherence_default_and_agent_ts() {
        let (log, _dir) = make_log();
        let agent = [0x04u8; 16];

        let id = log.append(&entry(agent, 42, None)).unwrap();

        // get() depuis CF default
        let fetched = log.get(&id).unwrap().unwrap();
        assert_eq!(fetched.ts_ms, 42);

        // query_by_agent_range depuis CF agent_ts
        let ids = log.query_by_agent_range(&agent, None, None).unwrap();
        assert_eq!(ids, vec![id], "CF agent_ts cohérente avec CF default");
    }
}
