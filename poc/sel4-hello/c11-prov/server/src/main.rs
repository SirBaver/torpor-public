//! Jalon C.11-prov — Server
//!
//! K=1 : seul le runtime valide commit (le runtime malformé retourne Err avant d'émettre).
//!
//! Phase A : commit loop (badge=AGENT_A_ID) + oracle (badge=ORACLE_BADGE).
//! Phase B : verify K=1 (badge=VERIFY_BADGE) puis suspend.

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

const EP_SLOT: u64 = 1;
const TCB_SLOT: u64 = 2;

const AGENT_A_ID: u64 = 1;
const ORACLE_BADGE: u64 = 0xC1F1FE;
const VERIFY_BADGE: u64 = 0xC1F1FF;
const INIT_BADGE: u64 = 0xC1F000;

const K: u64 = 1;

const GRANULE_SIZE: usize = sel4::cap_type::Granule::FRAME_OBJECT_TYPE.bytes();
const DMA_VA_BASE: usize = 0x1000_0000;
const DMA_PAGES: usize = 16;
const DMA_SIZE: usize = DMA_PAGES * GRANULE_SIZE;
const SCAN_VA: usize = 0x2000_0000;
const VIRTIO_MMIO_STRIDE: usize = 0x200;
const VIRTIO_MMIO_COUNT: usize = 32;
const SECTOR_BYTES: u64 = 512;

const TABLE_BLOBS: TableDefinition<&[u8], &[u8]> = TableDefinition::new("blobs");
const TABLE_HEADERS: TableDefinition<&[u8], &[u8]> = TableDefinition::new("headers");
const TABLE_JOURNAL_A: TableDefinition<u64, &[u8]> = TableDefinition::new("journal_a");
const TABLE_SEQ: TableDefinition<u64, u64> = TableDefinition::new("seq");

#[repr(C)]
struct RingBuffer {
    data_len: u32,
    data: [u8; 4092],
}

fn get_ring_va() -> *mut RingBuffer {
    unsafe extern "C" {
        static _end: usize;
    }
    let ipc_buf = (core::ptr::addr_of!(_end) as usize).next_multiple_of(GRANULE_SIZE);
    (ipc_buf + GRANULE_SIZE) as *mut RingBuffer
}

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
        Self { inner: Mutex::new(BlockStorageInner { blk, logical_len: 0 }), capacity_bytes }
    }

    fn new_reopen(blk: StaticBlk) -> Self {
        let capacity_bytes = blk.capacity() * SECTOR_BYTES;
        Self { inner: Mutex::new(BlockStorageInner { blk, logical_len: capacity_bytes }), capacity_bytes }
    }
}

impl fmt::Debug for BlockStorage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "BlockStorage({}MB)", self.capacity_bytes / (1024 * 1024))
    }
}

unsafe impl Send for BlockStorage {}
unsafe impl Sync for BlockStorage {}

impl StorageBackend for BlockStorage {
    fn len(&self) -> core::result::Result<u64, io::Error> {
        Ok(self.inner.lock().logical_len)
    }

    fn read(&self, offset: u64, out: &mut [u8]) -> core::result::Result<(), io::Error> {
        if out.is_empty() { return Ok(()); }
        let sector_start = (offset / SECTOR_BYTES) as usize;
        let end = offset + out.len() as u64;
        let sector_end = ((end + SECTOR_BYTES - 1) / SECTOR_BYTES) as usize;
        let n_sectors = sector_end - sector_start;
        if offset % SECTOR_BYTES == 0 && out.len() % SECTOR_BYTES as usize == 0 {
            return self.inner.lock().blk.read_blocks(sector_start, out)
                .map_err(|_| io::Error::new(io::ErrorKind::Other, "virtio read"));
        }
        let mut buf = alloc::vec![0u8; n_sectors * SECTOR_BYTES as usize];
        self.inner.lock().blk.read_blocks(sector_start, &mut buf)
            .map_err(|_| io::Error::new(io::ErrorKind::Other, "virtio read unaligned"))?;
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

    fn sync_data(&self) -> core::result::Result<(), io::Error> { Ok(()) }

    fn write(&self, offset: u64, data: &[u8]) -> core::result::Result<(), io::Error> {
        if data.is_empty() { return Ok(()); }
        let sector_start = (offset / SECTOR_BYTES) as usize;
        let end = offset + data.len() as u64;
        let sector_end = ((end + SECTOR_BYTES - 1) / SECTOR_BYTES) as usize;
        let n_sectors = sector_end - sector_start;
        if offset % SECTOR_BYTES == 0 && data.len() % SECTOR_BYTES as usize == 0 {
            return self.inner.lock().blk.write_blocks(sector_start, data)
                .map_err(|_| io::Error::new(io::ErrorKind::Other, "virtio write"));
        }
        let mut buf = alloc::vec![0u8; n_sectors * SECTOR_BYTES as usize];
        {
            let mut inner = self.inner.lock();
            inner.blk.read_blocks(sector_start, &mut buf)
                .map_err(|_| io::Error::new(io::ErrorKind::Other, "virtio RMW-read"))?;
            let skip = (offset - sector_start as u64 * SECTOR_BYTES) as usize;
            buf[skip..skip + data.len()].copy_from_slice(data);
            inner.blk.write_blocks(sector_start, &buf)
                .map_err(|_| io::Error::new(io::ErrorKind::Other, "virtio RMW-write"))
        }
    }
}

fn find_virtio_blk() -> Option<usize> {
    for i in 0..VIRTIO_MMIO_COUNT {
        let slot_va = SCAN_VA + i * VIRTIO_MMIO_STRIDE;
        let magic = unsafe { core::ptr::read_volatile(slot_va as *const u32) };
        if magic != 0x7472_6976 { continue; }
        let device_id = unsafe { core::ptr::read_volatile((slot_va + 8) as *const u32) };
        if device_id == 2 { return Some(slot_va); }
    }
    None
}

fn parse_records(data: &[u8]) -> Vec<(u8, [u8; 32], Vec<u8>)> {
    let mut records = Vec::new();
    let mut pos = 0usize;
    while pos + 37 <= data.len() {
        let kind = data[pos]; pos += 1;
        let mut hash = [0u8; 32];
        hash.copy_from_slice(&data[pos..pos + 32]); pos += 32;
        if pos + 4 > data.len() { break; }
        let payload_len = u32::from_le_bytes([data[pos], data[pos+1], data[pos+2], data[pos+3]]) as usize;
        pos += 4;
        if pos + payload_len > data.len() { break; }
        let payload = data[pos..pos + payload_len].to_vec();
        pos += payload_len;
        records.push((kind, hash, payload));
    }
    records
}

fn commit_to_redb(db: &Database, ring_va: *mut RingBuffer) -> u64 {
    use core::sync::atomic::Ordering;
    core::sync::atomic::fence(Ordering::SeqCst);
    let ring = unsafe { &*ring_va };
    let data_len = ring.data_len as usize;
    if data_len == 0 || data_len > 4092 {
        sel4::debug_println!("[C11prov] server: payload invalide (data_len={})", data_len);
        return 0;
    }
    let records = parse_records(&ring.data[..data_len]);
    let wtx = db.begin_write().unwrap();
    let committed_seq = {
        let mut blobs_tbl = wtx.open_table(TABLE_BLOBS).unwrap();
        let mut hdrs_tbl = wtx.open_table(TABLE_HEADERS).unwrap();
        let mut journal_tbl = wtx.open_table(TABLE_JOURNAL_A).unwrap();
        let mut seq_tbl = wtx.open_table(TABLE_SEQ).unwrap();
        let mut header_hash_opt: Option<[u8; 32]> = None;
        for (kind, hash, payload) in &records {
            match kind {
                0 => { blobs_tbl.insert(hash.as_slice(), payload.as_slice()).unwrap(); }
                1 => {
                    header_hash_opt = Some(*hash);
                    hdrs_tbl.insert(hash.as_slice(), payload.as_slice()).unwrap();
                }
                2 => {
                    if let Some(hh) = header_hash_opt {
                        let seq = seq_tbl.get(AGENT_A_ID).unwrap()
                            .map(|v| v.value()).unwrap_or(0);
                        journal_tbl.insert(seq, hh.as_slice()).unwrap();
                        seq_tbl.insert(AGENT_A_ID, seq + 1).unwrap();
                    }
                }
                _ => {}
            }
        }
        seq_tbl.get(AGENT_A_ID).unwrap().map(|v| v.value()).unwrap_or(0)
    };
    wtx.commit().unwrap();
    committed_seq
}

fn oracle_query(db: &Database) -> u64 {
    let rtx = db.begin_read().unwrap();
    match rtx.open_table(TABLE_SEQ) {
        Ok(seq_tbl) => seq_tbl.get(AGENT_A_ID).unwrap().map(|v| v.value()).unwrap_or(0),
        Err(_) => 0,
    }
}

fn verify_k_commits(db: &Database) -> (u64, u64) {
    let rtx = db.begin_read().unwrap();
    let seq_a = match rtx.open_table(TABLE_SEQ) {
        Ok(seq_tbl) => seq_tbl.get(AGENT_A_ID).unwrap().map(|v| v.value()).unwrap_or(0),
        Err(_) => 0,
    };
    let count = match rtx.open_table(TABLE_JOURNAL_A) {
        Ok(journal_tbl) => {
            let mut count = 0u64;
            for i in 0..K {
                if journal_tbl.get(i).unwrap().is_some() { count += 1; }
            }
            count
        }
        Err(_) => 0,
    };
    (count, seq_a)
}

pub fn main() -> ! {
    sel4::debug_println!("[C11prov] server: démarrage");

    let ep = sel4::cap::Endpoint::from_bits(EP_SLOT);

    let (_msg, badge) = ep.recv(());
    assert_eq!(badge, INIT_BADGE, "[C11prov] server: badge init inattendu");
    let (dma_paddr, phase) = sel4::with_ipc_buffer(|buf| {
        (buf.msg_regs()[0] as usize, buf.msg_regs()[1])
    });
    sel4::debug_println!("[C11prov] server: dma_paddr=0x{:08x} phase={}", dma_paddr, phase);

    HalImpl::init(DMA_SIZE, DMA_VA_BASE, dma_paddr);
    let blk_va = find_virtio_blk().expect("[C11prov] server: aucun virtio-blk");
    let header = NonNull::new(blk_va as *mut VirtIOHeader).unwrap();
    let transport_local = unsafe { MmioTransport::new(header, VIRTIO_MMIO_STRIDE) }.unwrap();
    assert_eq!(transport_local.device_type(), DeviceType::Block);
    let transport: MmioTransport<'static> = unsafe { core::mem::transmute(transport_local) };
    let blk = VirtIOBlk::<HalImpl, _>::new(transport).unwrap();

    let storage = if phase == 1 { BlockStorage::new_reopen(blk) } else { BlockStorage::new(blk) };
    let db = Database::builder()
        .set_cache_size(1024 * 1024)
        .create_with_backend(storage)
        .expect("[C11prov] server: Database::create_with_backend echoue");
    sel4::debug_println!("[C11prov] server: redb ouvert (phase={})", phase);

    sel4::with_ipc_buffer_mut(|buf| {
        sel4::reply(buf, sel4::MessageInfo::new(0, 0, 0, 0));
    });

    if phase == 1 {
        loop {
            let (_msg_info, badge) = ep.recv(());
            if badge == VERIFY_BADGE {
                let (verified, seq_a) = verify_k_commits(&db);
                sel4::debug_println!(
                    "[C11prov] server Phase B: verified={} seq_a={} K={}",
                    verified, seq_a, K
                );
                sel4::with_ipc_buffer_mut(|buf| {
                    buf.msg_regs_mut()[0] = verified;
                    buf.msg_regs_mut()[1] = seq_a;
                    sel4::reply(buf, sel4::MessageInfo::new(0, 0, 0, 2));
                });
                sel4::cap::Tcb::from_bits(TCB_SLOT).tcb_suspend().unwrap();
                unreachable!()
            }
        }
    }

    let ring_va = get_ring_va();
    loop {
        let (_msg_info, badge) = ep.recv(());
        if badge == ORACLE_BADGE {
            let seq_a = oracle_query(&db);
            sel4::with_ipc_buffer_mut(|buf| {
                buf.msg_regs_mut()[0] = seq_a;
                sel4::reply(buf, sel4::MessageInfo::new(0, 0, 0, 1));
            });
            continue;
        }
        if badge == AGENT_A_ID {
            let seq = commit_to_redb(&db, ring_va);
            sel4::debug_println!("[C11prov] server: commit → seq={}", seq);
            sel4::with_ipc_buffer_mut(|buf| {
                buf.msg_regs_mut()[0] = seq;
                sel4::reply(buf, sel4::MessageInfo::new(0, 0, 0, 1));
            });
        } else {
            sel4::with_ipc_buffer_mut(|buf| {
                sel4::reply(buf, sel4::MessageInfo::new(0, 0, 0, 0));
            });
        }
    }
}
