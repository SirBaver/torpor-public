// object_allocator.rs — Crate commune C.10 (identique à c8-store)

use alloc::vec::Vec;
use core::ops::Range;

pub struct ObjectAllocator {
    empty_slots: Range<usize>,
    ut: sel4::cap::Untyped,
}

impl ObjectAllocator {
    pub fn new(bootinfo: &sel4::BootInfoPtr) -> Self {
        Self {
            empty_slots: bootinfo.empty().range(),
            ut: find_largest_kernel_untyped(bootinfo),
        }
    }

    pub fn allocate(&mut self, blueprint: sel4::ObjectBlueprint) -> sel4::cap::Unspecified {
        let slot_index = self.empty_slots.next().unwrap();
        self.ut
            .untyped_retype(
                &blueprint,
                &sel4::init_thread::slot::CNODE
                    .cap()
                    .absolute_cptr_for_self(),
                slot_index,
                1,
            )
            .unwrap();
        sel4::init_thread::Slot::from_index(slot_index).cap()
    }

    pub fn allocate_fixed_sized<T: sel4::CapTypeForObjectOfFixedSize>(
        &mut self,
    ) -> sel4::Cap<T> {
        self.allocate(T::object_blueprint()).cast()
    }

    pub fn allocate_variable_sized<T: sel4::CapTypeForObjectOfVariableSize>(
        &mut self,
        size_bits: usize,
    ) -> sel4::Cap<T> {
        self.allocate(T::object_blueprint(size_bits)).cast()
    }

    pub fn next_slot(&mut self) -> usize {
        self.empty_slots.next().unwrap()
    }

    pub fn next_slots_batch(&mut self, n: usize) -> usize {
        let base = self.empty_slots.next().unwrap();
        for _ in 1..n {
            self.empty_slots.next().unwrap();
        }
        base
    }

    pub fn ut_ref(&self) -> sel4::cap::Untyped {
        self.ut
    }

    /// Alloue n_frames SmallPages non-DMA (sans garantie de paddr contigu avec ut_paddr).
    /// À utiliser après allocate_dma_frames_first.
    pub fn allocate_frames_batch(&mut self, n_frames: usize) -> Vec<sel4::cap::Granule> {
        let cnode = sel4::init_thread::slot::CNODE.cap();
        let base_slot = self.next_slots_batch(n_frames);
        self.ut
            .untyped_retype(
                &sel4::ObjectBlueprint::Arch(sel4::ObjectBlueprintArch::SmallPage),
                &cnode.absolute_cptr_for_self(),
                base_slot,
                n_frames,
            )
            .unwrap();
        (0..n_frames)
            .map(|i| {
                sel4::init_thread::Slot::<sel4::cap_type::Unspecified>::from_index(base_slot + i)
                    .cap()
                    .cast::<sel4::cap_type::Granule>()
            })
            .collect()
    }

    pub fn allocate_dma_frames_first(
        &mut self,
        bootinfo: &sel4::BootInfoPtr,
        n_frames: usize,
    ) -> (Vec<sel4::cap::Granule>, usize) {
        let (_, ut_desc) = bootinfo
            .untyped_list()
            .iter()
            .enumerate()
            .filter(|(_, d)| !d.is_device())
            .max_by_key(|(_, d)| d.size_bits())
            .unwrap();
        let dma_paddr = ut_desc.paddr();

        let cnode = sel4::init_thread::slot::CNODE.cap();
        let base_slot = self.next_slots_batch(n_frames);

        self.ut
            .untyped_retype(
                &sel4::ObjectBlueprint::Arch(sel4::ObjectBlueprintArch::SmallPage),
                &cnode.absolute_cptr_for_self(),
                base_slot,
                n_frames,
            )
            .unwrap();

        let frames = (0..n_frames)
            .map(|i| {
                sel4::init_thread::Slot::<sel4::cap_type::Unspecified>::from_index(base_slot + i)
                    .cap()
                    .cast::<sel4::cap_type::Granule>()
            })
            .collect();

        (frames, dma_paddr)
    }

    pub fn allocate_device_frames(
        &mut self,
        bootinfo: &sel4::BootInfoPtr,
        target_paddr: usize,
        n_frames: usize,
    ) -> Vec<sel4::cap::Granule> {
        const PAGE_SIZE: usize = 4096;

        let (ut_ix, ut_desc) = bootinfo
            .untyped_list()
            .iter()
            .enumerate()
            .find(|(_, d)| {
                d.is_device()
                    && d.paddr() <= target_paddr
                    && target_paddr + n_frames * PAGE_SIZE <= d.paddr() + (1 << d.size_bits())
            })
            .expect("aucun Untyped device couvrant la plage MMIO cible");

        let ut = bootinfo.untyped().index(ut_ix).cap();
        let ut_paddr = ut_desc.paddr();

        if ut_paddr < target_paddr {
            self.trim_untyped(ut, ut_paddr, target_paddr);
        }

        let cnode = sel4::init_thread::slot::CNODE.cap();
        let base_slot = self.next_slots_batch(n_frames);

        ut.untyped_retype(
            &sel4::ObjectBlueprint::Arch(sel4::ObjectBlueprintArch::SmallPage),
            &cnode.absolute_cptr_for_self(),
            base_slot,
            n_frames,
        )
        .unwrap();

        (0..n_frames)
            .map(|i| {
                sel4::init_thread::Slot::<sel4::cap_type::Unspecified>::from_index(base_slot + i)
                    .cap()
                    .cast::<sel4::cap_type::Granule>()
            })
            .collect()
    }

    fn trim_untyped(
        &mut self,
        ut: sel4::cap::Untyped,
        ut_paddr: usize,
        target_paddr: usize,
    ) {
        let cnode = sel4::init_thread::slot::CNODE.cap();
        let cnode_abs = cnode.absolute_cptr_for_self();

        let slot_a = self.next_slot();
        let slot_b = self.next_slot();

        let rel_a = cnode.absolute_cptr(
            sel4::init_thread::Slot::<sel4::cap_type::Unspecified>::from_index(slot_a).cptr(),
        );
        let rel_b = cnode.absolute_cptr(
            sel4::init_thread::Slot::<sel4::cap_type::Unspecified>::from_index(slot_b).cptr(),
        );

        let mut cur = ut_paddr;
        while cur < target_paddr {
            let size_bits: usize = (target_paddr - cur).ilog2().try_into().unwrap();
            ut.untyped_retype(
                &sel4::ObjectBlueprint::Untyped { size_bits },
                &cnode_abs,
                slot_b,
                1,
            )
            .unwrap();
            let _ = rel_a.delete();
            rel_a.move_(&rel_b).unwrap();
            cur += 1 << size_bits;
        }
    }
}

fn find_largest_kernel_untyped(bootinfo: &sel4::BootInfoPtr) -> sel4::cap::Untyped {
    let (ut_ix, _) = bootinfo
        .untyped_list()
        .iter()
        .enumerate()
        .filter(|(_, desc)| !desc.is_device())
        .max_by_key(|(_, desc)| desc.size_bits())
        .unwrap();
    bootinfo.untyped().index(ut_ix).cap()
}
