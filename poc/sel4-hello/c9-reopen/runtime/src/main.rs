//! Jalon C.9 — Runtime process (Rust pur, sans Wasmtime)
//!
//! Envoie K=100 commits déterministes au serveur via ring buffer SPSC + seL4_Call.
//! Pour chaque commit i, écrit 3 records dans le ring :
//!   kind=0 (blob)      : hash=[i as u8; 32], payload=b"C9_REOPEN_BLOB"
//!   kind=1 (header)    : hash=[i.wrapping_add(1) as u8; 32], payload=blob_hash
//!   kind=2 (log_entry) : hash=header_hash, payload=[]
//!
//! Après K commits : signal superviseur via Notification, puis self-suspend.
//!
//! CNode layout (size_bits=2) :
//!   Slot 1 : endpoint commit (badge=AGENT_A_ID)
//!   Slot 2 : done notification (write_only)
//!   Slot 3 : own TCB cap

#![no_std]
#![no_main]

use core::sync::atomic::Ordering;

use sel4::CapTypeForFrameObjectOfFixedSize;

mod runtime;

const EP_SLOT: u64 = 1;
const DONE_NFN_SLOT: u64 = 2;
const TCB_SLOT: u64 = 3;

const K: u64 = 100;

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

fn write_record(buf: &mut [u8], pos: &mut usize, kind: u8, hash: &[u8; 32], payload: &[u8]) {
    buf[*pos] = kind;
    *pos += 1;
    buf[*pos..*pos + 32].copy_from_slice(hash);
    *pos += 32;
    let plen = payload.len() as u32;
    buf[*pos..*pos + 4].copy_from_slice(&plen.to_le_bytes());
    *pos += 4;
    buf[*pos..*pos + payload.len()].copy_from_slice(payload);
    *pos += payload.len();
}

pub fn main() -> ! {
    sel4::debug_println!("[C9] runtime: démarrage (K={})", K);

    let ep = sel4::cap::Endpoint::from_bits(EP_SLOT);
    let done_nfn = sel4::cap::Notification::from_bits(DONE_NFN_SLOT);
    let ring_va = get_ring_va();

    unsafe { (*ring_va).data_len = 0; }

    for i in 0..K {
        let ring = unsafe { &mut *ring_va };
        let mut pos = 0usize;

        let blob_hash: [u8; 32] = [i as u8; 32];
        let header_hash: [u8; 32] = [(i as u8).wrapping_add(1); 32];
        let payload = b"C9_REOPEN_BLOB";

        // Record 0 : blob
        write_record(&mut ring.data, &mut pos, 0, &blob_hash, payload);
        // Record 1 : header (payload = blob_hash)
        write_record(&mut ring.data, &mut pos, 1, &header_hash, &blob_hash);
        // Record 2 : log_entry (vide)
        write_record(&mut ring.data, &mut pos, 2, &header_hash, &[]);

        ring.data_len = pos as u32;
        core::sync::atomic::fence(Ordering::SeqCst);

        ep.call(sel4::MessageInfo::new(0, 0, 0, 0));

        // S3 : le serveur renvoie le seq committé dans msg_regs[0]. Le vérifier
        // détecte toute divergence log/état (commit perdu ou payload rejeté → 0).
        let seq = sel4::with_ipc_buffer(|buf| buf.msg_regs()[0]);
        if seq != i + 1 {
            sel4::debug_println!(
                "[C9] runtime: divergence commit i={} seq_serveur={} (attendu {})",
                i, seq, i + 1
            );
            panic!("[C9] runtime: commit non confirmé par le serveur");
        }

        ring.data_len = 0;
    }

    sel4::debug_println!("[C9] runtime: {} commits done, signal supervisor", K);
    done_nfn.signal();
    sel4::cap::Tcb::from_bits(TCB_SLOT).tcb_suspend().unwrap();
    unreachable!()
}
