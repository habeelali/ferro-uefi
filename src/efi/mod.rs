//! UEFI Boot Services core: a physical page allocator, a real
//! EFI_MEMORY_DESCRIPTOR-producing memory map, a fixed-capacity
//! protocol database, and both wired into a spec-laid-out
//! EFI_BOOT_SERVICES/EFI_SYSTEM_TABLE pair. See boot_services.rs for
//! exactly which of the ~40 Boot Services functions are real versus
//! stubbed.

pub mod block_io;
pub mod boot_services;
pub mod console;
mod crc32;
pub mod device_path;
pub mod events;
pub mod file_protocol;
pub mod memory;
pub mod protocol_db;
pub mod protocols;
pub mod runtime_services;
pub mod system_table;
pub mod types;
pub mod variables;

/// Brings the whole layer up: page allocator, then the table headers
/// and their CRCs. Must run after mmu::init() (memory.rs's allocator
/// needs RAM_LIMIT); no dependency on UART/timer/framebuffer.
pub fn init() {
    memory::init();
    boot_services::init();
    runtime_services::init();
    system_table::init();
}
