//! Jalon C.8 — Server process avec backend redb sur virtio-blk
//!
//! Architecture (ADR-0038 §Q3-C, ADR-0045) :
//!   Le journal append-only Q3-C est implémenté via une transaction redb :
//!   - Écriture blobs (content-addressed) → TABLE_BLOBS
//!   - Écriture header (content-addressed) → TABLE_HEADERS
//!   - Écriture journal + seq  → TABLE_JOURNAL_A/B + TABLE_SEQ  [ATOMIQUE via wtx.commit()]
//!   wtx.commit() est l'équivalent de l'append log_entry atomique d'ADR-0038 §Q3-C.
//!
//! Durabilité : niveau 1 (ADR-0045 §Q2=α, ADR-0038 §Q2).
//!   sync_data() est un no-op — ack Committed ≠ flush media.
//!
//! Topologie caps :
//!   Slot 1 : endpoint (read_write)
//!   Slot 2 : own TCB cap
//!
//! VSpace layout :
//!   ELF segments + IPC buffer + ring_a + ring_b     (footprint standard)
//!   0x1000_0000  DMA frames (16 pages, mappés par supervisor)
//!   0x2000_0000  MMIO scan (4 pages device, mappés par supervisor)
//!
//! Badges :
//!   AGENT_A_ID=1 → commit sur ring[0]
//!   AGENT_B_ID=2 → commit sur ring[1]
//!   ORACLE_BADGE=0xC8FE → retourner (seq_a, seq_b)
//!   INIT_BADGE=0xC8_0000 → initialiser hardware + redb, reply ready

#![no_std]
#![no_main]

extern crate alloc;

use alloc::vec::Vec;
use core::fmt;
use core::ptr::NonNull;

use sel4::CapTypeForFrameObjectOfFixedSize;
use sel4_virtio_hal_impl::HalImpl;
use spin::Mutex;
use virtio_drivers::{
    device::blk::VirtIOBlk,
    transport::{
        DeviceType, Transport,
        mmio::{MmioTransport, VirtIOHeader},
    },
};

use redb::{Database, ReadableDatabase, ReadableTable, StorageBackend, TableDefinition};
use redb::io;

mod runtime;

// ── Cap layout ────────────────────────────────────────────────────────────────
const EP_SLOT: u64 = 1;

// ── Badges ────────────────────────────────────────────────────────────────────
const AGENT_A_ID: u64 = 1;
const AGENT_B_ID: u64 = 2;
const ORACLE_BADGE: u64 = 0xC8FE;
const INIT_BADGE: u64 = 0xC8_0000;

// ── Layout mémoire (identique C.4/C.5) ───────────────────────────────────────
const GRANULE_SIZE: usize = sel4::cap_type::Granule::FRAME_OBJECT_TYPE.bytes();
const DMA_VA_BASE: usize = 0x1000_0000;
const DMA_PAGES: usize = 16;
const DMA_SIZE: usize = DMA_PAGES * GRANULE_SIZE;
const SCAN_VA: usize = 0x2000_0000;
const VIRTIO_MMIO_STRIDE: usize = 0x200;
const VIRTIO_MMIO_COUNT: usize = 32;
const SECTOR_BYTES: u64 = 512;

// ── redb tables ───────────────────────────────────────────────────────────────
// Blobs + headers content-addressed (hash → data)
const TABLE_BLOBS: TableDefinition<&[u8], &[u8]> = TableDefinition::new("blobs");
const TABLE_HEADERS: TableDefinition<&[u8], &[u8]> = TableDefinition::new("headers");
// Journal par agent : seq → header_hash (32 bytes)
const TABLE_JOURNAL_A: TableDefinition<u64, &[u8]> = TableDefinition::new("journal_a");
const TABLE_JOURNAL_B: TableDefinition<u64, &[u8]> = TableDefinition::new("journal_b");
// Compteur de séquence par agent : agent_id → next_seq
const TABLE_SEQ: TableDefinition<u64, u64> = TableDefinition::new("seq");

// ── Ring buffer (partagé avec le runtime) ─────────────────────────────────────
#[repr(C)]
struct RingBuffer {
    data_len: u32,
    data: [u8; 4092],
}

fn get_ring_va_for_agent(ring_index: usize) -> *mut RingBuffer {
    unsafe extern "C" {
        static _end: usize;
    }
    let ipc_buf = (core::ptr::addr_of!(_end) as usize).next_multiple_of(GRANULE_SIZE);
    (ipc_buf + (1 + ring_index) * GRANULE_SIZE) as *mut RingBuffer
}

// ── BlockStorage sur VirtIOBlk ────────────────────────────────────────────────
type StaticBlk = VirtIOBlk<HalImpl, MmioTransport<'static>>;

struct BlockStorageInner {
    blk: StaticBlk,
    logical_len: u64,
}

struct BlockStorage {
    inner: Mutex<BlockStorageInner>,
    capacity_bytes: u64,
}

impl BlockStorage {
    fn new(blk: StaticBlk) -> Self {
        let capacity_bytes = blk.capacity() * SECTOR_BYTES;
        Self {
            inner: Mutex::new(BlockStorageInner { blk, logical_len: 0 }),
            capacity_bytes,
        }
    }
}

impl fmt::Debug for BlockStorage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "BlockStorage({}MB)", self.capacity_bytes / (1024 * 1024))
    }
}

// Safety : root task seL4 single-threaded (server VSpace isolé).
unsafe impl Send for BlockStorage {}
unsafe impl Sync for BlockStorage {}

impl StorageBackend for BlockStorage {
    fn len(&self) -> core::result::Result<u64, io::Error> {
        Ok(self.inner.lock().logical_len)
    }

    fn read(&self, offset: u64, out: &mut [u8]) -> core::result::Result<(), io::Error> {
        if out.is_empty() {
            return Ok(());
        }
        let sector_start = (offset / SECTOR_BYTES) as usize;
        let end = offset + out.len() as u64;
        let sector_end = ((end + SECTOR_BYTES - 1) / SECTOR_BYTES) as usize;
        let n_sectors = sector_end - sector_start;

        if offset % SECTOR_BYTES == 0 && out.len() % SECTOR_BYTES as usize == 0 {
            return self
                .inner
                .lock()
                .blk
                .read_blocks(sector_start, out)
                .map_err(|_| io::Error::new(io::ErrorKind::Other, "virtio read echoue"));
        }
        let mut buf = alloc::vec![0u8; n_sectors * SECTOR_BYTES as usize];
        self.inner.lock().blk.read_blocks(sector_start, &mut buf)
            .map_err(|_| io::Error::new(io::ErrorKind::Other, "virtio read (unaligned) echoue"))?;
        let skip = (offset - sector_start as u64 * SECTOR_BYTES) as usize;
        out.copy_from_slice(&buf[skip..skip + out.len()]);
        Ok(())
    }

    fn set_len(&self, len: u64) -> core::result::Result<(), io::Error> {
        if len > self.capacity_bytes {
            return Err(io::Error::new(io::ErrorKind::Other, "disque plein"));
        }
        self.inner.lock().logical_len = len;
        Ok(())
    }

    fn sync_data(&self) -> core::result::Result<(), io::Error> {
        // Durabilité niveau 1 (ADR-0045 §Q2=α, ADR-0038 §Q2) : no-op.
        // L'ack Committed ne garantit PAS le flush media.
        Ok(())
    }

    fn write(&self, offset: u64, data: &[u8]) -> core::result::Result<(), io::Error> {
        if data.is_empty() {
            return Ok(());
        }
        let sector_start = (offset / SECTOR_BYTES) as usize;
        let end = offset + data.len() as u64;
        let sector_end = ((end + SECTOR_BYTES - 1) / SECTOR_BYTES) as usize;
        let n_sectors = sector_end - sector_start;

        if offset % SECTOR_BYTES == 0 && data.len() % SECTOR_BYTES as usize == 0 {
            return self
                .inner
                .lock()
                .blk
                .write_blocks(sector_start, data)
                .map_err(|_| io::Error::new(io::ErrorKind::Other, "virtio write echoue"));
        }
        let mut buf = alloc::vec![0u8; n_sectors * SECTOR_BYTES as usize];
        {
            let mut inner = self.inner.lock();
            inner.blk.read_blocks(sector_start, &mut buf)
                .map_err(|_| io::Error::new(io::ErrorKind::Other, "virtio RMW-read echoue"))?;
            let skip = (offset - sector_start as u64 * SECTOR_BYTES) as usize;
            buf[skip..skip + data.len()].copy_from_slice(data);
            inner
                .blk
                .write_blocks(sector_start, &buf)
                .map_err(|_| io::Error::new(io::ErrorKind::Other, "virtio RMW-write echoue"))
        }
    }
}

// ── Helpers virtio-blk ────────────────────────────────────────────────────────
fn find_virtio_blk() -> Option<usize> {
    for i in 0..VIRTIO_MMIO_COUNT {
        let slot_va = SCAN_VA + i * VIRTIO_MMIO_STRIDE;
        let magic = unsafe { core::ptr::read_volatile(slot_va as *const u32) };
        if magic != 0x7472_6976 {
            continue;
        }
        let device_id = unsafe { core::ptr::read_volatile((slot_va + 8) as *const u32) };
        if device_id == 2 {
            return Some(slot_va);
        }
    }
    None
}

// ── Records du ring buffer ────────────────────────────────────────────────────
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

// ── Commit atomique vers redb (ADR-0038 §Q3-C) ──────────────────────────────
//
// Séquence Q3-C dans une seule transaction redb :
//   1. Écrire blobs (content-addressed, pas d'atomicité requise individuellement)
//   2. Écrire header (content-addressed)
//   3. Écrire journal entry + incrémenter seq  ← point atomique
//   wtx.commit() = l'opération atomique unique (P6)
fn commit_to_redb(db: &Database, agent_id: u64, ring_va: *mut RingBuffer) -> u64 {
    use core::sync::atomic::Ordering;
    core::sync::atomic::fence(Ordering::SeqCst);

    let ring = unsafe { &*ring_va };
    let data_len = ring.data_len as usize;

    if data_len == 0 || data_len > 4092 {
        return 0;
    }

    let data = &ring.data[..data_len];
    let records = parse_records(data);

    let wtx = db.begin_write().unwrap();

    // Bloc : toutes les refs de table sont droppées avant wtx.commit()
    let committed_seq = {
        let mut blobs_tbl = wtx.open_table(TABLE_BLOBS).unwrap();
        let mut hdrs_tbl = wtx.open_table(TABLE_HEADERS).unwrap();
        let mut journal_a_tbl = wtx.open_table(TABLE_JOURNAL_A).unwrap();
        let mut journal_b_tbl = wtx.open_table(TABLE_JOURNAL_B).unwrap();
        let mut seq_tbl = wtx.open_table(TABLE_SEQ).unwrap();

        let mut header_hash_opt: Option<[u8; 32]> = None;

        for (kind, hash, payload) in &records {
            match kind {
                0 => {
                    blobs_tbl.insert(hash.as_slice(), payload.as_slice()).unwrap();
                }
                1 => {
                    header_hash_opt = Some(*hash);
                    hdrs_tbl.insert(hash.as_slice(), payload.as_slice()).unwrap();
                }
                2 => {
                    if let Some(hh) = header_hash_opt {
                        let seq = seq_tbl
                            .get(agent_id)
                            .unwrap()
                            .map(|v| v.value())
                            .unwrap_or(0);
                        // Journal entry + seq_count dans la MÊME txn → wtx.commit() = P6
                        if agent_id == AGENT_A_ID {
                            journal_a_tbl.insert(seq, hh.as_slice()).unwrap();
                        } else {
                            journal_b_tbl.insert(seq, hh.as_slice()).unwrap();
                        }
                        seq_tbl.insert(agent_id, seq + 1).unwrap();
                    }
                }
                _ => {}
            }
        }

        // Seq après éventuelle mise à jour
        seq_tbl.get(agent_id).unwrap().map(|v| v.value()).unwrap_or(0)
        // ← tables droppées ici
    };

    wtx.commit().unwrap();
    committed_seq
}

fn oracle_query(db: &Database) -> (u64, u64) {
    let rtx = db.begin_read().unwrap();
    let seq_tbl = rtx.open_table(TABLE_SEQ).unwrap();
    let seq_a = seq_tbl.get(AGENT_A_ID).unwrap().map(|v| v.value()).unwrap_or(0);
    let seq_b = seq_tbl.get(AGENT_B_ID).unwrap().map(|v| v.value()).unwrap_or(0);
    (seq_a, seq_b)
}

// ── Entrée principale ─────────────────────────────────────────────────────────
pub fn main() -> ! {
    sel4::debug_println!("[C8] server: démarrage");

    let ep = sel4::cap::Endpoint::from_bits(EP_SLOT);

    // ── Init IPC : recevoir dma_paddr du superviseur ──────────────────────────
    let (_msg, badge) = ep.recv(());
    assert_eq!(badge, INIT_BADGE, "[C8] server: badge init inattendu");

    let dma_paddr = sel4::with_ipc_buffer(|buf| buf.msg_regs()[0] as usize);
    sel4::debug_println!("[C8] server: dma_paddr=0x{:08x}", dma_paddr);

    // ── Init virtio-blk ───────────────────────────────────────────────────────
    // Safety : DMA_VA_BASE mappé dans ce VSpace par le superviseur
    HalImpl::init(DMA_SIZE, DMA_VA_BASE, dma_paddr);

    let blk_va = find_virtio_blk().expect("[C8] server: aucun virtio-blk dans les 32 slots");
    sel4::debug_println!("[C8] server: virtio-blk trouvé à VA=0x{:08x}", blk_va);

    let header = NonNull::new(blk_va as *mut VirtIOHeader).unwrap();
    let transport_local =
        unsafe { MmioTransport::new(header, VIRTIO_MMIO_STRIDE) }.expect("MmioTransport::new");
    assert_eq!(transport_local.device_type(), DeviceType::Block);
    let transport: MmioTransport<'static> = unsafe { core::mem::transmute(transport_local) };
    let blk = VirtIOBlk::<HalImpl, _>::new(transport).expect("VirtIOBlk::new");
    sel4::debug_println!(
        "[C8] server: virtio-blk {} secteurs ({} MB)",
        blk.capacity(), blk.capacity() / 2048
    );

    // ── Ouvrir redb sur le block device ──────────────────────────────────────
    let storage = BlockStorage::new(blk);
    let db = Database::builder()
        .set_cache_size(1024 * 1024) // 1 MB cache
        .create_with_backend(storage)
        .expect("[C8] server: Database::create_with_backend echoue");
    sel4::debug_println!("[C8] server: redb ouvert (cache 1MB)");

    sel4::debug_println!(
        "[C8] server: ring_a VA={:#x}, ring_b VA={:#x}",
        get_ring_va_for_agent(0) as usize,
        get_ring_va_for_agent(1) as usize,
    );

    // Reply "ready" au superviseur
    sel4::with_ipc_buffer_mut(|buf| {
        sel4::reply(buf, sel4::MessageInfo::new(0, 0, 0, 0));
    });

    // ── Boucle commit ─────────────────────────────────────────────────────────
    loop {
        let (_msg_info, badge) = ep.recv(());

        if badge == ORACLE_BADGE {
            let (seq_a, seq_b) = oracle_query(&db);
            sel4::debug_println!(
                "[C8] server: oracle → seq_a={}, seq_b={}",
                seq_a, seq_b
            );
            sel4::with_ipc_buffer_mut(|buf| {
                buf.msg_regs_mut()[0] = seq_a;
                buf.msg_regs_mut()[1] = seq_b;
                sel4::reply(buf, sel4::MessageInfo::new(0, 0, 0, 2));
            });
            continue;
        }

        // Dispatch commit par badge = agent_id (ADR-0044 §D2)
        let (agent_id, ring_va) = if badge == AGENT_A_ID {
            (AGENT_A_ID, get_ring_va_for_agent(0))
        } else if badge == AGENT_B_ID {
            (AGENT_B_ID, get_ring_va_for_agent(1))
        } else {
            sel4::debug_println!("[C8] server: badge inconnu {}, ignoré", badge);
            sel4::with_ipc_buffer_mut(|buf| {
                sel4::reply(buf, sel4::MessageInfo::new(0, 0, 0, 0));
            });
            continue;
        };

        let new_seq = commit_to_redb(&db, agent_id, ring_va);
        sel4::debug_println!(
            "[C8] server: agent={} → seq={} (redb commit)",
            agent_id, new_seq
        );

        sel4::with_ipc_buffer_mut(|buf| {
            sel4::reply(buf, sel4::MessageInfo::new(0, 0, 0, 0));
        });
    }
}
