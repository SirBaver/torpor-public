// log_dump — affiche les entrées du CausalLog d'une session chat.
//
// Usage :
//   cargo run --bin log_dump -- /tmp/chat-XXXXXXXX/log
use std::sync::Arc;

use os_poc_causal_log::{CausalLog, EmitEnvelope, EmitType};
use os_poc_store::{Cache, ContentStore};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: log_dump <log_dir> [store_dir]");
        std::process::exit(1);
    }

    let log_path = std::path::Path::new(&args[1]);
    let cache = Cache::new_lru_cache(16 * 1024 * 1024);
    let log = CausalLog::open(log_path, Some(cache.clone()))
        .unwrap_or_else(|e| { eprintln!("Erreur ouverture log: {e}"); std::process::exit(1); });

    // agent_id : "chat-agent-00001" (chat-runner) ou "evict-agent-aaaa" (evict-wake-runner)
    let agent_id_str = args.get(2).map(String::as_str).unwrap_or("chat-agent-00001");
    let mut agent_id = [0u8; 16];
    let b = agent_id_str.as_bytes();
    agent_id[..b.len().min(16)].copy_from_slice(&b[..b.len().min(16)]);

    let ids = log.query_by_agent_range(&agent_id, None, None)
        .unwrap_or_default();

    if ids.is_empty() {
        println!("Aucune entrée trouvée pour chat-agent-00001.");
        return;
    }

    println!("=== CausalLog — {} entrées ===\n", ids.len());

    for (i, action_id) in ids.iter().enumerate() {
        let Ok(Some(entry)) = log.get(action_id) else { continue };

        let action_hex = hex(&action_id[..8]);
        let parent_hex = if entry.parent_ids.is_empty() {
            "genesis".to_string()
        } else {
            entry.parent_ids.iter().map(|p| hex(&p[..8])).collect::<Vec<_>>().join(" + ")
        };
        let hash_after = hex(&entry.hash_after[..8]);

        let (type_name, payload_summary) = if let Some(pb) = &entry.emit_payload {
            if let Ok(env) = EmitEnvelope::from_msgpack(pb) {
                let et = EmitType::try_from(env.emit_type)
                    .map(|t| format!("{:?}", t))
                    .unwrap_or_else(|_| format!("0x{:02X}", env.emit_type));
                let summary = match EmitType::try_from(env.emit_type) {
                    Ok(EmitType::ActionResult) => {
                        let text = String::from_utf8_lossy(&env.payload);
                        format!("\"{}\"", truncate(&text, 80))
                    }
                    Ok(EmitType::InferenceFailed) if env.payload.len() >= 2 => {
                        let code = env.payload[0];
                        let ml = env.payload[1] as usize;
                        let msg = if env.payload.len() >= 2 + ml {
                            String::from_utf8_lossy(&env.payload[2..2 + ml]).to_string()
                        } else { "?".to_string() };
                        format!("code=0x{code:02X} msg={msg:?}")
                    }
                    Ok(EmitType::InferenceRequest) => {
                        format!("prompt_hash={}", hex(&env.payload[..8]))
                    }
                    Ok(EmitType::InferenceResponse) => {
                        let dur = if env.payload.len() >= 36 {
                            u32::from_le_bytes(env.payload[32..36].try_into().unwrap_or([0;4]))
                        } else { 0 };
                        format!("duration={dur}ms")
                    }
                    _ => format!("{} bytes", env.payload.len()),
                };
                (et, summary)
            } else {
                ("(parse error)".to_string(), String::new())
            }
        } else {
            ("(no payload)".to_string(), String::new())
        };

        println!("#{i:02} action={action_hex}… parent={parent_hex}… hash={hash_after}…");
        println!("     ts={} ms | type={type_name}", entry.ts_ms);
        if !payload_summary.is_empty() {
            println!("     payload={payload_summary}");
        }
        println!();
    }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(max).collect::<String>())
    }
}
