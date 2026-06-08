// ObjectAllocator — C.8 extension : alloue frames DMA + frames device MMIO
//
// allocate_dma_frames : DOIT être appelé en PREMIER (avant tout autre alloc)
//   pour que paddr = ut_paddr (watermark untyped = 0).
//
// allocate_device_frames : extrait des caps de frames device depuis l'Untyped
//   device qui couvre target_paddr, avec trim si nécessaire.

use alloc::vec::Vec;
use core::ops::Range;

pub(crate) struct ObjectAllocator {
    empty_slots: Range<usize>,
    ut: sel4::cap::Untyped,
}

impl ObjectAllocator {
    pub(crate) fn new(bootinfo: &sel4::BootInfoPtr) -> Self {
        Self {
            empty_slots: bootinfo.empty().range(),
            ut: find_largest_kernel_untyped(bootinfo),
        }
    }

    pub(crate) fn allocate(&mut self, blueprint: sel4::ObjectBlueprint) -> sel4::cap::Unspecified {
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

    pub(crate) fn allocate_fixed_sized<T: sel4::CapTypeForObjectOfFixedSize>(
        &mut self,
    ) -> sel4::Cap<T> {
        self.allocate(T::object_blueprint()).cast()
    }

    pub(crate) fn allocate_variable_sized<T: sel4::CapTypeForObjectOfVariableSize>(
        &mut self,
        size_bits: usize,
    ) -> sel4::Cap<T> {
        self.allocate(T::object_blueprint(size_bits)).cast()
    }

    /// Réserve 1 slot sans retype — pour les copies de cap
    pub(crate) fn next_slot(&mut self) -> usize {
        self.empty_slots.next().unwrap()
    }

    /// Réserve n slots consécutifs sans retype — retourne l'index du premier.
    pub(crate) fn next_slots_batch(&mut self, n: usize) -> usize {
        let base = self.empty_slots.next().unwrap();
        for _ in 1..n {
            self.empty_slots.next().unwrap();
        }
        base
    }

    /// Alloue n_frames SmallPages DMA depuis le MÊME Untyped que ce allocateur.
    ///
    /// CONTRAINTE : appelé en PREMIER avant toute autre allocation — le
    /// watermark de l'Untyped est alors 0 et paddr = ut_paddr exactement.
    ///
    /// Retourne (Vec<Granule>, paddr_premier_frame).
    pub(crate) fn allocate_dma_frames_first(
        &mut self,
        bootinfo: &sel4::BootInfoPtr,
        n_frames: usize,
    ) -> (Vec<sel4::cap::Granule>, usize) {
        // Récupérer le paddr de l'Untyped utilisé par ce allocateur
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

    /// Alloue n_frames SmallPage device depuis l'Untyped device couvrant
    /// [target_paddr .. target_paddr + n_frames×4KB).
    /// Trim l'Untyped si nécessaire (target_paddr > ut_paddr).
    ///
    /// Retourne Vec<Granule> (device frames, non mappés).
    pub(crate) fn allocate_device_frames(
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

    /// Trim un Untyped de ut_paddr vers target_paddr en retypant des blocs
    /// Untyped de taille décroissante (algorithme identique à C.4/C.5).
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
