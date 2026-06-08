//! Jalon C.10-crash — Runtime process (crash dans fenêtre de remap W→X)
//!
//! Charge le module WASM agent.cwasm (pré-compilé AOT).
//! Fournit la host function emit() (protocole Q3-C, K=1 commit).
//! Wasmtime JIT utilise le pool de frames dédiées avec W^X actif (platform.rs).
//!
//! Séquence :
//!   1. Wasmtime JIT compile + exécute agent.cwasm → K=1 commit via emit()
//!      (le commit est stocké dans le store redb/virtio-blk du server)
//!   2. crash_in_remap_window() : unmap une page RX, PUIS signal suspend_nfn + tcb_suspend
//!      → simule un crash dans la fenêtre de remap (KP_WX : entre frame_unmap et frame_map)
//!
//! Le superviseur attend suspend_nfn, puis oracle query → vérifie seq_a == K=1.
//! Preuve : le store est intact malgré le crash dans la fenêtre de remap.
//!
//! Cap layout CNode runtime (size_bits=8) :
//!   Slot 1 : EP commit (badge=AGENT_A_ID)
//!   Slot 2 : suspend_nfn (write_only) — signal avant tcb_suspend (pattern c7-crash)
//!   Slot 3 : TCB
//!   Slot 4 : VSpace (pour wasmtime_mprotect)
//!   Slots 5..132 : frames JIT

#![no_std]
#![no_main]

extern crate alloc;

use core::sync::atomic::Ordering;

use sel4::CapTypeForFrameObjectOfFixedSize;
use sha2::{Digest, Sha256};
use wasmtime::{Engine, Linker, Module, Store};

mod platform;
mod runtime;

const EP_SLOT:  u64 = 1;
const NFN_SLOT: u64 = 2; // suspend_nfn (write_only)
const TCB_SLOT: u64 = 3;

// Module WASM pré-compilé par build.rs
static AGENT_CWASM: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/agent.cwasm"));

static EMIT_PAYLOAD: &[u8] = b"C10_CRASH_COMMIT_FROM_JIT_AGENT";

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

fn write_record(buf: &mut [u8], pos: &mut usize, kind: u8, hash: &[u8; 32], payload: &[u8]) -> bool {
    let needed = 1 + 32 + 4 + payload.len();
    if *pos + needed > buf.len() { return false; }
    buf[*pos] = kind; *pos += 1;
    buf[*pos..*pos + 32].copy_from_slice(hash); *pos += 32;
    let plen = payload.len() as u32;
    buf[*pos..*pos + 4].copy_from_slice(&plen.to_le_bytes()); *pos += 4;
    buf[*pos..*pos + payload.len()].copy_from_slice(payload); *pos += payload.len();
    true
}

struct EmitCtx {
    ep: sel4::cap::Endpoint,
    ring_va: *mut RingBuffer,
}

unsafe impl Send for EmitCtx {}
unsafe impl Sync for EmitCtx {}

pub fn main() -> ! {
    sel4::debug_println!("[C10-crash] runtime: démarrage");

    let ep = sel4::cap::Endpoint::from_bits(EP_SLOT);
    let ring_va = get_ring_va();

    unsafe { (*ring_va).data_len = 0; }

    let ctx = EmitCtx { ep, ring_va };

    // JIT compile + exécute le module avec W^X actif (platform.rs gère le remap)
    let engine = Engine::default();
    let module = unsafe {
        Module::deserialize(&engine, AGENT_CWASM).expect("Module::deserialize échoué")
    };

    let mut linker: Linker<EmitCtx> = Linker::new(&engine);

    linker
        .func_wrap("env", "emit", |caller: wasmtime::Caller<'_, EmitCtx>| -> i32 {
            let payload = EMIT_PAYLOAD;

            let blob_hash: [u8; 32] = {
                let mut h = Sha256::new();
                h.update(payload);
                h.finalize().into()
            };
            let header_bytes = blob_hash;
            let header_hash: [u8; 32] = {
                let mut h = Sha256::new();
                h.update(&header_bytes);
                h.finalize().into()
            };

            let ring = unsafe { &mut *caller.data().ring_va };
            let mut pos = 0usize;
            let buf = &mut ring.data;

            let ok1 = write_record(buf, &mut pos, 0, &blob_hash, payload);
            if !ok1 { return -1; }
            let ok2 = write_record(buf, &mut pos, 1, &header_hash, &header_bytes);
            if !ok2 { return -1; }
            let ok3 = write_record(buf, &mut pos, 2, &header_hash, &[]);
            if !ok3 { return -1; }

            ring.data_len = pos as u32;
            core::sync::atomic::fence(Ordering::SeqCst);

            // Commit via IPC → server stocke dans redb/virtio-blk
            let ep = caller.data().ep;
            ep.call(sel4::MessageInfo::new(0, 0, 0, 0));

            unsafe { (*caller.data().ring_va).data_len = 0; }
            0
        })
        .expect("linker.func_wrap emit échoué");

    let mut store = Store::new(&engine, ctx);

    let instance = linker
        .instantiate(&mut store, &module)
        .expect("linker.instantiate échoué");

    sel4::debug_println!("[C10-crash] runtime: module WASM instancié (JIT W^X actif)");

    let run = instance
        .get_typed_func::<(), ()>(&mut store, "run")
        .expect("get_typed_func 'run' échoué");

    run.call(&mut store, ()).expect("call run() échoué");

    sel4::debug_println!("[C10-crash] runtime: run() terminé — K=1 commit sous W^X ✓");

    // ── Crash dans la fenêtre de remap (ADR-0047 §D7) ─────────────────────────
    // À ce stade : K=1 commit est durablement stocké dans redb/virtio-blk.
    // On simule un crash dans la fenêtre de remap W→X :
    //   - unmap une page JIT RX (page transitoirement non-mappée)
    //   - signal suspend_nfn + tcb_suspend (le processus "crashe")
    // Le superviseur observe suspend_nfn et vérifie que seq_a == K=1.
    sel4::debug_println!("[C10-crash] runtime: crash dans fenêtre de remap (KP_WX)");
    unsafe { platform::crash_in_remap_window(NFN_SLOT, TCB_SLOT) }
}
