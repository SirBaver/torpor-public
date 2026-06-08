// Utilitaires mémoire seL4 AArch64 — copie de c5-redb-on-virtio/src/mem.rs

use sel4::{CapRights, ObjectBlueprint, ObjectBlueprintArch, VmAttributes, init_thread};

pub const PAGE_SIZE: usize = 4096;

pub fn map_ram_pages(
    bootinfo: &sel4::BootInfo,
    n_pages: usize,
    va_base: usize,
    next_slot: &mut usize,
) -> usize {
    let cnode = init_thread::slot::CNODE.cap();
    let cnode_abs = cnode.absolute_cptr_for_self();
    let page_bp = ObjectBlueprint::Arch(ObjectBlueprintArch::SmallPage);
    let pt_bp = ObjectBlueprint::Arch(ObjectBlueprintArch::PT);

    let (ut_ix, ut_desc) = bootinfo
        .untyped_list()
        .iter()
        .enumerate()
        .filter(|(_, d)| !d.is_device())
        .max_by_key(|(_, d)| d.size_bits())
        .expect("aucun Untyped non-device");

    let ut = bootinfo.untyped().index(ut_ix).cap();
    let paddr = ut_desc.paddr();

    let frames_base = *next_slot;
    ut.untyped_retype(&page_bp, &cnode_abs, frames_base, n_pages)
        .expect("retype SmallPages RAM échoué");
    *next_slot += n_pages;

    for i in 0..n_pages {
        let va = va_base + i * PAGE_SIZE;
        loop {
            let frame =
                init_thread::Slot::<sel4::cap_type::SmallPage>::from_index(frames_base + i).cap();
            match frame.frame_map(
                init_thread::slot::VSPACE.cap(),
                va,
                CapRights::read_write(),
                VmAttributes::default(),
            ) {
                Ok(()) => break,
                Err(_) => {
                    let pt_slot = *next_slot;
                    *next_slot += 1;
                    ut.untyped_retype(&pt_bp, &cnode_abs, pt_slot, 1)
                        .expect("retype PT (RAM) échoué");
                    init_thread::Slot::<sel4::cap_type::PT>::from_index(pt_slot)
                        .cap()
                        .pt_map(init_thread::slot::VSPACE.cap(), va, VmAttributes::default())
                        .expect("pt_map (RAM) échoué");
                }
            }
        }
    }

    paddr
}

pub fn map_device_pages_contiguous(
    bootinfo: &sel4::BootInfo,
    target_paddr: usize,
    n_pages: usize,
    va_base: usize,
    next_slot: &mut usize,
) {
    let cnode = init_thread::slot::CNODE.cap();
    let cnode_abs = cnode.absolute_cptr_for_self();
    let page_bp = ObjectBlueprint::Arch(ObjectBlueprintArch::SmallPage);
    let pt_bp = ObjectBlueprint::Arch(ObjectBlueprintArch::PT);

    let (ut_ix, ut_desc) = bootinfo
        .untyped_list()
        .iter()
        .enumerate()
        .find(|(_, d)| {
            d.is_device()
                && d.paddr() <= target_paddr
                && target_paddr + n_pages * PAGE_SIZE <= d.paddr() + (1 << d.size_bits())
        })
        .expect("aucun Untyped device couvrant la plage MMIO cible");

    let ut = bootinfo.untyped().index(ut_ix).cap();
    let ut_paddr = ut_desc.paddr();

    if ut_paddr != target_paddr {
        trim_untyped(ut, ut_paddr, target_paddr, next_slot);
    }

    let frames_base = *next_slot;
    ut.untyped_retype(&page_bp, &cnode_abs, frames_base, n_pages)
        .expect("retype SmallPages device échoué");
    *next_slot += n_pages;

    let (kern_ut_ix, _) = bootinfo
        .untyped_list()
        .iter()
        .enumerate()
        .filter(|(_, d)| !d.is_device())
        .max_by_key(|(_, d)| d.size_bits())
        .expect("aucun Untyped kernel");
    let kern_ut = bootinfo.untyped().index(kern_ut_ix).cap();

    for i in 0..n_pages {
        let va = va_base + i * PAGE_SIZE;
        loop {
            let frame =
                init_thread::Slot::<sel4::cap_type::SmallPage>::from_index(frames_base + i).cap();
            match frame.frame_map(
                init_thread::slot::VSPACE.cap(),
                va,
                CapRights::read_write(),
                VmAttributes::default(),
            ) {
                Ok(()) => break,
                Err(_) => {
                    let pt_slot = *next_slot;
                    *next_slot += 1;
                    kern_ut
                        .untyped_retype(&pt_bp, &cnode.absolute_cptr_for_self(), pt_slot, 1)
                        .expect("retype PT (device) échoué");
                    init_thread::Slot::<sel4::cap_type::PT>::from_index(pt_slot)
                        .cap()
                        .pt_map(init_thread::slot::VSPACE.cap(), va, VmAttributes::default())
                        .expect("pt_map (device) échoué");
                }
            }
        }
    }
}

fn trim_untyped(
    ut: sel4::cap::Untyped,
    ut_paddr: usize,
    target_paddr: usize,
    next_slot: &mut usize,
) {
    let cnode = init_thread::slot::CNODE.cap();
    let cnode_abs = cnode.absolute_cptr_for_self();

    let slot_a = *next_slot;
    let slot_b = *next_slot + 1;
    *next_slot += 2;

    let rel_a = cnode.absolute_cptr(
        init_thread::Slot::<sel4::cap_type::Unspecified>::from_index(slot_a).cptr(),
    );
    let rel_b = cnode.absolute_cptr(
        init_thread::Slot::<sel4::cap_type::Unspecified>::from_index(slot_b).cptr(),
    );

    let mut cur_paddr = ut_paddr;
    while cur_paddr != target_paddr {
        let size_bits: usize = (target_paddr - cur_paddr).ilog2().try_into().unwrap();
        ut.untyped_retype(&ObjectBlueprint::Untyped { size_bits }, &cnode_abs, slot_b, 1)
            .unwrap();
        let _ = rel_a.delete();
        rel_a.move_(&rel_b).unwrap();
        cur_paddr += 1 << size_bits;
    }
}
