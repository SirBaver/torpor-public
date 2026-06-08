//! Jalon C.6 — Runtime process
//!
//! Charge le module WASM agent.cwasm (pré-compilé AOT).
//! Fournit la host function emit() qui :
//!   1. Hash un payload statique (SHA-256) → blob_hash
//!   2. Hash le blob_hash → header_hash
//!   3. Écrit 3 records dans le ring buffer (Blob, Header, LogEntry)
//!   4. Appelle ep.call() → serveur commit Q3-C en RAM
//! Après le run WASM : affiche C6_PASS, signal done_nfn, suspend.
//!
//! Note architecture : le module WASM n'a pas de mémoire linéaire pour éviter
//! la réservation virtuelle de 8GB par Wasmtime. Le payload est géré côté host.

#![no_std]
#![no_main]

extern crate alloc;

use core::sync::atomic::Ordering;

use sel4::CapTypeForFrameObjectOfFixedSize;
use sha2::{Digest, Sha256};
use wasmtime::{Engine, Linker, Module, Store};

mod platform;
mod runtime;

// Cap layout dans le runtime CNode (child_cnode_size_bits = 2)
// Slot 0: NULL
// Slot 1: endpoint (read_write)
// Slot 2: done notification (write_only)
// Slot 3: own TCB cap
const EP_SLOT: u64 = 1;
const DONE_NFN_SLOT: u64 = 2;
const TCB_SLOT: u64 = 3;

// Module WASM pré-compilé par build.rs
static AGENT_CWASM: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/agent.cwasm"));

// Payload statique émis par le module WASM
static EMIT_PAYLOAD: &[u8] = b"C6_LOG_ENTRY_PAYLOAD_FROM_WASM";

// Ring buffer layout (partagé avec le serveur)
#[repr(C)]
struct RingBuffer {
    data_len: u32,
    data: [u8; 4092],
}

fn get_ring_va() -> *mut RingBuffer {
    unsafe extern "C" {
        static _end: usize;
    }
    let ipc_buf = (core::ptr::addr_of!(_end) as usize)
        .next_multiple_of(sel4::cap_type::Granule::FRAME_OBJECT_TYPE.bytes());
    let ring = ipc_buf + sel4::cap_type::Granule::FRAME_OBJECT_TYPE.bytes();
    ring as *mut RingBuffer
}

// Ecriture d'un record dans le ring buffer
// Format: [kind: u8, hash: [u8;32], payload_len: u32 LE, payload: ...]
fn write_record(buf: &mut [u8], pos: &mut usize, kind: u8, hash: &[u8; 32], payload: &[u8]) -> bool {
    let needed = 1 + 32 + 4 + payload.len();
    if *pos + needed > buf.len() {
        return false;
    }
    buf[*pos] = kind;
    *pos += 1;
    buf[*pos..*pos + 32].copy_from_slice(hash);
    *pos += 32;
    let plen = payload.len() as u32;
    buf[*pos..*pos + 4].copy_from_slice(&plen.to_le_bytes());
    *pos += 4;
    buf[*pos..*pos + payload.len()].copy_from_slice(payload);
    *pos += payload.len();
    true
}

// Contexte de la host function emit() — passé via Store
struct EmitCtx {
    ep: sel4::cap::Endpoint,
    ring_va: *mut RingBuffer,
}

// Safety: runtime seL4 = single-threaded, pas de races réelles
unsafe impl Send for EmitCtx {}
unsafe impl Sync for EmitCtx {}

pub fn main() -> ! {
    sel4::debug_println!("[C6] runtime: démarrage");

    let ep = sel4::cap::Endpoint::from_bits(EP_SLOT);
    let done_nfn = sel4::cap::Notification::from_bits(DONE_NFN_SLOT);
    let ring_va = get_ring_va();

    // Initialiser le ring buffer
    unsafe {
        (*ring_va).data_len = 0;
    }

    let ctx = EmitCtx { ep, ring_va };

    // Créer le moteur Wasmtime (runtime-only, pas de cranelift)
    let engine = Engine::default();
    let module = unsafe {
        Module::deserialize(&engine, AGENT_CWASM).expect("Module::deserialize échoué")
    };

    let mut linker: Linker<EmitCtx> = Linker::new(&engine);

    // Enregistrer la host function emit() -> i32
    // Le module WASM n'a pas de mémoire linéaire — le payload est fixé côté host
    linker
        .func_wrap("env", "emit", |caller: wasmtime::Caller<'_, EmitCtx>| -> i32 {
            let payload = EMIT_PAYLOAD;

            // SHA-256(payload) → blob_hash
            let blob_hash: [u8; 32] = {
                let mut h = Sha256::new();
                h.update(payload);
                h.finalize().into()
            };

            // header_bytes = blob_hash, SHA-256(header_bytes) → header_hash
            let header_bytes = blob_hash;
            let header_hash: [u8; 32] = {
                let mut h = Sha256::new();
                h.update(&header_bytes);
                h.finalize().into()
            };

            // Écrire les 3 records dans le ring buffer
            let ring = unsafe { &mut *caller.data().ring_va };
            let mut pos = 0usize;
            let buf = &mut ring.data;

            let ok1 = write_record(buf, &mut pos, 0, &blob_hash, payload);          // Blob
            let ok2 = write_record(buf, &mut pos, 1, &header_hash, &header_bytes);  // Header
            let ok3 = write_record(buf, &mut pos, 2, &header_hash, &[]);             // LogEntry

            if !ok1 || !ok2 || !ok3 {
                sel4::debug_println!("[C6] emit: ring buffer trop petit");
                return -1;
            }

            ring.data_len = pos as u32;

            // Fence SeqCst avant l'appel IPC
            core::sync::atomic::fence(Ordering::SeqCst);

            // ep.call() — bloque jusqu'au reply du serveur
            let ep = caller.data().ep;
            ep.call(sel4::MessageInfo::new(0, 0, 0, 0));

            // Remettre le ring à zéro pour la prochaine émission
            unsafe {
                (*caller.data().ring_va).data_len = 0;
            }

            0 // succès
        })
        .expect("linker.func_wrap emit échoué");

    let mut store = Store::new(&engine, ctx);

    let instance = linker
        .instantiate(&mut store, &module)
        .expect("linker.instantiate échoué");

    sel4::debug_println!("[C6] runtime: module WASM instancié");

    // Appeler la fonction run() du module WASM
    let run = instance
        .get_typed_func::<(), ()>(&mut store, "run")
        .expect("get_typed_func 'run' échoué");

    run.call(&mut store, ()).expect("call run() échoué");

    sel4::debug_println!("[C6] runtime: WASM run() terminé");
    sel4::debug_println!("C6_PASS");

    // Signaler au superviseur que c'est terminé
    done_nfn.signal();

    // Suspendre ce thread
    sel4::cap::Tcb::from_bits(TCB_SLOT).tcb_suspend().unwrap();
    unreachable!()
}
