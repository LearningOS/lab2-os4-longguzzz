//! Implementation of [`MapArea`] and [`MemorySet`].

use super::range_manager::RangeManager;
use super::{frame_alloc, FrameTracker};
use super::{PTEFlags, PageTable, PageTableEntry};
use super::{PhysAddr, PhysPageNum, VirtAddr, VirtPageNum};
use super::{StepByOne, VPNRange};
use crate::config::{MEMORY_END, PAGE_SIZE, TRAMPOLINE, TRAP_CONTEXT, USER_STACK_SIZE};
use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec::Vec;
use lazy_static::*;
use riscv::register::satp;
use spin::Mutex;
use core::cell::RefCell;

extern "C" {
    fn stext();
    fn etext();
    fn srodata();
    fn erodata();
    fn sdata();
    fn edata();
    fn sbss_with_stack();
    fn ebss();
    fn ekernel();
    fn strampoline();
}

lazy_static! {
    /// a memory set instance through lazy_static! managing kernel space
    pub static ref KERNEL_SPACE: Arc<Mutex<MemorySet>> =
        Arc::new(Mutex::new(MemorySet::new_kernel()));
}

/// memory set structure, controls virtual-memory space
pub struct MemorySet {
    page_table: PageTable,
    areas: Vec<MapArea>,
    pub range_manager: RefCell<RangeManager>,
}



impl MemorySet {
    pub fn new_bare() -> Self {
        Self {
            page_table: PageTable::new(),
            areas: Vec::new(),
            range_manager: RefCell::new(RangeManager::new()),
        }
    }
    pub fn token(&self) -> usize {
        self.page_table.token()
    }

    pub fn find_area_idx(&self, vpn_range: VPNRange) -> Option<usize> {
        // 找的是VPNRange里的第一个
        let index = self.areas.iter().position(
            |area| {
                area.vpn_range == vpn_range
            }
        );
        return index
    }

    /// Assume that no conflicts.
    pub fn insert_framed_area(
        &mut self,
        start_va: VirtAddr,
        end_va: VirtAddr,
        permission: MapPermission,
    ) {
        self.push(
            MapArea::new(start_va, end_va, MapType::Framed, permission),
            None,
        );
    }

    pub fn remove_framed_area(
        &mut self, 
        vpn_rng: VPNRange
    )  {
        let idx = self.find_area_idx(vpn_rng);
        let idx = idx.unwrap();
        // 对应的data_frames会自动回收
        self.areas.remove(idx);
    }

    // todo: 初始化的时候需要加入range manager
    pub fn push(&mut self, mut map_area: MapArea, data: Option<&[u8]>) {
        map_area.map(&mut self.page_table);
        if let Some(data) = data {
            map_area.copy_data(&mut self.page_table, data);
        }
        self.areas.push(map_area);
    }

    pub fn extend_framed_area(
        &mut self, 
        prev_range: VPNRange, 
        new_range:VPNRange, 
        map_perm:MapPermission
    ) {
        let idx = self.find_area_idx(prev_range);
        let idx = idx.unwrap();
        let old_range = self.areas[idx].vpn_range;
        let (old_start,old_end) = (old_range.get_start(),old_range.get_end());
        let (new_start,new_end) = (new_range.get_start(),new_range.get_end());
        
        let old_perm = self.areas[idx].map_perm;
        if old_perm == map_perm {
            if old_end == new_end && new_start < old_start {
                for vpn in VPNRange::new(new_start, old_start) {
                    self.areas[idx].map_one(&mut self.page_table, vpn);
                }
                self.areas[idx].vpn_range = VPNRange::new(new_start, new_end);
            }
            if old_start == new_start && new_end > old_end {
                for vpn in VPNRange::new(old_end, new_end) {
                    self.areas[idx].map_one(&mut self.page_table, vpn);
                }
                self.areas[idx].vpn_range = VPNRange::new(new_start, new_end);
            }
        }// else {}
        
    }

    pub fn shrink_framed_area(
        &mut self, 
        prev_range: VPNRange, 
        new_range:VPNRange, 
    ) {
        let idx = self.find_area_idx(prev_range);
        let idx = idx.unwrap();
        let old_range = self.areas[idx].vpn_range;
        let (old_start,old_end) = (old_range.get_start(),old_range.get_end());
        let (new_start,new_end) = (new_range.get_start(),new_range.get_end());
        
        if old_end == new_end && new_start > old_start {
            for vpn in VPNRange::new(old_start, new_start) {
                self.areas[idx].unmap_one(&mut self.page_table, vpn);
            }
            self.areas[idx].vpn_range = VPNRange::new(new_start, new_end);
        }
        if old_start == new_start && new_end < old_end {
            for vpn in VPNRange::new(new_end, old_end) {
                self.areas[idx].unmap_one(&mut self.page_table, vpn);
            }
            self.areas[idx].vpn_range = VPNRange::new(new_start, new_end);
        }
        
    }

    // pub fn copy_data_to(
    //     &mut self, 
    //     page_table: &mut PageTable, 
    //     vpn_range: VPNRange,
    //     data: &[u8]) 
    // {
    //     let mut start: usize = 0;
    //     let mut current_vpn = vpn_range.get_start();
    //     let len = data.len();
    //     loop {
    //         let src = &data[start..len.min(start + PAGE_SIZE)];
    //         let dst = &mut page_table
    //             .translate(current_vpn)
    //             .unwrap()
    //             .ppn()
    //             .get_bytes_array()[..src.len()];
    //         dst.copy_from_slice(src);
    //         start += PAGE_SIZE;
    //         if start >= len {
    //             break;
    //         }
    //         current_vpn.step();
    //     }
    // }

    pub fn combine_framed_area(
        &mut self, 
        front_range: VPNRange, 
        back_range:VPNRange,
        map_perm:MapPermission
    )  {
        let front_idx = self.find_area_idx(front_range);
        let front_idx = front_idx.unwrap();
        let front_perm = self.areas[front_idx].map_perm;

        let back_idx = self.find_area_idx(back_range);
        let back_idx = back_idx.unwrap();
        let back_perm = self.areas[back_idx].map_perm;
        
        if front_perm == map_perm && back_perm == map_perm {
            // let mut frames_for_transfer = self.areas[back_idx].data_frames;
            // self.areas[front_idx].data_frames.append(& mut frames_for_transfer);
            // for (&vpn, &frame) in &self.areas[back_idx].v{
            //     self.areas[front_idx].data_frames.insert(vpn, frame);
            // }

            // while let Some((vpn,frame)) = self.areas[back_idx].data_frames.pop_first() {

            // }
            // 迁移back_area的数据
            for vpn in back_range {
                let frame = self.areas[back_idx].data_frames.remove(&vpn);
                let frame = frame.unwrap();
                self.areas[front_idx].data_frames.insert(vpn, frame);
            }
            // 分配中间的新数据部分
            for vpn in VPNRange::new(front_range.get_end(), back_range.get_start()) {
                self.areas[front_idx].map_one(&mut self.page_table, vpn);
            }
            self.areas[front_idx].vpn_range = VPNRange::new(front_range.get_start(), back_range.get_end());
            self.areas.remove(back_idx);
        }
    }

    pub fn split_framed_area(
        &mut self, 
        front_range: VPNRange, 
        back_range:VPNRange,
    ) {
        let original_range = VPNRange::new(front_range.get_start(), back_range.get_end());
        let original_idx = self.find_area_idx(original_range);
        let original_idx = original_idx.unwrap();
        let original_perm = self.areas[original_idx].map_perm;

        let remove_range = VPNRange::new(front_range.get_end(), back_range.get_start());
        for vpn in remove_range {
            self.areas[original_idx].data_frames.remove(&vpn);
        }
        self.areas[original_idx].vpn_range = VPNRange::new(front_range.get_start(), front_range.get_end());

        let mut back_data_frames = BTreeMap::new();
        for vpn in back_range {
            let frame = self.areas[original_idx].data_frames.remove(&vpn);
            let frame = frame.unwrap();
            back_data_frames.insert(vpn, frame);
        }
        let back_map_area = MapArea {
            vpn_range: back_range,
            data_frames: back_data_frames,
            map_type: MapType::Framed,
            map_perm: original_perm,
        };

        self.areas.push(back_map_area);
    }

    // pub fn remove(&mut self, vpn_range:VPNRange){
    //     let res = self.range_manager.borrow_mut().remove(&vpn_range);
    //     match res {
    //         None => {

    //         }
    //     }
        
    //     let index = self.areas.iter()
    //         .position(|&map_area| 
    //             map_area.vpn_range.get_start() == vpn_range.get_start()
    //             && map_area.vpn_range.get_end() == vpn_range.get_end());
    //     match index {
    //         None => {},
    //         Some(index) => {
    //             self.areas.remove(index);
    //         }
    //     }
    // }

    /// Mention that trampoline is not collected by areas.
    fn map_trampoline(&mut self) {
        self.page_table.map(
            VirtAddr::from(TRAMPOLINE).into(),
            PhysAddr::from(strampoline as usize).into(),
            PTEFlags::R | PTEFlags::X,
        );
    }
    /// Without kernel stacks.
    pub fn new_kernel() -> Self {
        let mut memory_set = Self::new_bare();
        // map trampoline
        memory_set.map_trampoline();
        // map kernel sections
        info!(".text [{:#x}, {:#x})", stext as usize, etext as usize);
        info!(".rodata [{:#x}, {:#x})", srodata as usize, erodata as usize);
        info!(".data [{:#x}, {:#x})", sdata as usize, edata as usize);
        info!(
            ".bss [{:#x}, {:#x})",
            sbss_with_stack as usize, ebss as usize
        );
        info!("mapping .text section");
        memory_set.push(
            MapArea::new(
                (stext as usize).into(),
                (etext as usize).into(),
                MapType::Identical,
                MapPermission::R | MapPermission::X,
            ),
            None,
        );
        info!("mapping .rodata section");
        memory_set.push(
            MapArea::new(
                (srodata as usize).into(),
                (erodata as usize).into(),
                MapType::Identical,
                MapPermission::R,
            ),
            None,
        );
        info!("mapping .data section");
        memory_set.push(
            MapArea::new(
                (sdata as usize).into(),
                (edata as usize).into(),
                MapType::Identical,
                MapPermission::R | MapPermission::W,
            ),
            None,
        );
        info!("mapping .bss section");
        memory_set.push(
            MapArea::new(
                (sbss_with_stack as usize).into(),
                (ebss as usize).into(),
                MapType::Identical,
                MapPermission::R | MapPermission::W,
            ),
            None,
        );
        info!("mapping physical memory");
        memory_set.push(
            MapArea::new(
                (ekernel as usize).into(),
                MEMORY_END.into(),
                MapType::Identical,
                MapPermission::R | MapPermission::W,
            ),
            None,
        );
        memory_set
    }
    /// Include sections in elf and trampoline and TrapContext and user stack,
    /// also returns user_sp and entry point.
    pub fn from_elf(elf_data: &[u8]) -> (Self, usize, usize) {
        let mut memory_set = Self::new_bare();
        // map trampoline
        memory_set.map_trampoline();
        // map program headers of elf, with U flag
        let elf = xmas_elf::ElfFile::new(elf_data).unwrap();
        let elf_header = elf.header;
        let magic = elf_header.pt1.magic;
        assert_eq!(magic, [0x7f, 0x45, 0x4c, 0x46], "invalid elf!");
        let ph_count = elf_header.pt2.ph_count();
        let mut max_end_vpn = VirtPageNum(0);
        for i in 0..ph_count {
            let ph = elf.program_header(i).unwrap();
            if ph.get_type().unwrap() == xmas_elf::program::Type::Load {
                let start_va: VirtAddr = (ph.virtual_addr() as usize).into();
                let end_va: VirtAddr = ((ph.virtual_addr() + ph.mem_size()) as usize).into();
                let mut map_perm = MapPermission::U;
                let ph_flags = ph.flags();
                if ph_flags.is_read() {
                    map_perm |= MapPermission::R;
                }
                if ph_flags.is_write() {
                    map_perm |= MapPermission::W;
                }
                if ph_flags.is_execute() {
                    map_perm |= MapPermission::X;
                }
                let map_area = MapArea::new(start_va, end_va, MapType::Framed, map_perm);
                max_end_vpn = map_area.vpn_range.get_end();
                memory_set.push(
                    map_area,
                    Some(&elf.input[ph.offset() as usize..(ph.offset() + ph.file_size()) as usize]),
                );
            }
        }
        // map user stack with U flags
        let max_end_va: VirtAddr = max_end_vpn.into();
        let mut user_stack_bottom: usize = max_end_va.into();
        // guard page
        user_stack_bottom += PAGE_SIZE;
        let user_stack_top = user_stack_bottom + USER_STACK_SIZE;
        memory_set.push(
            MapArea::new(
                user_stack_bottom.into(),
                user_stack_top.into(),
                MapType::Framed,
                MapPermission::R | MapPermission::W | MapPermission::U,
            ),
            None,
        );
        // map TrapContext
        memory_set.push(
            MapArea::new(
                TRAP_CONTEXT.into(),
                TRAMPOLINE.into(),
                MapType::Framed,
                MapPermission::R | MapPermission::W,
            ),
            None,
        );
        (
            memory_set,
            user_stack_top,
            elf.header.pt2.entry_point() as usize,
        )
    }
    pub fn activate(&self) {
        let satp = self.page_table.token();
        unsafe {
            satp::write(satp);
            core::arch::asm!("sfence.vma");
        }
    }
    pub fn translate(&self, vpn: VirtPageNum) -> Option<PageTableEntry> {
        self.page_table.translate(vpn)
    }
}

/// map area structure, controls a contiguous piece of virtual memory
pub struct MapArea {
    vpn_range: VPNRange,
    data_frames: BTreeMap<VirtPageNum, FrameTracker>,
    map_type: MapType,
    map_perm: MapPermission,
}

impl MapArea {
    pub fn new(
        start_va: VirtAddr,
        end_va: VirtAddr,
        map_type: MapType,
        map_perm: MapPermission,
    ) -> Self {
        let start_vpn: VirtPageNum = start_va.floor();
        let end_vpn: VirtPageNum = end_va.ceil();
        Self {
            vpn_range: VPNRange::new(start_vpn, end_vpn),
            data_frames: BTreeMap::new(),
            map_type,
            map_perm,
        }
    }

    pub fn map_one(&mut self, page_table: &mut PageTable, vpn: VirtPageNum) {
        let ppn: PhysPageNum;
        match self.map_type {
            MapType::Identical => {
                ppn = PhysPageNum(vpn.0);
            }
            MapType::Framed => {
                let frame = frame_alloc().unwrap();
                ppn = frame.ppn;
                self.data_frames.insert(vpn, frame);
            }
        }
        let pte_flags = PTEFlags::from_bits(self.map_perm.bits).unwrap();
        page_table.map(vpn, ppn, pte_flags);
    }
    #[allow(unused)]
    pub fn unmap_one(&mut self, page_table: &mut PageTable, vpn: VirtPageNum) {
        #[allow(clippy::single_match)]
        match self.map_type {
            MapType::Framed => {
                self.data_frames.remove(&vpn);
            }
            _ => {}
        }
        page_table.unmap(vpn);
    }
    pub fn map(&mut self, page_table: &mut PageTable) {
        for vpn in self.vpn_range {
            self.map_one(page_table, vpn);
        }
    }
    #[allow(unused)]
    pub fn unmap(&mut self, page_table: &mut PageTable) {
        for vpn in self.vpn_range {
            self.unmap_one(page_table, vpn);
        }
    }
    /// data: start-aligned but maybe with shorter length
    /// assume that all frames were cleared before
    pub fn copy_data(&mut self, page_table: &mut PageTable, data: &[u8]) {
        assert_eq!(self.map_type, MapType::Framed);
        let mut start: usize = 0;
        let mut current_vpn = self.vpn_range.get_start();
        let len = data.len();
        loop {
            let src = &data[start..len.min(start + PAGE_SIZE)];
            let dst = &mut page_table
                .translate(current_vpn)
                .unwrap()
                .ppn()
                .get_bytes_array()[..src.len()];
            dst.copy_from_slice(src);
            start += PAGE_SIZE;
            if start >= len {
                break;
            }
            current_vpn.step();
        }
    }
}

#[derive(Copy, Clone, PartialEq, Debug)]
/// map type for memory set: identical or framed
pub enum MapType {
    Identical,
    Framed,
}

bitflags! {
    /// map permission corresponding to that in pte: `R W X U`
    pub struct MapPermission: u8 {
        const R = 1 << 1;
        const W = 1 << 2;
        const X = 1 << 3;
        const U = 1 << 4;
    }
}

#[allow(unused)]
pub fn remap_test() {
    let mut kernel_space = KERNEL_SPACE.lock();
    let mid_text: VirtAddr = ((stext as usize + etext as usize) / 2).into();
    let mid_rodata: VirtAddr = ((srodata as usize + erodata as usize) / 2).into();
    let mid_data: VirtAddr = ((sdata as usize + edata as usize) / 2).into();
    assert!(!kernel_space
        .page_table
        .translate(mid_text.floor())
        .unwrap()
        .writable());
    assert!(!kernel_space
        .page_table
        .translate(mid_rodata.floor())
        .unwrap()
        .writable());
    assert!(!kernel_space
        .page_table
        .translate(mid_data.floor())
        .unwrap()
        .executable());
    info!("remap_test passed!");
}



