// Harness de durabilité — oracle I-CSR (spec/10 §4, ADR-0051 §Amendement).
//
// Invariant I-CSR : ∀ log_entry ∈ journal : log_entry.snapshot_hash ∈ store
// (tout SnapshotHeader référencé par le log causal existe dans le ContentStore).
//
// Asymétrie (spec/10 §4.2, ADR-0051 §D1) :
//   - Orphelin (snapshot dans le store non référencé par le log) : toléré.
//   - Référence pendante (log_entry → snapshot absent) : violation I-CSR.
//
// Ce module fournit :
//   1. write_commits()  — phase d'écriture (store + log directs, sans acteur WASM)
//   2. verify_icsr()    — phase de vérification post-reopen
//   3. IcsrWitness      — fichier témoin JSON partagé entre les deux binaires
//
// Modes de coupure paramétrés (icsr-writer --cut-mode) :
//   drop   — drop propre des Arcs (simule arrêt coopératif, niveau D2 confirmé)
//   exit   — process::exit(1) (simule SIGKILL, régime α ADR-0027)
//   [stub] drop_caches — requiert root (déclencheur : accès root/VM, spec/10 §6)
//   [stub] kill_qemu   — requiert board réel (déclencheur : hardware, spec/10 §6)

use os_poc_causal_log::{AgentId, CausalLog, LogEntry};
use os_poc_store::{ContentStore, SnapshotHeader};
use serde::{Deserialize, Serialize};
use std::path::Path;

// ── Types publics ──────────────────────────────────────────────────────────────

/// Enregistrement d'un commit dans le témoin.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IcsrCommit {
    /// Numéro de séquence de l'action dans la chaîne agent.
    pub seq: u64,
    /// Clé RocksDB du log (CF default) : SHA-256(LogEntry sérialisé).
    pub action_id_hex: String,
    /// Clé RocksDB du store (CF headers) : SHA-256(SnapshotHeader sérialisé).
    pub snapshot_id_hex: String,
    /// SHA-256 du bloc de données (= LogEntry.hash_after = SnapshotHeader.data_hash).
    pub data_hash_hex: String,
}

/// Fichier témoin JSON — partagé entre icsr-writer et icsr-verifier.
#[derive(Debug, Serialize, Deserialize)]
pub struct IcsrWitness {
    pub agent_id_hex: String,
    pub n_commits: usize,
    /// Mode de coupure utilisé lors de l'écriture.
    pub cut_mode: String,
    pub commits: Vec<IcsrCommit>,
}

impl IcsrWitness {
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        std::fs::write(path, json)
    }

    pub fn load(path: &Path) -> std::io::Result<Self> {
        let json = std::fs::read_to_string(path)?;
        serde_json::from_str(&json)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }
}

/// Nature de la violation détectée.
#[derive(Debug, Serialize, Deserialize)]
pub enum IcsrViolationKind {
    /// log_entry absent après reopen (perte d'écriture — information, pas violation I-CSR stricte).
    LogEntryMissing,
    /// log_entry présent mais snapshot absent — violation I-CSR (référence pendante).
    SnapshotMissing,
    /// log_entry présent mais bloc de données absent (hash_after non trouvé dans blocks CF).
    DataBlockMissing,
}

/// Violation individuelle détectée par verify_icsr.
#[derive(Debug, Serialize, Deserialize)]
pub struct IcsrViolation {
    pub seq: u64,
    pub action_id_hex: String,
    pub snapshot_id_hex: String,
    pub kind: IcsrViolationKind,
}

/// Résultat de la vérification I-CSR.
#[derive(Debug, Serialize, Deserialize)]
pub struct IcsrResult {
    /// Nombre de commits vérifiés.
    pub checked: usize,
    /// Commits dont le log_entry est absent (perte d'écriture non-fsync — admis).
    pub log_missing: usize,
    /// Commits dont le snapshot est absent malgré un log_entry présent (I-CSR violé).
    pub snapshot_missing: usize,
    /// Commits dont le bloc de données est absent malgré un log_entry présent.
    pub data_block_missing: usize,
    pub violations: Vec<IcsrViolation>,
    /// true ssi snapshot_missing == 0 (I-CSR satisfait).
    pub icsr_ok: bool,
}

// ── Phase d'écriture ───────────────────────────────────────────────────────────

/// Écrit `n` commits synthétiques (put_block + put_snapshot + append log_entry)
/// sans acteur WASM ni runtime Tokio.
///
/// Chaque commit :
///   block_data  = [0xAB] × block_size
///   data_hash   = SHA-256(block_data)
///   snapshot    = SnapshotHeader { data_hash, parent, seq, ts_us=seq*1000 }
///   snap_id     = SHA-256(serialize(snapshot))
///   log_entry   = LogEntry { agent_id, ts_ms=seq, parent_ids, hash_before, hash_after=data_hash }
///   action_id   = SHA-256(serialize(log_entry))
///
/// L'état chaîné est simulé : hash_before[k] = hash_after[k-1].
pub fn write_commits(
    store: &ContentStore,
    log: &CausalLog,
    agent_id: AgentId,
    n: usize,
    block_size: usize,
) -> Result<IcsrWitness, String> {
    let data = vec![0xABu8; block_size];
    let data_h = store.put_block(&data).map_err(|e| format!("put_block: {e}"))?;

    let mut parent_snap: Option<[u8; 32]> = None;
    let mut prev_action: Option<[u8; 32]> = None;
    let mut hash_before = [0u8; 32]; // initial zero state

    let mut commits = Vec::with_capacity(n);

    for seq in 0u64..n as u64 {
        // 1. SnapshotHeader
        let header = SnapshotHeader {
            data_hash: data_h,
            parent: parent_snap,
            seq,
            ts_us: seq * 1_000,
        };
        let snap_id = store
            .put_snapshot(header.clone())
            .map_err(|e| format!("put_snapshot seq={seq}: {e}"))?;

        // 2. LogEntry
        let parent_ids = prev_action.map(|id| vec![id]).unwrap_or_default();
        let entry = LogEntry {
            agent_id,
            ts_ms: seq,
            parent_ids,
            hash_before,
            hash_after: data_h,
            emit_payload: None,
        };
        let action_id = log
            .append(&entry)
            .map_err(|e| format!("append seq={seq}: {e}"))?;

        commits.push(IcsrCommit {
            seq,
            action_id_hex: hex_encode(&action_id),
            snapshot_id_hex: hex_encode(&snap_id),
            data_hash_hex: hex_encode(&data_h),
        });

        parent_snap = Some(snap_id);
        prev_action = Some(action_id);
        hash_before = data_h;
    }

    Ok(IcsrWitness {
        agent_id_hex: hex_encode(&agent_id),
        n_commits: n,
        cut_mode: String::new(), // filled by the caller
        commits,
    })
}

// ── Phase de vérification ─────────────────────────────────────────────────────

/// Vérifie l'invariant I-CSR sur les commits du témoin.
///
/// Pour chaque commit :
///   1. log.get(action_id) — si absent : LogEntryMissing (info, admis sous régime no-force)
///   2. store.has_snapshot(snap_id) — si log présent mais snapshot absent : SnapshotMissing
///      → VIOLATION I-CSR (référence pendante, spec/10 §4.2)
///   3. store.get_block(data_hash) — si log présent mais bloc absent : DataBlockMissing
pub fn verify_icsr(
    store: &ContentStore,
    log: &CausalLog,
    witness: &IcsrWitness,
) -> IcsrResult {
    let mut violations = Vec::new();
    let mut log_missing = 0usize;
    let mut snapshot_missing = 0usize;
    let mut data_block_missing = 0usize;

    for commit in &witness.commits {
        let action_id = match hex_decode_32(&commit.action_id_hex) {
            Some(id) => id,
            None => {
                eprintln!("witness: action_id_hex invalide seq={}", commit.seq);
                continue;
            }
        };
        let snap_id = match hex_decode_32(&commit.snapshot_id_hex) {
            Some(id) => id,
            None => {
                eprintln!("witness: snapshot_id_hex invalide seq={}", commit.seq);
                continue;
            }
        };
        let data_hash = match hex_decode_32(&commit.data_hash_hex) {
            Some(id) => id,
            None => {
                eprintln!("witness: data_hash_hex invalide seq={}", commit.seq);
                continue;
            }
        };

        // 1. Log entry présent ?
        let log_present = match log.get(&action_id) {
            Ok(Some(_)) => true,
            Ok(None) => {
                log_missing += 1;
                violations.push(IcsrViolation {
                    seq: commit.seq,
                    action_id_hex: commit.action_id_hex.clone(),
                    snapshot_id_hex: commit.snapshot_id_hex.clone(),
                    kind: IcsrViolationKind::LogEntryMissing,
                });
                false
            }
            Err(e) => {
                eprintln!("log.get seq={} err: {e}", commit.seq);
                false
            }
        };

        if log_present {
            // 2. Snapshot présent ? (I-CSR core)
            match store.has_snapshot(&snap_id) {
                Ok(true) => {}
                Ok(false) => {
                    snapshot_missing += 1;
                    violations.push(IcsrViolation {
                        seq: commit.seq,
                        action_id_hex: commit.action_id_hex.clone(),
                        snapshot_id_hex: commit.snapshot_id_hex.clone(),
                        kind: IcsrViolationKind::SnapshotMissing,
                    });
                }
                Err(e) => eprintln!("store.has_snapshot seq={} err: {e}", commit.seq),
            }

            // 3. Bloc de données présent ?
            match store.get_block(&data_hash) {
                Ok(None) => {
                    data_block_missing += 1;
                    violations.push(IcsrViolation {
                        seq: commit.seq,
                        action_id_hex: commit.action_id_hex.clone(),
                        snapshot_id_hex: commit.snapshot_id_hex.clone(),
                        kind: IcsrViolationKind::DataBlockMissing,
                    });
                }
                Err(e) => eprintln!("store.get_block seq={} err: {e}", commit.seq),
                Ok(Some(_)) => {}
            }
        }
    }

    let icsr_ok = snapshot_missing == 0 && data_block_missing == 0;

    IcsrResult {
        checked: witness.commits.len(),
        log_missing,
        snapshot_missing,
        data_block_missing,
        violations,
        icsr_ok,
    }
}

// ── Types P6 concurrent (S15) ─────────────────────────────────────────────────

/// Témoin d'un agent dans le harness S15.
#[derive(Debug, Serialize, Deserialize)]
pub struct AgentWitness {
    pub agent_id_hex: String,
    /// Commits ackés par cet agent, dans l'ordre d'émission (seq croissant).
    pub acked_commits: Vec<IcsrCommit>,
}

/// Témoin global du harness S15 (N agents concurrents).
#[derive(Debug, Serialize, Deserialize)]
pub struct ConcurrentWitness {
    pub n_agents: usize,
    pub kill_threshold: usize,
    pub agents: Vec<AgentWitness>,
}

impl ConcurrentWitness {
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        std::fs::write(path, json)
    }

    pub fn load(path: &Path) -> std::io::Result<Self> {
        let json = std::fs::read_to_string(path)?;
        serde_json::from_str(&json)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }
}

/// Nature d'une violation P6 concurrent.
#[derive(Debug, Serialize, Deserialize)]
pub enum P6ViolationKind {
    /// Trou : commit at `later_seq` visible, mais commit at `first_missing_seq` absent.
    Gap { first_missing_seq: u64, later_present_seq: u64 },
    /// Référence pendante : log_entry présent mais snapshot absent (I-CSR).
    SnapshotMissing,
    /// Bloc de données absent malgré log_entry présent.
    DataBlockMissing,
    /// parent_id référence une action absente du log reconstruit.
    ParentIdMissing { parent_hex: String },
}

/// Violation individuelle détectée par verify_p6_concurrent.
#[derive(Debug, Serialize, Deserialize)]
pub struct P6Violation {
    pub agent_id_hex: String,
    pub seq: u64,
    pub action_id_hex: String,
    pub kind: P6ViolationKind,
}

/// Résultat de l'oracle P6 valid-prefix multi-agents.
#[derive(Debug, Serialize, Deserialize)]
pub struct P6ConcurrentResult {
    pub n_agents: usize,
    pub total_acked: usize,
    pub total_visible: usize,
    pub total_gaps: usize,
    pub total_icsr_violations: usize,
    pub total_parent_violations: usize,
    pub p6_ok: bool,
    pub violations: Vec<P6Violation>,
}

/// Oracle P6 valid-prefix pour le harness S15.
///
/// Pour chaque agent :
///   1. Trouve quels commits ackés sont visibles dans le log.
///   2. Vérifie que les visibles forment un préfixe contigu (pas de gap).
///   3. Pour le préfixe visible, vérifie I-CSR (SnapshotMissing, DataBlockMissing).
///   4. Vérifie l'intégrité des parent_ids des entrées visibles.
pub fn verify_p6_concurrent(
    store: &ContentStore,
    log: &CausalLog,
    witness: &ConcurrentWitness,
) -> P6ConcurrentResult {
    let mut violations = Vec::new();
    let mut total_visible = 0usize;
    let mut total_gaps = 0usize;
    let mut total_icsr = 0usize;
    let mut total_parent = 0usize;
    let total_acked: usize = witness.agents.iter().map(|a| a.acked_commits.len()).sum();

    for agent in &witness.agents {
        // 1. Déterminer quels commits ackés sont présents dans le log.
        let visible: Vec<bool> = agent.acked_commits.iter().map(|commit| {
            let Some(action_id) = hex_decode_32(&commit.action_id_hex) else { return false };
            matches!(log.get(&action_id), Ok(Some(_)))
        }).collect();

        // 2. Trouver la longueur du préfixe contigu visible.
        let prefix_len = visible.iter().take_while(|&&v| v).count();
        total_visible += prefix_len;

        // 3. Gap check : toute entrée visible après le préfixe est un gap.
        if prefix_len < visible.len() {
            // first_missing est à prefix_len
            let first_missing_seq = agent.acked_commits[prefix_len].seq;
            for (i, &vis) in visible.iter().enumerate().skip(prefix_len + 1) {
                if vis {
                    total_gaps += 1;
                    violations.push(P6Violation {
                        agent_id_hex: agent.agent_id_hex.clone(),
                        seq: agent.acked_commits[i].seq,
                        action_id_hex: agent.acked_commits[i].action_id_hex.clone(),
                        kind: P6ViolationKind::Gap {
                            first_missing_seq,
                            later_present_seq: agent.acked_commits[i].seq,
                        },
                    });
                }
            }
        }

        // 4. I-CSR pour le préfixe visible.
        for commit in &agent.acked_commits[..prefix_len] {
            let Some(snap_id) = hex_decode_32(&commit.snapshot_id_hex) else { continue };
            let Some(data_hash) = hex_decode_32(&commit.data_hash_hex) else { continue };

            match store.has_snapshot(&snap_id) {
                Ok(false) => {
                    total_icsr += 1;
                    violations.push(P6Violation {
                        agent_id_hex: agent.agent_id_hex.clone(),
                        seq: commit.seq,
                        action_id_hex: commit.action_id_hex.clone(),
                        kind: P6ViolationKind::SnapshotMissing,
                    });
                }
                Err(e) => eprintln!("has_snapshot seq={} err: {e}", commit.seq),
                Ok(true) => {}
            }

            match store.get_block(&data_hash) {
                Ok(None) => {
                    total_icsr += 1;
                    violations.push(P6Violation {
                        agent_id_hex: agent.agent_id_hex.clone(),
                        seq: commit.seq,
                        action_id_hex: commit.action_id_hex.clone(),
                        kind: P6ViolationKind::DataBlockMissing,
                    });
                }
                Err(e) => eprintln!("get_block seq={} err: {e}", commit.seq),
                Ok(Some(_)) => {}
            }
        }

        // 5. Intégrité parent_ids pour le préfixe visible.
        for commit in &agent.acked_commits[..prefix_len] {
            let Some(action_id) = hex_decode_32(&commit.action_id_hex) else { continue };
            let entry = match log.get(&action_id) {
                Ok(Some(e)) => e,
                _ => continue,
            };
            for parent_id in &entry.parent_ids {
                match log.get(parent_id) {
                    Ok(None) => {
                        total_parent += 1;
                        violations.push(P6Violation {
                            agent_id_hex: agent.agent_id_hex.clone(),
                            seq: commit.seq,
                            action_id_hex: commit.action_id_hex.clone(),
                            kind: P6ViolationKind::ParentIdMissing {
                                parent_hex: hex_encode(parent_id),
                            },
                        });
                    }
                    Err(e) => eprintln!("parent get seq={} err: {e}", commit.seq),
                    Ok(Some(_)) => {}
                }
            }
        }
    }

    let p6_ok = total_gaps == 0 && total_icsr == 0 && total_parent == 0;

    P6ConcurrentResult {
        n_agents: witness.n_agents,
        total_acked,
        total_visible,
        total_gaps,
        total_icsr_violations: total_icsr,
        total_parent_violations: total_parent,
        p6_ok,
        violations,
    }
}

// ── Utilitaires ───────────────────────────────────────────────────────────────

pub fn hex_encode(b: &[u8]) -> String {
    b.iter()
        .fold(String::with_capacity(b.len() * 2), |mut s, byte| {
            s.push_str(&format!("{byte:02x}"));
            s
        })
}

fn hex_decode_32(s: &str) -> Option<[u8; 32]> {
    if s.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, chunk) in s.as_bytes().chunks(2).enumerate() {
        let pair = std::str::from_utf8(chunk).ok()?;
        out[i] = u8::from_str_radix(pair, 16).ok()?;
    }
    Some(out)
}
