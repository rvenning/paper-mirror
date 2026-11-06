//! Process memory counters for the performance probes.

use windows_sys::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX};
use windows_sys::Win32::System::Threading::GetCurrentProcess;

#[derive(Clone, Copy, Debug, Default)]
pub struct MemoryUsage {
    /// Resident set ("Memory" in Task Manager's Details tab is private working set;
    /// this is the full working set).
    pub working_set: usize,
    pub peak_working_set: usize,
    /// Committed private bytes.
    pub private_bytes: usize,
}

pub fn process_memory() -> MemoryUsage {
    let mut c: PROCESS_MEMORY_COUNTERS_EX = unsafe { std::mem::zeroed() };
    c.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
    let ok =
        unsafe { GetProcessMemoryInfo(GetCurrentProcess(), (&mut c as *mut PROCESS_MEMORY_COUNTERS_EX).cast(), c.cb) };
    if ok == 0 {
        return MemoryUsage::default();
    }
    MemoryUsage { working_set: c.WorkingSetSize, peak_working_set: c.PeakWorkingSetSize, private_bytes: c.PrivateUsage }
}

pub fn mib(bytes: usize) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}
