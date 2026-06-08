// Jalon C.4 — Root task seL4 + driver virtio-blk (ADR-0041)
//
// Critères C.4 :
//   (a) HalImpl DMA initialisé (sel4-virtio-hal-impl, pas de microkit)
//   (b) Transport MMIO virtio-blk détecté par scan des 32 slots QEMU virt AArch64
//   (c) VirtIOBlk read_blocks(0) + write_blocks(0) synchrones (polling)
//   Signal de succès : "C4_PASS"
#![no_std]
#![no_main]

extern crate alloc;

use core::ptr::NonNull;

use sel4_root_task::{root_task, Never};
use virtio_drivers::{
    device::blk::VirtIOBlk,
    transport::{
        DeviceType, Transport,
        mmio::{MmioTransport, VirtIOHeader},
    },
};
use sel4_virtio_hal_impl::HalImpl;

mod mem;

use mem::{PAGE_SIZE, map_device_pages_contiguous, map_ram_pages};

// Zone VA pour la région DMA (16 pages = 64 KB, physiquement contigues)
const DMA_VA_BASE: usize = 0x1000_0000;
const DMA_PAGES: usize = 16;
const DMA_SIZE: usize = DMA_PAGES * PAGE_SIZE;

// Zone VA pour mapper les 4 pages couvrant les 32 slots virtio-mmio (32×0x200 = 0x4000 = 4 pages)
const SCAN_VA: usize = 0x2000_0000;

// QEMU virt AArch64 : base des virtio-mmio devices (spec QEMU hw/arm/virt.c)
const VIRTIO_MMIO_BASE_PHYS: usize = 0x0a00_0000;
const VIRTIO_MMIO_STRIDE: usize = 0x200;
const VIRTIO_MMIO_COUNT: usize = 32;
// 32 slots × 0x200 = 0x4000 = 4 pages de 4 KB
const VIRTIO_MMIO_PAGES: usize = 4;

#[root_task(heap_size = 2 * 1024 * 1024)]
fn main(bootinfo: &sel4::BootInfoPtr) -> sel4::Result<Never> {
    sel4::debug_println!("=== OS-pour-IA : root task seL4 C.4 ===");
    sel4::debug_println!("    Driver virtio-blk (ADR-0041)");

    let mut next_slot = bootinfo.empty().start();

    // Critère C.4a : région DMA physiquement contigüe
    let dma_paddr = map_ram_pages(bootinfo, DMA_PAGES, DMA_VA_BASE, &mut next_slot);
    sel4::debug_println!(
        "DMA: paddr=0x{:08x} vaddr=0x{:08x} size={}K",
        dma_paddr,
        DMA_VA_BASE,
        DMA_SIZE / 1024
    );
    HalImpl::init(DMA_SIZE, DMA_VA_BASE, dma_paddr);

    // Mapper les 4 pages physiques de la plage virtio-mmio (0x0a000000..0x0a004000)
    map_device_pages_contiguous(
        bootinfo,
        VIRTIO_MMIO_BASE_PHYS,
        VIRTIO_MMIO_PAGES,
        SCAN_VA,
        &mut next_slot,
    );

    // Critère C.4b : scan des 32 slots pour trouver le Block device (device_id == 2)
    let blk_vaddr = find_virtio_blk().expect("aucun device virtio-blk trouvé parmi les 32 slots");
    sel4::debug_println!("virtio-blk : slot VA=0x{:08x}", blk_vaddr);

    // Critère C.4c : transport MMIO + VirtIOBlk + read/write synchrones
    let header = NonNull::new(blk_vaddr as *mut VirtIOHeader).unwrap();
    let transport = unsafe { MmioTransport::new(header, VIRTIO_MMIO_STRIDE) }
        .expect("MmioTransport::new échoué");
    assert_eq!(transport.device_type(), DeviceType::Block);

    let mut blk = VirtIOBlk::<HalImpl, _>::new(transport).expect("VirtIOBlk::new échoué");
    sel4::debug_println!("VirtIOBlk prêt, capacité={} secteurs", blk.capacity());

    // Lecture bloc 0 (512 B)
    let mut buf = alloc::vec![0u8; 512];
    blk.read_blocks(0, &mut buf).expect("read_blocks(0) échoué");
    sel4::debug_println!(
        "  bloc 0 lu    : {:02x} {:02x} {:02x} {:02x} ...",
        buf[0], buf[1], buf[2], buf[3]
    );

    // Écriture bloc 0 avec marque C4
    buf[0] = 0xC4;
    buf[1] = 0x04;
    blk.write_blocks(0, &buf).expect("write_blocks(0) échoué");
    sel4::debug_println!("  bloc 0 écrit : marque 0xC4 0x04");

    // Relecture pour valider le round-trip
    let mut buf2 = alloc::vec![0u8; 512];
    blk.read_blocks(0, &mut buf2).expect("re-read_blocks(0) échoué");
    assert_eq!(buf2[0], 0xC4, "relecture: octet 0 incorrect");
    assert_eq!(buf2[1], 0x04, "relecture: octet 1 incorrect");
    sel4::debug_println!("  relecture    : marque C4 confirmée");

    sel4::debug_println!("C4_PASS");

    sel4::init_thread::suspend_self()
}

// Scanne la plage MMIO virtio (déjà mappée à SCAN_VA) pour trouver le premier
// slot dont magic == "virt" et device_id == 2 (Block).
fn find_virtio_blk() -> Option<usize> {
    for i in 0..VIRTIO_MMIO_COUNT {
        let slot_va = SCAN_VA + i * VIRTIO_MMIO_STRIDE;
        // Safety : la plage est mappée (device non-cacheable, MMIO QEMU)
        let magic = unsafe { core::ptr::read_volatile(slot_va as *const u32) };
        if magic != 0x7472_6976 {
            // 0x74726976 = "virt" little-endian
            continue;
        }
        let version = unsafe { core::ptr::read_volatile((slot_va + 4) as *const u32) };
        let device_id = unsafe { core::ptr::read_volatile((slot_va + 8) as *const u32) };
        sel4::debug_println!(
            "  slot {:2} : magic=OK version={} device_id={}",
            i, version, device_id
        );
        if device_id == 2 {
            return Some(slot_va);
        }
    }
    None
}
