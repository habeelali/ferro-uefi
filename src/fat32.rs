//! Minimal read-only FAT32: mount an MBR-partitioned FAT32 volume,
//! list the root directory, and read a named file's contents by
//! walking its cluster chain in the FAT. Long-filename entries are
//! skipped (not parsed) -- files are matched and listed by their 8.3
//! short name only.

use crate::sd::{Card, SdError};

const SECTOR_SIZE: usize = 512;
const END_OF_CHAIN: u32 = 0x0FFF_FFF8;

#[derive(Debug)]
pub enum Fat32Error {
    Sd(#[allow(dead_code)] SdError), // read via Debug logging
    NoMbrSignature,
    NoFat32Partition,
    BadBootSector,
    NotFound,
}

impl From<SdError> for Fat32Error {
    fn from(e: SdError) -> Self {
        Fat32Error::Sd(e)
    }
}

#[derive(Clone, Copy)]
pub struct Fat32 {
    partition_start_lba: u32,
    sectors_per_cluster: u8,
    reserved_sectors: u16,
    num_fats: u8,
    fat_size_sectors: u32,
    root_cluster: u32,
}

impl Fat32 {
    /// Reads the MBR, finds the first FAT32 partition (type 0x0B or
    /// 0x0C), and parses its BPB.
    pub fn mount(card: &Card) -> Result<Fat32, Fat32Error> {
        let mut sector = [0u8; SECTOR_SIZE];
        card.read_block(0, &mut sector)?;
        if sector[510] != 0x55 || sector[511] != 0xAA {
            return Err(Fat32Error::NoMbrSignature);
        }

        let mut partition_start_lba = None;
        for i in 0..4 {
            let entry = &sector[446 + i * 16..446 + i * 16 + 16];
            if entry[4] == 0x0B || entry[4] == 0x0C {
                partition_start_lba = Some(u32::from_le_bytes([
                    entry[8], entry[9], entry[10], entry[11],
                ]));
                break;
            }
        }
        let partition_start_lba = partition_start_lba.ok_or(Fat32Error::NoFat32Partition)?;

        let mut boot = [0u8; SECTOR_SIZE];
        card.read_block(partition_start_lba, &mut boot)?;
        if boot[510] != 0x55 || boot[511] != 0xAA {
            return Err(Fat32Error::BadBootSector);
        }

        let bytes_per_sector = u16::from_le_bytes([boot[11], boot[12]]);
        let fat_size_sectors = u32::from_le_bytes([boot[36], boot[37], boot[38], boot[39]]);
        if bytes_per_sector as usize != SECTOR_SIZE || fat_size_sectors == 0 {
            return Err(Fat32Error::BadBootSector);
        }

        Ok(Fat32 {
            partition_start_lba,
            sectors_per_cluster: boot[13],
            reserved_sectors: u16::from_le_bytes([boot[14], boot[15]]),
            num_fats: boot[16],
            fat_size_sectors,
            root_cluster: u32::from_le_bytes([boot[44], boot[45], boot[46], boot[47]]),
        })
    }

    fn first_data_sector(&self) -> u32 {
        self.partition_start_lba
            + self.reserved_sectors as u32
            + self.num_fats as u32 * self.fat_size_sectors
    }

    /// A safe range of otherwise-unused sectors within the reserved
    /// area (before the FATs start) -- sectors 0 (boot sector), 1
    /// (FSInfo), and 6-7 (their conventional backups) are spoken for,
    /// but real FAT32 volumes always carry many more reserved sectors
    /// than those four use (mkfs.fat's default is 32). Returns
    /// (start_lba, sector_count) for whatever's safely free starting
    /// at sector 16, or None if this volume's reserved area is too
    /// small to have any margin there.
    pub fn private_scratch_region(&self) -> Option<(u32, u32)> {
        const START_OFFSET: u32 = 16;
        let reserved = self.reserved_sectors as u32;
        if reserved <= START_OFFSET {
            return None;
        }
        Some((self.partition_start_lba + START_OFFSET, reserved - START_OFFSET))
    }

    fn cluster_to_lba(&self, cluster: u32) -> u32 {
        self.first_data_sector() + (cluster - 2) * self.sectors_per_cluster as u32
    }

    fn fat_entry(&self, card: &Card, cluster: u32) -> Result<u32, Fat32Error> {
        let fat_offset = cluster * 4;
        let fat_sector =
            self.partition_start_lba + self.reserved_sectors as u32 + fat_offset / SECTOR_SIZE as u32;
        let offset_in_sector = (fat_offset % SECTOR_SIZE as u32) as usize;
        let mut sector = [0u8; SECTOR_SIZE];
        card.read_block(fat_sector, &mut sector)?;
        let raw = u32::from_le_bytes(
            sector[offset_in_sector..offset_in_sector + 4]
                .try_into()
                .unwrap(),
        );
        Ok(raw & 0x0FFF_FFFF)
    }

    /// Lists root-directory entries (8.3 names, directories included)
    /// into `names`/`sizes`, returning how many were found (capped at
    /// the slices' length).
    pub fn list_root(
        &self,
        card: &Card,
        names: &mut [[u8; 11]],
        sizes: &mut [u32],
    ) -> Result<usize, Fat32Error> {
        let mut cluster = self.root_cluster;
        let mut count = 0usize;
        loop {
            let lba = self.cluster_to_lba(cluster);
            for s in 0..self.sectors_per_cluster as u32 {
                let mut sector = [0u8; SECTOR_SIZE];
                card.read_block(lba + s, &mut sector)?;
                for entry in sector.chunks_exact(32) {
                    if entry[0] == 0x00 {
                        return Ok(count);
                    }
                    if entry[0] == 0xE5 || entry[11] == 0x0F || entry[11] & 0x08 != 0 {
                        continue; // deleted, long-name, or volume-label entry
                    }
                    if count < names.len() {
                        names[count].copy_from_slice(&entry[0..11]);
                        sizes[count] = u32::from_le_bytes([entry[28], entry[29], entry[30], entry[31]]);
                        count += 1;
                    }
                }
            }
            let next = self.fat_entry(card, cluster)?;
            if next >= END_OF_CHAIN {
                break;
            }
            cluster = next;
        }
        Ok(count)
    }

    /// Reads a root-directory file's contents into `out`, matched by
    /// raw 8.3 name (e.g. `b"README  TXT"`, space-padded). Returns
    /// the byte count actually written (truncated to `out.len()` if
    /// the file is larger).
    pub fn read_file(&self, card: &Card, name_8_3: &[u8; 11], out: &mut [u8]) -> Result<usize, Fat32Error> {
        let mut cluster = self.root_cluster;
        let mut first_cluster = 0u32;
        let mut file_size = 0u32;

        'search: loop {
            let lba = self.cluster_to_lba(cluster);
            for s in 0..self.sectors_per_cluster as u32 {
                let mut sector = [0u8; SECTOR_SIZE];
                card.read_block(lba + s, &mut sector)?;
                for entry in sector.chunks_exact(32) {
                    if entry[0] == 0x00 {
                        break 'search;
                    }
                    if entry[0] == 0xE5 || entry[11] == 0x0F || entry[11] & 0x08 != 0 {
                        continue;
                    }
                    if &entry[0..11] == name_8_3 {
                        let hi = u16::from_le_bytes([entry[20], entry[21]]) as u32;
                        let lo = u16::from_le_bytes([entry[26], entry[27]]) as u32;
                        first_cluster = (hi << 16) | lo;
                        file_size = u32::from_le_bytes([entry[28], entry[29], entry[30], entry[31]]);
                        break 'search;
                    }
                }
            }
            let next = self.fat_entry(card, cluster)?;
            if next >= END_OF_CHAIN {
                break;
            }
            cluster = next;
        }

        if first_cluster == 0 {
            return Err(Fat32Error::NotFound);
        }

        let to_read = (file_size as usize).min(out.len());
        let mut written = 0usize;
        let mut cluster = first_cluster;
        while written < to_read {
            let lba = self.cluster_to_lba(cluster);
            for s in 0..self.sectors_per_cluster as u32 {
                if written >= to_read {
                    break;
                }
                let mut sector = [0u8; SECTOR_SIZE];
                card.read_block(lba + s, &mut sector)?;
                let take = (to_read - written).min(SECTOR_SIZE);
                out[written..written + take].copy_from_slice(&sector[..take]);
                written += take;
            }
            if written >= to_read {
                break;
            }
            let next = self.fat_entry(card, cluster)?;
            if next >= END_OF_CHAIN {
                break;
            }
            cluster = next;
        }
        Ok(written)
    }
}
