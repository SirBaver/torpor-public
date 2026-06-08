// brainstorm_synth.rs — synthétiseur de brainstorm multi-agent.
//
// Reçoit les résultats de 3 agents brainstormers (un message par agent).
// Ne commite PAS de barrier sur les 2 premiers messages : les causes s'accumulent
// dans pending_extra_causes. Au 3e message : barrier() crée une entrée de log
// avec 3+ parent_ids (fan-in causal), puis infer sélectionne le meilleur nom.
//
// Propriété démontrée :
//   - P3b : vrai fan-in DAG — la décision finale a des parent_ids qui pointent
//     vers les 3 brainstormers en parallèle, non vers une chaîne linéaire.
//   - Contraste : Message::caused ne transporte qu'une cause à la fois,
//     mais l'accumulation dans pending_extra_causes permet un fan-in réel.
//
// Build :
//   cargo build --target wasm32-unknown-unknown -p agent-sdk --example brainstorm_synth --release
#![cfg_attr(target_arch = "wasm32", no_main)]

use agent_sdk::{barrier, emit_raw, infer, terminate, INFER_RESPONSE_BUF_LEN};

const RESULT_BUF: usize = 512;

static mut RESULTS:     [[u8; RESULT_BUF]; 3] = [[0u8; RESULT_BUF]; 3];
static mut RESULT_LENS: [usize; 3]            = [0usize; 3];
static mut COUNT:       usize                 = 0;
static mut DECISION:    [u8; INFER_RESPONSE_BUF_LEN] = [0u8; INFER_RESPONSE_BUF_LEN];

#[no_mangle]
pub unsafe extern "C" fn process(ptr: i32, len: i32) {
    if len == 0 { terminate(); return; }
    let data = core::slice::from_raw_parts(ptr as *const u8, len as usize);

    let idx = *core::ptr::addr_of!(COUNT);
    if idx >= 3 { terminate(); return; }

    // Stocker ce résultat SANS appeler barrier() — pending_extra_causes accumule
    let copy_len = data.len().min(RESULT_BUF);
    let results = &mut *core::ptr::addr_of_mut!(RESULTS);
    results[idx][..copy_len].copy_from_slice(&data[..copy_len]);
    let lens = &mut *core::ptr::addr_of_mut!(RESULT_LENS);
    lens[idx] = copy_len;
    *core::ptr::addr_of_mut!(COUNT) = idx + 1;

    if idx + 1 < 3 {
        // Pas encore tous les résultats — retour sans barrier
        // Les causes des messages 1 et 2 restent dans pending_extra_causes
        return;
    }

    // Tous les 3 résultats reçus — construire le prompt de synthèse
    let r = &*core::ptr::addr_of!(RESULTS);
    let l = &*core::ptr::addr_of!(RESULT_LENS);

    let mut prompt: Vec<u8> = Vec::with_capacity(256 + l[0] + l[1] + l[2]);
    prompt.extend_from_slice(
        b"Three teams proposed product names for an AI operating system.\n\
        Choose the single BEST name overall. Reply with exactly:\n\
        WINNER: <name>\nREASON: <one sentence>\n\n",
    );
    prompt.extend_from_slice(b"Team MYTH (mythological): ");
    prompt.extend_from_slice(&r[0][..l[0]]);
    prompt.extend_from_slice(b"\nTeam TECH (technical precision): ");
    prompt.extend_from_slice(&r[1][..l[1]]);
    prompt.extend_from_slice(b"\nTeam MODERN (contemporary culture): ");
    prompt.extend_from_slice(&r[2][..l[2]]);
    prompt.extend_from_slice(b"\n\nWINNER:");

    let dec = &mut *core::ptr::addr_of_mut!(DECISION);
    let resp_len = match infer(&prompt, dec, 90_000) {
        Ok(n) => n,
        Err(_) => {
            barrier();
            emit_raw(1, b"[synthesis error: inference failed]");
            terminate();
            return;
        }
    };

    // barrier() ici — parent_ids contiendra les 3 causes accumulees (fan-in!)
    barrier();
    emit_raw(1, &dec[..resp_len]);
    terminate();
}

#[allow(dead_code)]
fn main() {}
