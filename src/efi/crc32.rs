//! Standard CRC-32 (IEEE 802.3, polynomial 0xEDB88320) -- what UEFI
//! table headers and EFI_BOOT_SERVICES.CalculateCrc32 both use.

pub fn crc32_ieee(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}
