// os-poc-reconstruct — log-dump du log causal par agent (Phase 2, ADR-0018).
//
// Usage :
//   os-poc-reconstruct --db <path> --agent <hex32> [--from-ts <ms>] [--to-ts <ms>]
//
// Sortie : une ligne par LogEntry, ordre temporel croissant.
// Format : <ts_ms>  <agent_short>  <action_short>  <emit_type>  <summary>

use os_poc_causal_log::{AgentId, CausalLog, EmitEnvelope, EmitType};
use std::path::PathBuf;
use std::process;

struct Args {
    db: PathBuf,
    agent: AgentId,
    from_ts: Option<u64>,
    to_ts: Option<u64>,
}

fn usage() -> ! {
    eprintln!("Usage: os-poc-reconstruct --db <path> --agent <hex32> [--from-ts <ms>] [--to-ts <ms>]");
    eprintln!("  --agent   : agent_id en hexadécimal (16 bytes = 32 chars)");
    eprintln!("  --from-ts : borne inférieure en ms Unix (optionnel)");
    eprintln!("  --to-ts   : borne supérieure en ms Unix (optionnel)");
    process::exit(1);
}

fn parse_args() -> Args {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let mut db: Option<PathBuf> = None;
    let mut agent: Option<AgentId> = None;
    let mut from_ts: Option<u64> = None;
    let mut to_ts: Option<u64> = None;

    let mut i = 0;
    while i < raw.len() {
        match raw[i].as_str() {
            "--db" => {
                i += 1;
                db = Some(PathBuf::from(raw.get(i).unwrap_or_else(|| { eprintln!("--db nécessite un argument"); process::exit(1); })));
            }
            "--agent" => {
                i += 1;
                let hex = raw.get(i).unwrap_or_else(|| { eprintln!("--agent nécessite un argument"); process::exit(1); });
                let bytes = hex::decode(hex).unwrap_or_else(|e| { eprintln!("--agent hex invalide : {e}"); process::exit(1); });
                if bytes.len() != 16 {
                    eprintln!("--agent doit être 16 bytes (32 chars hex), reçu {} bytes", bytes.len());
                    process::exit(1);
                }
                let mut id = [0u8; 16];
                id.copy_from_slice(&bytes);
                agent = Some(id);
            }
            "--from-ts" => {
                i += 1;
                let s = raw.get(i).unwrap_or_else(|| { eprintln!("--from-ts nécessite un argument"); process::exit(1); });
                from_ts = Some(s.parse().unwrap_or_else(|_| { eprintln!("--from-ts doit être un entier"); process::exit(1); }));
            }
            "--to-ts" => {
                i += 1;
                let s = raw.get(i).unwrap_or_else(|| { eprintln!("--to-ts nécessite un argument"); process::exit(1); });
                to_ts = Some(s.parse().unwrap_or_else(|_| { eprintln!("--to-ts doit être un entier"); process::exit(1); }));
            }
            other => {
                eprintln!("Argument inconnu : {other}");
                usage();
            }
        }
        i += 1;
    }

    Args {
        db: db.unwrap_or_else(|| { eprintln!("--db requis"); usage(); }),
        agent: agent.unwrap_or_else(|| { eprintln!("--agent requis"); usage(); }),
        from_ts,
        to_ts,
    }
}

fn summarize(emit_type: u8, payload: &[u8]) -> String {
    match EmitType::try_from(emit_type) {
        Ok(EmitType::ActionResult) => format!("({} bytes)", payload.len()),
        Ok(EmitType::StateDelta)   => format!("({} bytes msgpack)", payload.len()),
        Ok(EmitType::Event)        => format!("({} bytes)", payload.len()),
        Ok(EmitType::Proposal)     => format!("({} bytes)", payload.len()),
        Ok(EmitType::Lifecycle) => {
            // Payload : [state_byte(1), seq_le(8), ...]
            // ADR-0025 : si state_byte == 0x00 (Spawned) et payload.len() >= 10,
            //            le byte [9] est le profil watchdog.
            if payload.is_empty() {
                return "(payload tronqué, 0 bytes)".to_string();
            }
            let state_byte = payload[0];
            let state_name = match state_byte {
                0 => "Spawned",
                1 => "Active",
                2 => "Suspended",
                3 => "Checkpointed",
                4 => "Terminated",
                5 => "AwaitingValidation",
                6 => "WaitingInference",
                v => return format!("state=Unknown({v})"),
            };
            if state_byte == 0 && payload.len() >= 10 {
                // Spawned avec profil ADR-0025
                let seq = if payload.len() >= 9 {
                    u64::from_le_bytes(payload[1..9].try_into().unwrap())
                } else { 0 };
                let profile_name = match payload[9] {
                    0x01 => "Algo",
                    0x02 => "LlmShort",
                    0x03 => "LlmLong",
                    0x04 => "Batch",
                    v => return format!("state={state_name} seq={seq} profile=Unknown(0x{v:02x})"),
                };
                format!("state={state_name} seq={seq} profile={profile_name}")
            } else if payload.len() >= 9 {
                let seq = u64::from_le_bytes(payload[1..9].try_into().unwrap());
                format!("state={state_name} seq={seq}")
            } else {
                format!("state={state_name} (payload court, {} bytes)", payload.len())
            }
        }
        Ok(EmitType::Introspect) => {
            // Format fixe : last_action_id(32) | seq(8 u64 LE) | last_snapshot(32) | flags(1) | lifecycle(1) = 74 bytes
            if payload.len() >= 74 {
                let seq = u64::from_le_bytes(payload[32..40].try_into().unwrap());
                let flags = payload[72];
                let lifecycle = payload[73];
                format!("seq={seq} flags=0x{flags:02x} lifecycle={lifecycle}")
            } else {
                format!("(payload tronqué, {} bytes)", payload.len())
            }
        }
        Ok(EmitType::SelfRollback) => {
            // depth(1) | target_seq(8 u64 LE) = 9 bytes
            if payload.len() >= 9 {
                let depth = payload[0];
                let target_seq = u64::from_le_bytes(payload[1..9].try_into().unwrap());
                format!("depth={depth} target_seq={target_seq}")
            } else {
                format!("(payload tronqué, {} bytes)", payload.len())
            }
        }
        Ok(EmitType::ValidationRequest) => {
            if !payload.is_empty() {
                format!("risk={}", payload[0])
            } else {
                "(payload tronqué, 0 bytes)".to_string()
            }
        }
        Ok(EmitType::ValidationResponse) => {
            if !payload.is_empty() {
                let verdict = match payload[0] {
                    0 => "Approved",
                    1 => "Rejected",
                    2 => "Timeout",
                    v => return format!("verdict=Unknown({v})"),
                };
                format!("verdict={verdict}")
            } else {
                "(payload tronqué, 0 bytes)".to_string()
            }
        }
        Ok(EmitType::SessionBoundary) => format!("({} bytes résumé causal)", payload.len()),
        Ok(EmitType::SchedulerRollback) => {
            // distance(1) | target_seq(8 u64 LE) | caps_invalidated(1) = 10 bytes
            if payload.len() >= 10 {
                let distance = payload[0];
                let target_seq = u64::from_le_bytes(payload[1..9].try_into().unwrap());
                let caps = payload[9];
                format!("distance={distance} target_seq={target_seq} caps_invalidated={caps}")
            } else {
                format!("(payload tronqué, {} bytes)", payload.len())
            }
        }
        Ok(EmitType::InferenceRequest) => {
            // prompt_hash(32) | model_id_len(1) | model_id([u8;N]) | timeout_req(4 LE) | timeout_eff(4 LE)
            // ADR-0022 : bytes additionnels optionnels : priority_class(1) | queue_depth(2 LE) | promoted_from(1)
            if payload.len() >= 33 {
                let model_len = payload[32] as usize;
                let base = 33 + model_len;
                if payload.len() >= base + 8 {
                    let req = u32::from_le_bytes(payload[base..base+4].try_into().unwrap());
                    let eff = u32::from_le_bytes(payload[base+4..base+8].try_into().unwrap());
                    let model = String::from_utf8_lossy(&payload[33..33+model_len]);
                    // Enrichissement ADR-0022 (bytes optionnels).
                    if payload.len() >= base + 8 + 4 {
                        let pclass = match payload[base+8] {
                            0x01 => "Supervisor",
                            0x02 => "Foreground",
                            0x03 => "Batch",
                            v    => return format!("model={model} timeout_req={req}ms timeout_eff={eff}ms priority=Unknown({v:#04x})"),
                        };
                        let qdepth = u16::from_le_bytes(payload[base+9..base+11].try_into().unwrap());
                        let promoted = match payload[base+11] {
                            0xFF => "none".to_string(),
                            0x01 => "from=Supervisor".to_string(),
                            0x02 => "from=Foreground".to_string(),
                            0x03 => "from=Batch".to_string(),
                            v    => format!("from=Unknown({v:#04x})"),
                        };
                        format!("model={model} timeout_req={req}ms timeout_eff={eff}ms priority={pclass} qdepth={qdepth} promoted={promoted}")
                    } else {
                        format!("model={model} timeout_req={req}ms timeout_eff={eff}ms")
                    }
                } else {
                    format!("(payload tronqué, {} bytes)", payload.len())
                }
            } else {
                format!("(payload tronqué, {} bytes)", payload.len())
            }
        }
        Ok(EmitType::InferenceResponse) => {
            // response_hash(32) | tokens_estimated(4 LE) | duration_ms(4 LE) | truncated(1)
            if payload.len() >= 41 {
                let tokens = u32::from_le_bytes(payload[32..36].try_into().unwrap());
                let duration = u32::from_le_bytes(payload[36..40].try_into().unwrap());
                let truncated = payload[40];
                format!("tokens~={tokens} duration={duration}ms truncated={truncated}")
            } else {
                format!("(payload tronqué, {} bytes)", payload.len())
            }
        }
        Ok(EmitType::InferenceCancelled) => {
            // cancel_ts_ms(8 LE) | cause(1)
            if payload.len() >= 9 {
                let ts = u64::from_le_bytes(payload[0..8].try_into().unwrap());
                let cause = match payload[8] {
                    0x01 => "Rollback",
                    0x02 => "Terminate",
                    v    => return format!("cause=Unknown(0x{v:02x})"),
                };
                format!("cancel_ts={ts}ms cause={cause}")
            } else {
                format!("(payload tronqué, {} bytes)", payload.len())
            }
        }
        Ok(EmitType::InferenceFailed) => {
            // error_code(1) | message_len(1) | message([u8;N≤255])
            if payload.len() >= 2 {
                let code = payload[0];
                let msg_len = payload[1] as usize;
                let msg = if payload.len() >= 2 + msg_len {
                    String::from_utf8_lossy(&payload[2..2+msg_len]).to_string()
                } else {
                    "(message tronqué)".to_string()
                };
                format!("error_code=0x{code:02x} msg={msg:?}")
            } else {
                format!("(payload tronqué, {} bytes)", payload.len())
            }
        }
        Ok(EmitType::CompensationOpen) => {
            // agent_id(16) | expected_inference_event_id(32)
            if payload.len() >= 16 {
                let agent = hex::encode(&payload[..8]);
                format!("agent={}... [ouverture compensation]", agent)
            } else {
                format!("(payload tronqué, {} bytes)", payload.len())
            }
        }
        Ok(EmitType::CompensationClose) => {
            // agent_id(16)
            if payload.len() >= 16 {
                let agent = hex::encode(&payload[..8]);
                format!("agent={}... [fermeture compensation]", agent)
            } else {
                format!("(payload tronqué, {} bytes)", payload.len())
            }
        }
        Ok(EmitType::AgentCrash) => {
            // ADR-0015 D15.2 — payload fixe 49 bytes :
            //   [0]      cause u8
            //   [1..17]  parent_agent_id 16B ([0u8;16] = racine, pas de parent)
            //   [17..49] last_action_id 32B  ([0u8;32] = aucune action émise)
            if payload.len() < 49 {
                return format!("(payload tronqué, {} bytes, attendu 49)", payload.len());
            }
            let cause = match payload[0] {
                0x01 => "ProcessFailed",
                0x02 => "ContentStoreBroken",
                0x03 => "WatchdogTrap",
                0x04 => "HostPanic",
                v    => return format!("cause=Unknown(0x{v:02x})"),
            };
            let parent_zero = payload[1..17].iter().all(|b| *b == 0);
            let parent_repr = if parent_zero {
                "none".to_string()
            } else {
                format!("{}...", hex::encode(&payload[1..9]))
            };
            let action_zero = payload[17..49].iter().all(|b| *b == 0);
            let action_repr = if action_zero {
                "none".to_string()
            } else {
                format!("{}...", hex::encode(&payload[17..25]))
            };
            format!(
                "cause={cause} parent={parent_repr} last_action={action_repr}"
            )
        }
        Ok(EmitType::CapabilityDenied) => {
            // SEF-3 / P4 — payload :
            //   [agent_id 16B | cap_id u64 LE 8B | resource_len u8 | resource [u8;N≤255]
            //    | perm_flags u8 | rate_limited u8]
            // Si rate_limited=0x01 : resource_len est le premier byte d'un count u32 LE.
            if payload.len() < 25 {
                return format!("(payload tronqué, {} bytes)", payload.len());
            }
            let cap_id = u64::from_le_bytes(payload[16..24].try_into().unwrap());
            let rate_limited_pos = if payload.len() >= 26 {
                // Regarder si le champ rate_limited est à la fin
                // standard: 16(agent) + 8(cap_id) + 1(res_len) + res_len + 1(perm) + 1(rl)
                let res_len = payload[24] as usize;
                if payload.len() >= 25 + res_len + 2 {
                    let rate_limited = payload[25 + res_len + 1];
                    if rate_limited == 0x01 {
                        // Agrégé : les 4 bytes après cap_id sont un count u32 LE
                        let count = u32::from_le_bytes(payload[24..28].try_into().unwrap_or([0;4]));
                        let perm = payload[28];
                        return format!("cap_id={cap_id} [RATE_LIMITED count={count} perm=0x{perm:02x}]");
                    }
                    let resource = String::from_utf8_lossy(&payload[25..25+res_len]);
                    let perm_flags = payload[25 + res_len];
                    let mut perms = Vec::new();
                    if perm_flags & 0x01 != 0 { perms.push("read"); }
                    if perm_flags & 0x02 != 0 { perms.push("write"); }
                    if perm_flags & 0x04 != 0 { perms.push("execute"); }
                    if perm_flags & 0x08 != 0 { perms.push("delegate"); }
                    let perm_str = if perms.is_empty() { "none".to_string() } else { perms.join("+") };
                    format!("cap_id={cap_id} resource={resource:?} denied={perm_str}")
                } else {
                    format!("cap_id={cap_id} (payload court {} bytes)", payload.len())
                }
            } else {
                format!("cap_id={cap_id} (payload court {} bytes)", payload.len())
            };
            rate_limited_pos
        }
        Err(_) => format!("({} bytes opaque)", payload.len()),
    }
}

fn emit_type_name(emit_type: u8) -> String {
    match EmitType::try_from(emit_type) {
        Ok(t) => format!("{t:?}"),
        Err(u) => format!("Unknown(0x{u:02x})"),
    }
}

/// Agent réservé utilisé par le Scheduler pour les événements système (ADR-0024).
const SCHEDULER_AGENT_ID: [u8; 16] = [0xFFu8; 16];

/// Réconciliation du journal de compensation (ADR-0024).
///
/// Parcourt toutes les entrées du scheduler ([0xFF;16]) et détecte les
/// CompensationOpen (0x11) sans CompensationClose (0x12) correspondant.
///
/// Retourne la liste des agent_ids avec une compensation incomplète.
fn check_compensation_journal(log: &CausalLog) -> Vec<[u8; 16]> {
    let all_ids = log.query_by_agent_range(&SCHEDULER_AGENT_ID, None, None).unwrap_or_default();
    let mut open_set: Vec<[u8; 16]> = Vec::new();
    let mut incomplete: Vec<[u8; 16]> = Vec::new();

    for action_id in &all_ids {
        let entry = match log.get(action_id) {
            Ok(Some(e)) => e,
            _ => continue,
        };
        let raw = match &entry.emit_payload {
            Some(r) => r,
            None => continue,
        };
        let env = match EmitEnvelope::from_msgpack(raw) {
            Ok(e) => e,
            Err(_) => continue,
        };
        match EmitType::try_from(env.emit_type) {
            Ok(EmitType::CompensationOpen) if env.payload.len() >= 16 => {
                let mut aid = [0u8; 16];
                aid.copy_from_slice(&env.payload[..16]);
                open_set.push(aid);
            }
            Ok(EmitType::CompensationClose) if env.payload.len() >= 16 => {
                let mut aid = [0u8; 16];
                aid.copy_from_slice(&env.payload[..16]);
                // Retirer du open_set la première occurrence correspondante.
                if let Some(pos) = open_set.iter().position(|x| x == &aid) {
                    open_set.remove(pos);
                }
            }
            _ => {}
        }
    }

    // Ce qui reste dans open_set n'a pas de Close correspondant.
    for aid in open_set {
        if !incomplete.contains(&aid) {
            incomplete.push(aid);
        }
    }
    incomplete
}

fn main() {
    let args = parse_args();

    let log = CausalLog::open(&args.db, None).unwrap_or_else(|e| {
        eprintln!("Impossible d'ouvrir la DB {:?} : {e}", args.db);
        process::exit(1);
    });

    // ── Réconciliation du journal de compensation (ADR-0024) ─────────────────
    let incomplete_compensations = check_compensation_journal(&log);
    for agent_id in &incomplete_compensations {
        eprintln!(
            "[INCOMPLETE COMPENSATION: agent_id={}] (auto-close + warning — crash probable entre 0x0E et 0x0B)",
            hex::encode(agent_id)
        );
    }

    let action_ids = log.query_by_agent_range(&args.agent, args.from_ts, args.to_ts)
        .unwrap_or_else(|e| { eprintln!("query_by_agent_range: {e}"); process::exit(1); });

    if action_ids.is_empty() {
        eprintln!("Aucune entrée trouvée pour cet agent dans la fenêtre demandée.");
        return;
    }

    println!(
        "{:<16}  {:<16}  {:<16}  {:<22}  {}",
        "ts_ms", "agent", "action", "emit_type", "summary"
    );
    println!("{}", "-".repeat(90));

    for action_id in &action_ids {
        let entry = match log.get(action_id) {
            Ok(Some(e)) => e,
            Ok(None) => {
                eprintln!("WARN: action_id {} introuvable dans CF default (incohérence index)", hex::encode(&action_id[..8]));
                continue;
            }
            Err(e) => {
                eprintln!("WARN: erreur lecture action_id {} : {e}", hex::encode(&action_id[..8]));
                continue;
            }
        };

        let agent_short = hex::encode(&entry.agent_id[..8]);
        let action_short = hex::encode(&action_id[..8]);

        // SEF-7.3 — validation des parent_ids : warn si un parent référencé est absent du log.
        for parent_id in &entry.parent_ids {
            match log.get(parent_id) {
                Ok(Some(_)) => {}
                Ok(None) => eprintln!(
                    "WARN: parent_id {} référencé par action {} introuvable dans le log (DAG incomplet)",
                    hex::encode(&parent_id[..8]),
                    hex::encode(&action_id[..8])
                ),
                Err(e) => eprintln!(
                    "WARN: erreur lecture parent_id {} depuis action {} : {e}",
                    hex::encode(&parent_id[..8]),
                    hex::encode(&action_id[..8])
                ),
            }
        }

        match &entry.emit_payload {
            None => {
                println!(
                    "{:<16}  {:<16}  {:<16}  {:<22}  —",
                    entry.ts_ms, agent_short, action_short, "checkpoint"
                );
            }
            Some(raw) => {
                match EmitEnvelope::from_msgpack(raw) {
                    Ok(env) => {
                        let type_name = emit_type_name(env.emit_type);
                        let summary = summarize(env.emit_type, &env.payload);
                        println!(
                            "{:<16}  {:<16}  {:<16}  {:<22}  {}",
                            entry.ts_ms, agent_short, action_short, type_name, summary
                        );
                        // ADR-0015 P-D15-1 (amendé 2026-05-18) : AgentCrash est l'événement
                        // terminal — aucun Lifecycle::Terminated séparé dans le log après un
                        // crash. Synthétiser ici pour que les consommateurs (SEF, audit) voient
                        // un état Terminated explicite sans scanner la logique d'émission.
                        if env.emit_type == EmitType::AgentCrash as u8 {
                            println!(
                                "{:<16}  {:<16}  {:<16}  {:<22}  state=Terminated [synthétisé — ADR-0015]",
                                entry.ts_ms, agent_short, "[implicite]", "Lifecycle"
                            );
                        }
                    }
                    Err(_) => {
                        println!(
                            "{:<16}  {:<16}  {:<16}  {:<22}  (enveloppe corrompue, {} bytes raw)",
                            entry.ts_ms, agent_short, action_short, "CORRUPT", raw.len()
                        );
                    }
                }
            }
        }
    }

    println!("{}", "-".repeat(90));
    println!("{} entrée(s) affichée(s).", action_ids.len());
}
