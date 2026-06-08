//! Jalon C.6 — Server process
//!
//! Reçoit des records du ring buffer via IPC endpoint seL4.
//! Commit Q3-C en RAM (blobs + headers + journal).
//! Reply au runtime après chaque commit.
//!
//! Pattern seL4 server :
//!   1. ep.recv(()) — attend le premier message
//!   2. Traite le ring
//!   3. sel4::with_ipc_buffer_mut → sel4::reply() — reply explicite
//!   4. ep.recv(()) — attend le prochain message
//!   (boucle sur 2-4)

#![no_std]
#![no_main]

extern crate alloc;

use alloc::vec::Vec;
use sel4::CapTypeForFrameObjectOfFixedSize;

mod runtime;

// Cap layout dans le server CNode (child_cnode_size_bits = 2)
// Slot 0: NULL
// Slot 1: endpoint (read_write)
// Slot 2: own TCB cap
const EP_SLOT: u64 = 1;

// Ring buffer layout (partagé avec le runtime)
#[repr(C)]
struct RingBuffer {
    data_len: u32,
    data: [u8; 4092],
}

// Q3-C store en RAM
struct Q3cStore {
    blobs: Vec<([u8; 32], Vec<u8>)>,
    headers: Vec<([u8; 32], Vec<u8>)>,
    journal: Vec<[u8; 32]>,
}

impl Q3cStore {
    fn new() -> Self {
        Self {
            blobs: Vec::new(),
            headers: Vec::new(),
            journal: Vec::new(),
        }
    }

    fn commit_blob(&mut self, hash: [u8; 32], data: Vec<u8>) {
        self.blobs.push((hash, data));
    }

    fn commit_header(&mut self, hash: [u8; 32], data: Vec<u8>) {
        self.headers.push((hash, data));
    }

    fn append_journal(&mut self, header_hash: [u8; 32]) {
        self.journal.push(header_hash);
    }
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

// Lecture des records depuis le ring buffer
fn parse_records(data: &[u8]) -> Vec<(u8, [u8; 32], Vec<u8>)> {
    let mut records = Vec::new();
    let mut pos = 0usize;
    while pos + 37 <= data.len() {
        let kind = data[pos];
        pos += 1;
        let mut hash = [0u8; 32];
        hash.copy_from_slice(&data[pos..pos + 32]);
        pos += 32;
        if pos + 4 > data.len() {
            break;
        }
        let payload_len =
            u32::from_le_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]])
                as usize;
        pos += 4;
        if pos + payload_len > data.len() {
            break;
        }
        let payload = data[pos..pos + payload_len].to_vec();
        pos += payload_len;
        records.push((kind, hash, payload));
    }
    records
}

fn process_ring(store: &mut Q3cStore) {
    use core::sync::atomic::Ordering;
    core::sync::atomic::fence(Ordering::SeqCst);

    let ring = unsafe { &*get_ring_va() };
    let data_len = ring.data_len as usize;

    if data_len == 0 || data_len > 4092 {
        return;
    }

    let data = &ring.data[..data_len];
    let records = parse_records(data);

    let mut header_hash_opt: Option<[u8; 32]> = None;

    for (kind, hash, payload) in records {
        match kind {
            0 => {
                store.commit_blob(hash, payload);
            }
            1 => {
                header_hash_opt = Some(hash);
                store.commit_header(hash, payload);
            }
            2 => {
                if let Some(hh) = header_hash_opt {
                    store.append_journal(hh);
                }
            }
            _ => {}
        }
    }
}

pub fn main() -> ! {
    sel4::debug_println!("[C6] server: démarrage");

    let ep = sel4::cap::Endpoint::from_bits(EP_SLOT);
    let mut store = Q3cStore::new();

    // Boucle serveur : recv → process → reply → recv → ...
    loop {
        // Attendre un message
        let (_msg_info, _badge) = ep.recv(());

        // Traiter le ring buffer
        process_ring(&mut store);

        // Q3-C committed (blobs, headers, journal en RAM)

        // Reply explicite via seL4_Reply (reply cap implicite)
        sel4::with_ipc_buffer_mut(|buf| {
            sel4::reply(buf, sel4::MessageInfo::new(0, 0, 0, 0));
        });
    }
}
