extern crate libc;

use std::num::NonZeroUsize;

use self::libc::{
    kern_return_t, mach_port_t, natural_t, task_threads, thread_act_array_t, vm_size_t,
};

#[allow(non_camel_case_types)]
// https://developer.apple.com/documentation/kernel/mach_port_name_t
type mach_port_name_t = natural_t;

extern "C" {
    // https://developer.apple.com/documentation/kernel/1402285-mach_vm_deallocate
    fn mach_vm_deallocate(
        target_task: mach_port_t,
        address: *mut u32,
        size: vm_size_t,
    ) -> kern_return_t;

    // https://developer.apple.com/documentation/kernel/1578777-mach_port_deallocate
    fn mach_port_deallocate(task: mach_port_t, name: mach_port_name_t) -> kern_return_t;
}

// 修复：libc >= 0.2.155 起在 iOS 目标上移除了 `mach_task_self` 的导出，
// 但系统库中该符号依然存在，因此在此处手动声明外部函数以恢复调用。
// macOS 目标上 libc 仍提供该符号，但同名 extern 声明可安全共存（弱符号）。
extern "C" {
    fn mach_task_self() -> mach_port_t;
}

pub(crate) fn num_threads() -> Option<NonZeroUsize> {
    // https://developer.apple.com/documentation/kernel/1537751-task_threads
    let mut thread_list: thread_act_array_t = std::ptr::null_mut();
    let mut thread_count = 0;

    // Safety:
    //  - `mach_task_self` always returns a valid value,
    //  - `thread_list` is a pointer that will point to kernel allocated memory that needs to be
    //    deallocated if the call succeeds
    let task = unsafe { mach_task_self() };
    let result = unsafe { task_threads(task, &mut thread_list, &mut thread_count) };

    if result == libc::KERN_SUCCESS {
        // Deallocate the mach port rights for the threads
        for thread in 0..thread_count {
            unsafe {
                mach_port_deallocate(task, *(thread_list.offset(thread as isize)) as u32);
            }
        }
        // Deallocate the thread list
        unsafe {
            mach_vm_deallocate(
                task,
                thread_list,
                std::mem::size_of::<mach_port_t>() * thread_count as usize,
            );
        }

        NonZeroUsize::new(thread_count as usize)
    } else {
        None
    }
}
