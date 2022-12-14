//! Task management implementation
//!
//! Everything about task management, like starting and switching tasks is
//! implemented here.
//!
//! A single global instance of [`TaskManager`] called `TASK_MANAGER` controls
//! all the tasks in the operating system.
//!
//! Be careful when you see [`__switch`]. Control flow around this function
//! might not be what you expect.

mod context;
mod switch;
#[allow(clippy::module_inception)]
mod task;


use crate::config::{MAX_SYSCALL_NUM, PAGE_SIZE};
use crate::loader::{get_app_data, get_num_app};
use crate::mm::{MapPermission, VPNRange, VirtAddr, RangesChanged};
use crate::sync::UPSafeCell;
use crate::timer::get_time_us;
use crate::trap::TrapContext;
use alloc::vec::Vec;
use lazy_static::*;
pub use switch::__switch;
pub use task::{TaskControlBlock, TaskStatus};

pub use context::TaskContext;

/// The task manager, where all the tasks are managed.
///
/// Functions implemented on `TaskManager` deals with all task state transitions
/// and task context switching. For convenience, you can find wrappers around it
/// in the module level.
///
/// Most of `TaskManager` are hidden behind the field `inner`, to defer
/// borrowing checks to runtime. You can see examples on how to use `inner` in
/// existing functions on `TaskManager`.
pub struct TaskManager {
    /// total number of tasks
    num_app: usize,
    /// use inner value to get mutable access
    inner: UPSafeCell<TaskManagerInner>,
}

/// The task manager inner in 'UPSafeCell'
struct TaskManagerInner {
    /// task list
    tasks: Vec<TaskControlBlock>,
    /// id of current `Running` task
    current_task: usize,
}

lazy_static! {
    /// a `TaskManager` instance through lazy_static!
    pub static ref TASK_MANAGER: TaskManager = {
        info!("init TASK_MANAGER");
        let num_app = get_num_app();
        info!("num_app = {}", num_app);
        let mut tasks: Vec<TaskControlBlock> = Vec::new();
        for i in 0..num_app {
            tasks.push(TaskControlBlock::new(get_app_data(i), i));
        }
        TaskManager {
            num_app,
            inner: unsafe {
                UPSafeCell::new(TaskManagerInner {
                    tasks,
                    current_task: 0,
                })
            },
        }
    };
}

impl TaskManager {
    /// Run the first task in task list.
    ///
    /// Generally, the first task in task list is an idle task (we call it zero process later).
    /// But in ch4, we load apps statically, so the first task is a real app.
    fn run_first_task(&self) -> ! {
        let mut inner = self.inner.exclusive_access();
        let next_task = &mut inner.tasks[0];

        next_task.task_status = TaskStatus::Running;
        if next_task.begin_time_ms ==0 {
            next_task.begin_time_ms = get_time_us()/1_000;
        }

        let next_task_cx_ptr = &next_task.task_cx as *const TaskContext;
        drop(inner);
        let mut _unused = TaskContext::zero_init();
        // before this, we should drop local variables that must be dropped manually
        unsafe {
            __switch(&mut _unused as *mut _, next_task_cx_ptr);
        }
        panic!("unreachable in run_first_task!");
    }

    /// Change the status of current `Running` task into `Ready`.
    fn mark_current_suspended(&self) {
        let mut inner = self.inner.exclusive_access();
        let current = inner.current_task;
        inner.tasks[current].task_status = TaskStatus::Ready;
    }

    /// Change the status of current `Running` task into `Exited`.
    fn mark_current_exited(&self) {
        let mut inner = self.inner.exclusive_access();
        let current = inner.current_task;
        inner.tasks[current].task_status = TaskStatus::Exited;
    }

    /// Find next task to run and return task id.
    ///
    /// In this case, we only return the first `Ready` task in task list.
    fn find_next_task(&self) -> Option<usize> {
        let inner = self.inner.exclusive_access();
        let current = inner.current_task;
        (current + 1..current + self.num_app + 1)
            .map(|id| id % self.num_app)
            .find(|id| inner.tasks[*id].task_status == TaskStatus::Ready)
    }

    /// Get the current 'Running' task's token.
    fn get_current_token(&self) -> usize {
        let inner = self.inner.exclusive_access();
        inner.tasks[inner.current_task].get_user_token()
    }

    #[allow(clippy::mut_from_ref)]
    /// Get the current 'Running' task's trap contexts.
    fn get_current_trap_cx(&self) -> &mut TrapContext {
        let inner = self.inner.exclusive_access();
        inner.tasks[inner.current_task].get_trap_cx()
    }

    /// Switch current `Running` task to the task we have found,
    /// or there is no `Ready` task and we can exit with all applications completed
    fn run_next_task(&self) {
        if let Some(next) = self.find_next_task() {
            let mut inner = self.inner.exclusive_access();
            let current = inner.current_task;
            inner.tasks[next].task_status = TaskStatus::Running;

            if inner.tasks[next].begin_time_ms == 0 {
                inner.tasks[next].begin_time_ms = get_time_us()/1_000;
            }

            inner.current_task = next;
            let current_task_cx_ptr = &mut inner.tasks[current].task_cx as *mut TaskContext;
            let next_task_cx_ptr = &inner.tasks[next].task_cx as *const TaskContext;
            drop(inner);
            // before this, we should drop local variables that must be dropped manually
            unsafe {
                __switch(current_task_cx_ptr, next_task_cx_ptr);
            }
            // go back to user mode
        } else {
            panic!("All applications completed!");
        }
    }

    fn get_current_begin_time_ms(&self) -> usize {
        let inner = self.inner.exclusive_access();
        let current = inner.current_task;
        inner.tasks[current].begin_time_ms
    }

    fn get_current_syscall_times(&self) -> [u32;MAX_SYSCALL_NUM] {
        let inner = self.inner.exclusive_access();
        let current = inner.current_task;
        // inner.tasks[current].syscall_times
        let mut syscall_times = [0u32;MAX_SYSCALL_NUM];
        for (syscall_id, count) in &inner.tasks[current].syscall_times {
            syscall_times[*syscall_id] = *count;
        }
        syscall_times
    }

    fn update_current_syscall_times(&self, syscall_id: usize) {
        let mut inner = self.inner.exclusive_access();
        let current = inner.current_task;
        // let mut syscall_times = inner.tasks[current].syscall_times;
        // syscall_times[syscall_id] += 1;


        // let mut syscall_times = inner.tasks[current].syscall_times;
        // if !syscall_times.contains_key(&syscall_id) {
        //     syscall_times.insert(syscall_id, 1);
        // } else {
        //     let prev_count = syscall_times.get(&syscall_id);
        //     syscall_times
        // }

        let specific_syscall_times = inner.tasks[current].syscall_times.entry(syscall_id).or_insert(0);
        *specific_syscall_times += 1;
    }
}

/// Run the first task in task list.
pub fn run_first_task() {
    TASK_MANAGER.run_first_task();
}

/// Switch current `Running` task to the task we have found,
/// or there is no `Ready` task and we can exit with all applications completed
fn run_next_task() {
    TASK_MANAGER.run_next_task();
}

/// Change the status of current `Running` task into `Ready`.
fn mark_current_suspended() {
    TASK_MANAGER.mark_current_suspended();
}

/// Change the status of current `Running` task into `Exited`.
fn mark_current_exited() {
    TASK_MANAGER.mark_current_exited();
}

/// Suspend the current 'Running' task and run the next task in task list.
pub fn suspend_current_and_run_next() {
    mark_current_suspended();
    run_next_task();
}

/// Exit the current 'Running' task and run the next task in task list.
pub fn exit_current_and_run_next() {
    mark_current_exited();
    run_next_task();
}


pub fn get_current_begin_time_ms() -> usize {
    TASK_MANAGER.get_current_begin_time_ms()
}

pub fn get_current_syscall_times() -> [u32; MAX_SYSCALL_NUM] {
    TASK_MANAGER.get_current_syscall_times()
}

pub fn update_current_syscall_times(syscall_id:usize) {
    TASK_MANAGER.update_current_syscall_times(syscall_id)
}

/// Get the current 'Running' task's token.
pub fn current_user_token() -> usize {
    TASK_MANAGER.get_current_token()
}

/// Get the current 'Running' task's trap contexts.
pub fn current_trap_cx() -> &'static mut TrapContext {
    TASK_MANAGER.get_current_trap_cx()
}


pub fn task_memory_set_mmap( _start:usize, _len:usize, _port:usize) -> isize{
    if _port & !0b111 != 0 
    || _port & 0b111 == 0
    || _start & (PAGE_SIZE-1) !=0
    {        return -1    }

    let mut map_perm = MapPermission::U;
    if _port & 0b001 != 0 {
        map_perm |= MapPermission::R;
    }
    if _port & 0b010 != 0 {
        map_perm |= MapPermission::W;
    }
    if _port & 0b100 != 0 {
        map_perm |= MapPermission::X;
    }

    let mut inner = TASK_MANAGER.inner.exclusive_access();
    let current = inner.current_task;
    let (start_va, end_va) = (VirtAddr(_start),VirtAddr(_start+_len));
    // 通过ceil获得[,)区间
    let vpn_rng = VPNRange::new(start_va.into(),end_va.ceil().into());
    
    let mut range_manager = inner.tasks[current].memory_set.range_manager.borrow_mut();
    let res = range_manager.insert(&vpn_rng);
    // 消除immutable和mutable的问题
    drop(range_manager);
    match res {
        None => { return -1 }
        Some(RangesChanged::Inserted(_)) => {
            inner.tasks[current].memory_set.insert_framed_area(start_va, end_va, map_perm);
            return 0
        },
        Some(RangesChanged::Extended(
            prev_range, 
            new_range)
        ) => {
            inner.tasks[current].memory_set.extend_framed_area(prev_range, new_range, map_perm);
            return 0
        },
        Some(RangesChanged::Combined(
            front,
            back)
        ) => {
            inner.tasks[current].memory_set.combine_framed_area(front, back, map_perm);
            return 0
        },
        _ =>{}
            // let m = MapArea::new(start_va, end_va, MapType::Framed, map_perm);
            // return 0
    }
    -1
    // 需要当前memory_set的manager，统一管理映射情况
}

pub fn task_memory_set_unmmap(_start:usize, _len:usize) -> isize {
    let mut inner = TASK_MANAGER.inner.exclusive_access();
    let current = inner.current_task;
    let (start_va, end_va) = (VirtAddr(_start),VirtAddr(_start+_len));
    if _start & (PAGE_SIZE-1) !=0 || (_start+_len) & (PAGE_SIZE-1) !=0
    {        return -1    }
    let vpn_rng = VPNRange::new(start_va.into(),end_va.ceil().into());

    let mut range_manager = inner.tasks[current].memory_set.range_manager.borrow_mut();
    let res = range_manager.remove(&vpn_rng);
    drop(range_manager);
    match res {
        None => { return -1 },
        Some(RangesChanged::Removed(_)) => {
            inner.tasks[current].memory_set.remove_framed_area(vpn_rng);
            return 0
        },
        Some(RangesChanged::Shrinked(
            prev_range, 
            new_range)
        ) => {
            // debug!("prev_range:{:?}, new_range:{:?}",prev_range,new_range);
            inner.tasks[current].memory_set.shrink_framed_area(prev_range, new_range);
            return 0
        },
        Some(RangesChanged::Splitted(
            front,
            back)
        ) => {
            inner.tasks[current].memory_set.split_framed_area(
                front, back);
            return 0
        },
        _ => {}
    }
    -1
}