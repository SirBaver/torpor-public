//! Jalon C.7-crash — Server process
//!
//! Reçoit des commits de N agents via endpoint seL4.
//! Dispatch par badge = agent_id (ADR-0044 §D2).
//! Chaque agent a son ring dédié (SPSC, ADR-0038 §32).
//! Commit Q3-C en RAM, index par (agent_id, k) (ADR-0044).
//! Reply au runtime après chaque commit.
//! Extension crash : oracle query (badge=ORACLE_BADGE) → retourne (seq_a, seq_b)
//! pour assertion I3-N + I4 par le superviseur.
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

use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use sel4::CapTypeForFrameObjectOfFixedSize;

mod runtime;

// Cap layout dans le server CNode (child_cnode_size_bits = 2)
// Slot 0: NULL
// Slot 1: endpoint (read_write)
// Slot 2: own TCB cap
const EP_SLOT: u64 = 1;

// Agents connus (ADR-0044 §D2 : badge = agent_id pur)
const AGENT_A_ID: u64 = 1;
const AGENT_B_ID: u64 = 2;

// Badge oracle — superviseur utilise cette cap pour interroger l'état post-crash
const ORACLE_BADGE: u64 = 0xC7FE;

// Ring buffer layout (partagé avec le runtime)
#[repr(C)]
struct RingBuffer {
    data_len: u32,
    data: [u8; 4092],
}

// Q3-C store en RAM avec index per-agent (ADR-0044)
struct Q3cStore {
    blobs: Vec<([u8; 32], Vec<u8>)>,
    headers: Vec<([u8; 32], Vec<u8>)>,
    // journal_per_agent[agent_id] = liste des header_hashes committés par cet agent
    journal_per_agent: BTreeMap<u64, Vec<[u8; 32]>>,
}

impl Q3cStore {
    fn new() -> Self {
        Self {
            blobs: Vec::new(),
            headers: Vec::new(),
            journal_per_agent: BTreeMap::new(),
        }
    }

    fn commit_blob(&mut self, hash: [u8; 32], data: Vec<u8>) {
        self.blobs.push((hash, data));
    }

    fn commit_header(&mut self, hash: [u8; 32], data: Vec<u8>) {
        self.headers.push((hash, data));
    }

    fn append_journal(&mut self, agent_id: u64, header_hash: [u8; 32]) {
        self.journal_per_agent
            .entry(agent_id)
            .or_insert_with(Vec::new)
            .push(header_hash);
    }

    fn seq_for(&self, agent_id: u64) -> usize {
        self.journal_per_agent
            .get(&agent_id)
            .map(|v| v.len())
            .unwrap_or(0)
    }
}

const GRANULE_SIZE: usize = sel4::cap_type::Granule::FRAME_OBJECT_TYPE.bytes();

// VA du ring pour l'agent donné (index 0-based, 0 = agent A, 1 = agent B)
fn get_ring_va_for_agent(ring_index: usize) -> *mut RingBuffer {
    unsafe extern "C" {
        static _end: usize;
    }
    let ipc_buf = (core::ptr::addr_of!(_end) as usize).next_multiple_of(GRANULE_SIZE);
    // ring 0 à ipc_buf + G (premier ring après IPC buffer)
    // ring 1 à ipc_buf + 2G, etc.
    (ipc_buf + (1 + ring_index) * GRANULE_SIZE) as *mut RingBuffer
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

fn process_ring(store: &mut Q3cStore, agent_id: u64, ring_va: *mut RingBuffer) {
    use core::sync::atomic::Ordering;
    core::sync::atomic::fence(Ordering::SeqCst);

    let ring = unsafe { &*ring_va };
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
                    store.append_journal(agent_id, hh);
                }
            }
            _ => {}
        }
    }
}

pub fn main() -> ! {
    sel4::debug_println!("[C7-crash] server: démarrage");

    let ep = sel4::cap::Endpoint::from_bits(EP_SLOT);
    let mut store = Q3cStore::new();

    sel4::debug_println!(
        "[C7-crash] server: ring_a VA={:#x}, ring_b VA={:#x}",
        get_ring_va_for_agent(0) as usize,
        get_ring_va_for_agent(1) as usize,
    );

    // Boucle serveur : recv(badge) → dispatch → process/oracle → reply
    loop {
        let (_msg_info, badge) = ep.recv(());

        if badge == ORACLE_BADGE {
            // Oracle query : retourner (seq_a, seq_b) pour assertion I3-N + I4
            let seq_a = store.seq_for(AGENT_A_ID) as u64;
            let seq_b = store.seq_for(AGENT_B_ID) as u64;
            sel4::debug_println!(
                "[C7-crash] server: oracle → seq_a={}, seq_b={} (blobs={}, headers={})",
                seq_a, seq_b, store.blobs.len(), store.headers.len()
            );
            sel4::with_ipc_buffer_mut(|buf| {
                buf.msg_regs_mut()[0] = seq_a;
                buf.msg_regs_mut()[1] = seq_b;
                sel4::reply(buf, sel4::MessageInfo::new(0, 0, 0, 2));
            });
            continue;
        }

        // Dispatch commit : badge = agent_id
        let (agent_id, ring_va) = if badge == AGENT_A_ID {
            (AGENT_A_ID, get_ring_va_for_agent(0))
        } else if badge == AGENT_B_ID {
            (AGENT_B_ID, get_ring_va_for_agent(1))
        } else {
            sel4::debug_println!("[C7-crash] server: badge inconnu {}, ignoré", badge);
            sel4::with_ipc_buffer_mut(|buf| {
                sel4::reply(buf, sel4::MessageInfo::new(0, 0, 0, 0));
            });
            continue;
        };

        let seq_before = store.seq_for(agent_id);
        process_ring(&mut store, agent_id, ring_va);
        let seq_after = store.seq_for(agent_id);

        sel4::debug_println!(
            "[C7-crash] server: agent={} commit seq {} → {}  (blobs={}, headers={})",
            agent_id, seq_before, seq_after, store.blobs.len(), store.headers.len(),
        );

        sel4::with_ipc_buffer_mut(|buf| {
            sel4::reply(buf, sel4::MessageInfo::new(0, 0, 0, 0));
        });
    }
}
