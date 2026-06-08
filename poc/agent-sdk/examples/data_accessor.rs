// data_accessor.rs — P4 en action réelle : accès capability-gated au KV store.
//
// Protocole :
//   Message : [8 bytes cap_id LE][resource name bytes]
//   → tente agent_store_put(resource, cap_id, b"value")
//   → émet "WROTE:resource" (succès) ou "DENIED:resource" (refus)
//   → reste vivant pour le prochain message
//
//   Message vide → terminate
//
// Le runner accorde une cap sur "reports/" mais PAS sur "confidential/".
// Quand l'agent tente d'écrire sur "confidential/...", agent_store_put retourne -1
// et le runtime émet un CapabilityDenied (0x14) dans le log — tracé sans action de l'agent.
//
// Build :
//   cargo build --target wasm32-unknown-unknown -p agent-sdk --example data_accessor --release
#![cfg_attr(target_arch = "wasm32", no_main)]

use agent_sdk::{barrier, emit_raw, terminate};

// Déclaration manuelle — agent_store_put n'est pas encore wrappé dans agent_sdk.
#[cfg(target_arch = "wasm32")]
#[link(wasm_import_module = "env")]
extern "C" {
    fn agent_store_put(
        resource_ptr: i32, resource_len: i32,
        cap_id: i64,
        val_ptr: i32, val_len: i32,
    ) -> i32;
}

const VALUE: &[u8] = b"agent_written_value";

#[no_mangle]
pub unsafe extern "C" fn process(ptr: i32, len: i32) {
    if len == 0 { terminate(); return; }
    let input = core::slice::from_raw_parts(ptr as *const u8, len as usize);
    if input.len() < 8 { terminate(); return; }

    // Extraire cap_id (8 bytes LE) et nom de ressource (reste)
    let cap_id = i64::from_le_bytes(input[0..8].try_into().unwrap_or([0; 8]));
    let resource = &input[8..];

    #[cfg(target_arch = "wasm32")]
    let rc = agent_store_put(
        resource.as_ptr() as i32, resource.len() as i32,
        cap_id,
        VALUE.as_ptr() as i32, VALUE.len() as i32,
    );
    #[cfg(not(target_arch = "wasm32"))]
    let rc = 0i32;

    barrier();

    if rc == 0 {
        let mut out = b"WROTE:".to_vec();
        out.extend_from_slice(resource);
        emit_raw(1, &out);
    } else {
        // rc == -1 : CapabilityDenied (loggué par le runtime en 0x14)
        let mut out = b"DENIED:".to_vec();
        out.extend_from_slice(resource);
        emit_raw(1, &out);
    }
    // Ne pas terminer — attend le prochain message
}

#[allow(dead_code)]
fn main() {}
