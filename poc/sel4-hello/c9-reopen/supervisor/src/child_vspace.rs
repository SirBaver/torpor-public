// child_vspace.rs — Adapté du spawn-task example rust-sel4 rev 7a2321f2
//
// Crée un VSpace enfant à partir d'un image ELF.
// Mappe :
//   - Les segments ELF
//   - L'IPC buffer (page après le footprint image)
//   - N ring buffers (pages consécutives après l'IPC buffer)
//
// Extension C.8 : map_hardware_into_vspace
//   - DMA frames à DMA_VA_BASE (0x1000_0000)
//   - MMIO device frames à SCAN_VA (0x2000_0000)
//
// Retourne : (vspace, ipc_buffer_addr, ipc_buffer_cap, ring_vas)

use alloc::vec::Vec;
use core::ops::Range;
use object::{
    Object, ObjectSegment, SegmentFlags,
    elf::{PF_R, PF_W, PF_X},
};

use crate::object_allocator::ObjectAllocator;

const GRANULE_SIZE: usize = sel4::FrameObjectType::GRANULE.bytes();

pub(crate) fn create_child_vspace<'a>(
    allocator: &mut ObjectAllocator,
    image: &'a impl Object<'a>,
    caller_vspace: sel4::cap::VSpace,
    free_page_addr: usize,
    asid_pool: sel4::cap::AsidPool,
    ring_frames: &[sel4::cap::Granule],
) -> (sel4::cap::VSpace, usize, sel4::cap::Granule, Vec<usize>) {
    let child_vspace = allocator.allocate_fixed_sized::<sel4::cap_type::VSpace>();
    asid_pool.asid_pool_assign(child_vspace).unwrap();

    let image_footprint = footprint(image);

    // Footprint étendu : image + IPC buffer + N ring buffers
    let extra = (1 + ring_frames.len()) * GRANULE_SIZE;
    let extended_footprint = image_footprint.start..(image_footprint.end + extra);

    map_intermediate_translation_tables(allocator, child_vspace, extended_footprint.clone());

    map_image(
        allocator,
        child_vspace,
        image_footprint.clone(),
        image,
        caller_vspace,
        free_page_addr,
    );

    // IPC buffer — juste après le footprint image
    let ipc_buffer_addr = image_footprint.end;
    let ipc_buffer_cap = allocator.allocate_fixed_sized::<sel4::cap_type::Granule>();
    ipc_buffer_cap
        .frame_map(
            child_vspace,
            ipc_buffer_addr,
            sel4::CapRights::read_write(),
            sel4::VmAttributes::default(),
        )
        .unwrap();

    // Ring buffers — consécutifs après l'IPC buffer
    let mut ring_vas = Vec::new();
    for (i, ring_frame) in ring_frames.iter().enumerate() {
        let ring_va = image_footprint.end + (1 + i) * GRANULE_SIZE;
        ring_frame
            .frame_map(
                child_vspace,
                ring_va,
                sel4::CapRights::read_write(),
                sel4::VmAttributes::default(),
            )
            .unwrap();
        ring_vas.push(ring_va);
    }

    (child_vspace, ipc_buffer_addr, ipc_buffer_cap, ring_vas)
}

fn map_intermediate_translation_tables(
    allocator: &mut ObjectAllocator,
    vspace: sel4::cap::VSpace,
    footprint: Range<usize>,
) {
    for level in 1..sel4::vspace_levels::NUM_LEVELS {
        let span_bytes = 1 << sel4::vspace_levels::span_bits(level);
        let footprint_at_level = coarsen_footprint(&footprint, span_bytes);
        for i in 0..(footprint_at_level.len() / span_bytes) {
            let ty = sel4::TranslationTableObjectType::from_level(level).unwrap();
            let addr = footprint_at_level.start + i * span_bytes;
            allocator
                .allocate(ty.blueprint())
                .cast::<sel4::cap_type::UnspecifiedIntermediateTranslationTable>()
                .generic_intermediate_translation_table_map(
                    ty,
                    vspace,
                    addr,
                    sel4::VmAttributes::default(),
                )
                .unwrap()
        }
    }
}

fn map_image<'a>(
    allocator: &mut ObjectAllocator,
    vspace: sel4::cap::VSpace,
    footprint: Range<usize>,
    image: &'a impl Object<'a>,
    caller_vspace: sel4::cap::VSpace,
    free_page_addr: usize,
) {
    let num_pages = footprint.len() / GRANULE_SIZE;
    let mut pages = (0..num_pages)
        .map(|_| {
            (
                allocator.allocate_fixed_sized::<sel4::cap_type::Granule>(),
                sel4::CapRightsBuilder::none(),
            )
        })
        .collect::<Vec<_>>();

    for seg in image.segments() {
        let segment_addr = usize::try_from(seg.address()).unwrap();
        let segment_size = usize::try_from(seg.size()).unwrap();
        let segment_footprint =
            coarsen_footprint(&(segment_addr..(segment_addr + segment_size)), GRANULE_SIZE);
        let segment_data_size = seg.data().unwrap().len();
        let segment_data_footprint = coarsen_footprint(
            &(segment_addr..(segment_addr + segment_data_size)),
            GRANULE_SIZE,
        );
        let num_pages_spanned_by_segment = segment_footprint.len() / GRANULE_SIZE;
        let num_pages_spanned_by_segment_data = segment_data_footprint.len() / GRANULE_SIZE;
        let segment_page_index_offset = (segment_footprint.start - footprint.start) / GRANULE_SIZE;

        for (_, rights) in
            &mut pages[segment_page_index_offset..][..num_pages_spanned_by_segment]
        {
            add_rights(rights, seg.flags());
        }

        let mut data = seg.data().unwrap();
        let mut offset_into_page = segment_addr % GRANULE_SIZE;
        for (page_cap, _) in
            &pages[segment_page_index_offset..][..num_pages_spanned_by_segment_data]
        {
            let data_len = (GRANULE_SIZE - offset_into_page).min(data.len());
            page_cap
                .frame_map(
                    caller_vspace,
                    free_page_addr,
                    sel4::CapRights::read_write(),
                    sel4::VmAttributes::default(),
                )
                .unwrap();
            unsafe {
                ((free_page_addr + offset_into_page) as *mut u8)
                    .copy_from(data.as_ptr(), data_len);
            }
            page_cap.frame_unmap().unwrap();
            data = &data[data_len..];
            offset_into_page = 0;
        }
    }

    for (i, (page_cap, rights)) in pages.into_iter().enumerate() {
        let addr = footprint.start + i * GRANULE_SIZE;
        page_cap
            .frame_map(vspace, addr, rights.build(), sel4::VmAttributes::default())
            .unwrap();
    }
}

fn add_rights(rights: &mut sel4::CapRightsBuilder, flags: SegmentFlags) {
    match flags {
        SegmentFlags::Elf { p_flags } => {
            if p_flags & PF_R != 0 {
                *rights = rights.read(true);
            }
            if p_flags & PF_W != 0 {
                *rights = rights.write(true);
            }
            if p_flags & PF_X != 0 {
                *rights = rights.grant(true);
            }
        }
        _ => unimplemented!(),
    }
}

pub(crate) fn footprint<'a>(image: &'a impl Object<'a>) -> Range<usize> {
    let min: usize = image
        .segments()
        .map(|seg| seg.address())
        .min()
        .unwrap()
        .try_into()
        .unwrap();
    let max: usize = image
        .segments()
        .map(|seg| seg.address() + seg.size())
        .max()
        .unwrap()
        .try_into()
        .unwrap();
    coarsen_footprint(&(min..max), GRANULE_SIZE)
}

pub(crate) fn coarsen_footprint(footprint: &Range<usize>, granularity: usize) -> Range<usize> {
    round_down(footprint.start, granularity)..footprint.end.next_multiple_of(granularity)
}

const fn round_down(n: usize, b: usize) -> usize {
    n - n % b
}

// ── Extension C.8 : Hardware mapping ─────────────────────────────────────────

/// VA où le serveur accède à ses DMA frames (identique à C.4/C.5)
pub(crate) const DMA_VA_BASE: usize = 0x1000_0000;
/// VA où le serveur scanne le MMIO virtio (identique à C.4/C.5)
pub(crate) const SCAN_VA: usize = 0x2000_0000;

/// Mappe les frames DMA et MMIO dans le VSpace du serveur.
///
/// DMA_VA_BASE (0x1000_0000) et SCAN_VA (0x2000_0000) sont dans le même 1GB
/// que l'ELF du serveur. Les PTs de niveau 1 et 2 (PUD/PD, 512GB/1GB) ont
/// déjà été créés par create_child_vspace. Seuls les PTs de niveau 3 (PT, 2MB)
/// manquent : on les crée avec le retry-loop (identique à C.4/C.5).
///
/// dma_frames  : SmallPage caps non-device (mappés read_write à DMA_VA_BASE)
/// mmio_frames : SmallPage caps device    (mappés read_write à SCAN_VA)
pub(crate) fn map_hardware_into_vspace(
    allocator: &mut ObjectAllocator,
    vspace: sel4::cap::VSpace,
    dma_frames: &[sel4::cap::Granule],
    mmio_frames: &[sel4::cap::Granule],
) {
    for (i, frame) in dma_frames.iter().enumerate() {
        let va = DMA_VA_BASE + i * GRANULE_SIZE;
        map_frame_rw_retry(allocator, *frame, vspace, va);
    }
    for (i, frame) in mmio_frames.iter().enumerate() {
        let va = SCAN_VA + i * GRANULE_SIZE;
        map_frame_rw_retry(allocator, *frame, vspace, va);
    }
}

/// Mappe un frame en read_write ; si le PT niveau 3 (2MB) manque, le crée et
/// réessaie. Identique au retry-loop de map_ram_pages / map_device_pages_contiguous
/// dans C.4/C.5.
///
/// Les niveaux 1 (PUD 512GB) et 2 (PD 1GB) ont déjà été créés par
/// create_child_vspace pour le même 1GB bloc que DMA_VA_BASE et SCAN_VA.
fn map_frame_rw_retry(
    allocator: &mut ObjectAllocator,
    frame: sel4::cap::Granule,
    vspace: sel4::cap::VSpace,
    va: usize,
) {
    loop {
        match frame.frame_map(vspace, va, sel4::CapRights::read_write(), sel4::VmAttributes::default()) {
            Ok(()) => return,
            Err(_) => {
                // PT niveau 3 (2MB) manquant — créer et réessayer.
                allocator
                    .allocate_fixed_sized::<sel4::cap_type::PT>()
                    .pt_map(vspace, va, sel4::VmAttributes::default())
                    .expect("map_hardware: pt_map niveau 3 échoué");
            }
        }
    }
}
