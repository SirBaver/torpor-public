//! Jalon C.5 — redb no_std sur sel4-virtio-blk (ADR-0042)
//!
//! Protocole :
//!   1. Init DMA + driver virtio-blk (même séquence que C.4)
//!   2. Ouvrir une DB redb via BlockStorage (StorageBackend custom)
//!   3. Insérer N_INSERT=1000 entrées (u64 → &[u8; 32])
//!   4. Vérifier N_VERIFY=100 entrées par echantillonnage déterministe
//!   PASS : "C5_PASS" sur UART. FAIL : panic.

#![no_std]
#![no_main]

extern crate alloc;

use core::fmt;
use core::ptr::NonNull;

use sel4::BootInfoPtr;
use sel4_root_task::{root_task, Never};
use sel4_virtio_hal_impl::HalImpl;
use spin::Mutex;
use virtio_drivers::{
    device::blk::VirtIOBlk,
    transport::{
        DeviceType, Transport,
        mmio::{MmioTransport, VirtIOHeader},
    },
};

use redb::{Database, ReadableDatabase, StorageBackend, TableDefinition};
use redb::io;

mod mem;
use mem::{PAGE_SIZE, map_device_pages_contiguous, map_ram_pages};

// ── Layout mémoire (identique à C.4) ──────────────────────────────────────────

const DMA_VA_BASE: usize = 0x1000_0000;
const DMA_PAGES: usize = 16;
const DMA_SIZE: usize = DMA_PAGES * PAGE_SIZE;

const SCAN_VA: usize = 0x2000_0000;
const VIRTIO_MMIO_BASE_PHYS: usize = 0x0a00_0000;
const VIRTIO_MMIO_STRIDE: usize = 0x200;
const VIRTIO_MMIO_COUNT: usize = 32;
const VIRTIO_MMIO_PAGES: usize = 4;

// ── BlockStorage : redb::StorageBackend sur VirtIOBlk ─────────────────────────

const SECTOR_BYTES: u64 = 512;

// Le MMIO est mappé une fois pour toute la durée du programme → 'static correct.
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

// Safety : root task seL4 single-threaded — aucun accès concurrent réel.
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
            // Chemin aligné : lecture directe
            return self.inner.lock().blk.read_blocks(sector_start, out)
                .map_err(|_| io::Error::new(io::ErrorKind::Other, "virtio read échoué"));
        }
        // Chemin non-aligné : lire dans un buffer secteur, copier les octets voulus.
        // redb fait des accès non-alignés pour ses structures internes (ex. header 320 B).
        let mut buf = alloc::vec![0u8; n_sectors * SECTOR_BYTES as usize];
        self.inner.lock().blk.read_blocks(sector_start, &mut buf)
            .map_err(|_| io::Error::new(io::ErrorKind::Other, "virtio read (unaligned) échoué"))?;
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
        // Durabilité niveau 1 (ADR-0038) : virtio polling = synchrone → no-op.
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
            // Chemin aligné : écriture directe
            return self.inner.lock().blk.write_blocks(sector_start, data)
                .map_err(|_| io::Error::new(io::ErrorKind::Other, "virtio write échoué"));
        }
        // Chemin non-aligné : lire les secteurs affectés, patcher, réécrire.
        let mut buf = alloc::vec![0u8; n_sectors * SECTOR_BYTES as usize];
        {
            let mut inner = self.inner.lock();
            inner.blk.read_blocks(sector_start, &mut buf)
                .map_err(|_| io::Error::new(io::ErrorKind::Other, "virtio RMW-read échoué"))?;
            let skip = (offset - sector_start as u64 * SECTOR_BYTES) as usize;
            buf[skip..skip + data.len()].copy_from_slice(data);
            inner.blk.write_blocks(sector_start, &buf)
                .map_err(|_| io::Error::new(io::ErrorKind::Other, "virtio RMW-write échoué"))
        }
    }
}

// ── Protocole C.5 ─────────────────────────────────────────────────────────────

const TABLE: TableDefinition<u64, &[u8]> = TableDefinition::new("data");
const N_INSERT: u64 = 1000;
const N_VERIFY: u64 = 100;

#[root_task(heap_size = 8 * 1024 * 1024)]
fn main(bootinfo: &BootInfoPtr) -> sel4::Result<Never> {
    sel4::debug_println!("=== OS-pour-IA : root task seL4 C.5 ===");
    sel4::debug_println!("    redb no_std sur virtio-blk (ADR-0042)");

    let mut next_slot = bootinfo.empty().start();

    // Étape 1 : DMA + virtio-blk (identique à C.4)
    let dma_paddr = map_ram_pages(bootinfo, DMA_PAGES, DMA_VA_BASE, &mut next_slot);
    HalImpl::init(DMA_SIZE, DMA_VA_BASE, dma_paddr);
    sel4::debug_println!("DMA: {}KB à paddr=0x{:08x}", DMA_SIZE / 1024, dma_paddr);

    map_device_pages_contiguous(
        bootinfo,
        VIRTIO_MMIO_BASE_PHYS,
        VIRTIO_MMIO_PAGES,
        SCAN_VA,
        &mut next_slot,
    );

    let blk_vaddr = find_virtio_blk().expect("aucun device virtio-blk parmi les 32 slots");
    let header = NonNull::new(blk_vaddr as *mut VirtIOHeader).unwrap();
    let transport_local =
        unsafe { MmioTransport::new(header, VIRTIO_MMIO_STRIDE) }.expect("MmioTransport::new");
    assert_eq!(transport_local.device_type(), DeviceType::Block);
    // Safety: le MMIO est mappé à une adresse fixe valide pour toute la durée du programme.
    let transport: MmioTransport<'static> = unsafe { core::mem::transmute(transport_local) };
    let blk: StaticBlk = VirtIOBlk::<HalImpl, _>::new(transport).expect("VirtIOBlk::new");
    sel4::debug_println!("virtio-blk: {} secteurs ({} MB)", blk.capacity(), blk.capacity() / 2048);

    // Étape 2 : ouvrir redb sur le block device
    sel4::debug_println!("Ouverture redb...");
    let storage = BlockStorage::new(blk);
    let db = Database::builder()
        .set_cache_size(1024 * 1024) // 1 MB — budget heap 8 MB - 1 MB overhead
        .create_with_backend(storage)
        .expect("Database::create_with_backend echoue");
    sel4::debug_println!("redb: DB ouverte (cache 1MB)");

    // Étape 3 : insérer N_INSERT entrées
    sel4::debug_println!("Insertion {} entrees...", N_INSERT);
    {
        let wtx = db.begin_write().expect("begin_write");
        {
            let mut table = wtx.open_table(TABLE).expect("open_table ecriture");
            for i in 0..N_INSERT {
                let val = make_val(i);
                table.insert(i, val.as_slice()).expect("insert");
            }
        }
        wtx.commit().expect("commit");
    }
    sel4::debug_println!("Insertion terminee");

    // Étape 4 : vérifier N_VERIFY entrées par echantillonnage déterministe
    sel4::debug_println!("Verification {} entrees...", N_VERIFY);
    {
        let rtx = db.begin_read().expect("begin_read");
        let table = rtx.open_table(TABLE).expect("open_table lecture");
        for j in 0..N_VERIFY {
            let key = (j * (N_INSERT / N_VERIFY)) % N_INSERT;
            let expected = make_val(key);
            let guard = table.get(key).expect("get").expect("cle absente");
            assert_eq!(guard.value(), expected.as_slice(), "divergence cle={}", key);
        }
    }
    sel4::debug_println!("Integrite: {} lectures correctes", N_VERIFY);
    sel4::debug_println!("C5_PASS");

    sel4::init_thread::suspend_self()
}

fn make_val(key: u64) -> alloc::vec::Vec<u8> {
    let mut v = alloc::vec![0u8; 32];
    v[..8].copy_from_slice(&key.to_le_bytes());
    v[8..16].copy_from_slice(&(!key).to_le_bytes());
    v
}

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
