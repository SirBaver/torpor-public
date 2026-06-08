// child_vspace.rs — Adapté de c8-store, crate commune C.10
//
// Extension C.10 : map_jit_frames_rw
//   - Crée les tables intermédiaires pour le pool JIT (hors bloc 1GB ELF)
//   - Mappe 128 frames dédiées RW+EXECUTE_NEVER à JIT_POOL_VA_BASE

use alloc::vec::Vec;
use core::ops::Range;
use object::{
    Object, ObjectSegment, SegmentFlags,
    elf::{PF_R, PF_W, PF_X},
};

use crate::object_allocator::ObjectAllocator;

const GRANULE_SIZE: usize = sel4::FrameObjectType::GRANULE.bytes();

pub fn create_child_vspace<'a>(
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

/// Mappe 128 frames JIT à base_va en RW + EXECUTE_NEVER (W^X : état initial W).
///
/// Les tables intermédiaires pour base_va (hors du bloc 1 GB de l'ELF) sont créées
/// de façon tolérante : les erreurs "already exists" (niveau PUD commun) sont ignorées.
/// Un objet table est alloué mais non mappé dans ce cas (fuite mineure acceptable pour smoke).
pub fn map_jit_frames_rw(
    allocator: &mut ObjectAllocator,
    vspace: sel4::cap::VSpace,
    jit_frames: &[sel4::cap::Granule],
    base_va: usize,
) {
    let pool_end = base_va + jit_frames.len() * GRANULE_SIZE;

    // Créer les tables intermédiaires pour base_va (tolérant : ignore les tables déjà existantes)
    for level in 1..sel4::vspace_levels::NUM_LEVELS {
        let span_bytes = 1_usize << sel4::vspace_levels::span_bits(level);
        let fp = coarsen_footprint(&(base_va..pool_end), span_bytes);
        let count = fp.len() / span_bytes;
        for i in 0..count {
            let ty = sel4::TranslationTableObjectType::from_level(level).unwrap();
            let addr = fp.start + i * span_bytes;
            let _ = allocator
                .allocate(ty.blueprint())
                .cast::<sel4::cap_type::UnspecifiedIntermediateTranslationTable>()
                .generic_intermediate_translation_table_map(
                    ty,
                    vspace,
                    addr,
                    sel4::VmAttributes::default(),
                );
        }
    }

    // Mapper les frames JIT en RW + EXECUTE_NEVER (W^X : état W)
    for (i, frame) in jit_frames.iter().enumerate() {
        let va = base_va + i * GRANULE_SIZE;
        frame
            .frame_map(
                vspace,
                va,
                sel4::CapRights::read_write(),
                sel4::VmAttributes::default() | sel4::VmAttributes::EXECUTE_NEVER,
            )
            .expect("map_jit_frames_rw: frame_map failed");
    }
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

pub fn footprint<'a>(image: &'a impl Object<'a>) -> Range<usize> {
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

pub fn coarsen_footprint(footprint: &Range<usize>, granularity: usize) -> Range<usize> {
    round_down(footprint.start, granularity)..footprint.end.next_multiple_of(granularity)
}

const fn round_down(n: usize, b: usize) -> usize {
    n - n % b
}

/// Provisionne `data` dans N frames neuves et les mappe dans `vspace` à `base_va`.
///
/// Format dans les frames : octets 0..7 = len(data) en u64 LE, octets 8..8+len = data.
/// Approche tolérante pour les tables intermédiaires (même niveau 1 que le pool JIT).
pub fn provision_bytes_into_vspace(
    allocator: &mut ObjectAllocator,
    vspace: sel4::cap::VSpace,
    caller_vspace: sel4::cap::VSpace,
    free_page_addr: usize,
    data: &[u8],
    base_va: usize,
) {
    let total_len = 8 + data.len();
    let n_frames = total_len.div_ceil(GRANULE_SIZE);
    let pool_end = base_va + n_frames * GRANULE_SIZE;

    // Tables intermédiaires (tolérante : ignore "already exists")
    for level in 1..sel4::vspace_levels::NUM_LEVELS {
        let span_bytes = 1_usize << sel4::vspace_levels::span_bits(level);
        let fp = coarsen_footprint(&(base_va..pool_end), span_bytes);
        let count = fp.len() / span_bytes;
        for i in 0..count {
            let ty = sel4::TranslationTableObjectType::from_level(level).unwrap();
            let addr = fp.start + i * span_bytes;
            let _ = allocator
                .allocate(ty.blueprint())
                .cast::<sel4::cap_type::UnspecifiedIntermediateTranslationTable>()
                .generic_intermediate_translation_table_map(
                    ty,
                    vspace,
                    addr,
                    sel4::VmAttributes::default(),
                );
        }
    }

    let len_bytes = (data.len() as u64).to_le_bytes();
    let frames = allocator.allocate_frames_batch(n_frames);

    for (i, &frame) in frames.iter().enumerate() {
        let page_start = i * GRANULE_SIZE;
        let page_end = page_start + GRANULE_SIZE;

        frame.frame_map(
            caller_vspace,
            free_page_addr,
            sel4::CapRights::read_write(),
            sel4::VmAttributes::default(),
        ).unwrap();

        unsafe { core::ptr::write_bytes(free_page_addr as *mut u8, 0, GRANULE_SIZE); }

        // En-tête length (octets 0..8 de la région encodée, dans frame 0)
        if page_start < 8 {
            let h_end = page_end.min(8);
            unsafe {
                core::ptr::copy_nonoverlapping(
                    len_bytes[page_start..].as_ptr(),
                    (free_page_addr + page_start - page_start) as *mut u8,
                    h_end - page_start,
                );
            }
        }

        // Données (région encodée [8..8+data.len()))
        let d_start = page_start.max(8);
        let d_end = page_end.min(8 + data.len());
        if d_end > d_start {
            let in_page = d_start - page_start;
            let data_offset = d_start - 8;
            unsafe {
                core::ptr::copy_nonoverlapping(
                    data[data_offset..].as_ptr(),
                    (free_page_addr + in_page) as *mut u8,
                    d_end - d_start,
                );
            }
        }

        frame.frame_unmap().unwrap();

        frame.frame_map(
            vspace,
            base_va + i * GRANULE_SIZE,
            sel4::CapRights::read_write(),
            sel4::VmAttributes::default() | sel4::VmAttributes::EXECUTE_NEVER,
        ).unwrap();
    }
}

// ── Hardware mapping (DMA + MMIO dans le VSpace serveur) ─────────────────────

pub const DMA_VA_BASE: usize = 0x1000_0000;
pub const SCAN_VA: usize = 0x2000_0000;

pub fn map_hardware_into_vspace(
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

pub fn map_frame_rw_retry(
    allocator: &mut ObjectAllocator,
    frame: sel4::cap::Granule,
    vspace: sel4::cap::VSpace,
    va: usize,
) {
    loop {
        match frame.frame_map(vspace, va, sel4::CapRights::read_write(), sel4::VmAttributes::default()) {
            Ok(()) => return,
            Err(_) => {
                allocator
                    .allocate_fixed_sized::<sel4::cap_type::PT>()
                    .pt_map(vspace, va, sel4::VmAttributes::default())
                    .expect("map_frame_rw_retry: pt_map failed");
            }
        }
    }
}
