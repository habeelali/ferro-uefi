//! UEFI Boot Services core: a physical page allocator, a real
//! EFI_MEMORY_DESCRIPTOR-producing memory map, a fixed-capacity
//! protocol database, and both wired into a spec-laid-out
//! EFI_BOOT_SERVICES/EFI_SYSTEM_TABLE pair. See boot_services.rs for
//! exactly which of the ~40 Boot Services functions are real versus
//! stubbed.

pub mod boot_services;
mod crc32;
pub mod memory;
pub mod protocol_db;
pub mod protocols;
pub mod system_table;
pub mod types;

/// Brings the whole layer up: page allocator, then the table headers
/// and their CRCs. Must run after mmu::init() (memory.rs's allocator
/// needs RAM_LIMIT); no dependency on UART/timer/framebuffer.
pub fn init() {
    memory::init();
    boot_services::init();
    system_table::init();
}
