//! Minimal FAT32: mount an MBR-partitioned FAT32 volume, list a
//! directory (root or, via `resolve`, any subdirectory), and
//! read/write a named file's contents by walking (and, for writes,
//! allocating/truncating) its cluster chain in the FAT.
//! Long-filename entries are skipped (not parsed) -- files are
//! matched and listed by their 8.3 short name only. Write support
//! covers exactly what a root-directory data file needs: create,
//! overwrite, grow, and shrink -- reading walks into subdirectories
//! fine, but writing/creating stays root-only (no subdirectory
//! creation, no deletion).

use crate::sd::{Card, SdError};

const SECTOR_SIZE: usize = 512;
const END_OF_CHAIN: u32 = 0x0FFF_FFF8;
const ATTR_ARCHIVE: u8 = 0x20;
const ATTR_DIRECTORY: u8 = 0x10;

#[derive(Debug)]
pub enum Fat32Error {
    Sd(#[allow(dead_code)] SdError), // read via Debug logging
    NoMbrSignature,
    NoFat32Partition,
    BadBootSector,
    NotFound,
    NoSpace,
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

    fn cluster_size_bytes(&self) -> usize {
        self.sectors_per_cluster as usize * SECTOR_SIZE
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

    /// Writes one FAT entry, mirrored to every FAT copy (real volumes
    /// almost always carry 2), preserving the reserved top 4 bits per
    /// entry rather than assuming they're 0.
    fn set_fat_entry(&self, card: &Card, cluster: u32, value: u32) -> Result<(), Fat32Error> {
        let fat_offset = cluster * 4;
        let sector_offset = fat_offset / SECTOR_SIZE as u32;
        let offset_in_sector = (fat_offset % SECTOR_SIZE as u32) as usize;
        for fat_idx in 0..self.num_fats as u32 {
            let fat_sector = self.partition_start_lba
                + self.reserved_sectors as u32
                + fat_idx * self.fat_size_sectors
                + sector_offset;
            let mut sector = [0u8; SECTOR_SIZE];
            card.read_block(fat_sector, &mut sector)?;
            let existing = u32::from_le_bytes(
                sector[offset_in_sector..offset_in_sector + 4].try_into().unwrap(),
            );
            let new_val = (value & 0x0FFF_FFFF) | (existing & 0xF000_0000);
            sector[offset_in_sector..offset_in_sector + 4].copy_from_slice(&new_val.to_le_bytes());
            card.write_block(fat_sector, &sector)?;
        }
        Ok(())
    }

    fn zero_cluster(&self, card: &Card, cluster: u32) -> Result<(), Fat32Error> {
        let lba = self.cluster_to_lba(cluster);
        let zero = [0u8; SECTOR_SIZE];
        for s in 0..self.sectors_per_cluster as u32 {
            card.write_block(lba + s, &zero)?;
        }
        Ok(())
    }

    /// Scans the FAT for a free (all-zero) cluster at or after
    /// `start`, skipping cluster numbers 0/1 which are never valid
    /// data clusters.
    fn find_free_cluster(&self, card: &Card, start: u32) -> Result<u32, Fat32Error> {
        let max_cluster = (self.fat_size_sectors as usize * SECTOR_SIZE / 4) as u32;
        let mut cluster = start.max(2);
        while cluster < max_cluster {
            if self.fat_entry(card, cluster)? == 0 {
                return Ok(cluster);
            }
            cluster += 1;
        }
        Err(Fat32Error::NoSpace)
    }

    /// Allocates a fresh, zeroed `count`-cluster chain and returns its
    /// first cluster number.
    fn allocate_chain(&self, card: &Card, count: u32) -> Result<u32, Fat32Error> {
        let mut first = 0u32;
        let mut prev = 0u32;
        let mut search_from = 2u32;
        for _ in 0..count {
            let c = self.find_free_cluster(card, search_from)?;
            search_from = c + 1;
            self.set_fat_entry(card, c, END_OF_CHAIN)?;
            self.zero_cluster(card, c)?;
            if first == 0 {
                first = c;
            } else {
                self.set_fat_entry(card, prev, c)?;
            }
            prev = c;
        }
        Ok(first)
    }

    /// Frees every cluster in the chain starting at `cluster`
    /// (inclusive), following FAT links until end-of-chain.
    fn free_chain_from(&self, card: &Card, mut cluster: u32) -> Result<(), Fat32Error> {
        while cluster >= 2 && cluster < END_OF_CHAIN {
            let next = self.fat_entry(card, cluster)?;
            self.set_fat_entry(card, cluster, 0)?;
            cluster = next;
        }
        Ok(())
    }

    /// Walks an existing chain so it ends up exactly `needed` clusters
    /// long: truncates (freeing the tail) if it's currently longer,
    /// extends (allocating more) if shorter. Returns the unchanged
    /// first cluster.
    fn resize_chain(&self, card: &Card, first: u32, needed: u32) -> Result<u32, Fat32Error> {
        let mut cluster = first;
        let mut count = 1u32;
        loop {
            if count == needed {
                let next = self.fat_entry(card, cluster)?;
                if next >= 2 && next < END_OF_CHAIN {
                    self.free_chain_from(card, next)?;
                }
                self.set_fat_entry(card, cluster, END_OF_CHAIN)?;
                return Ok(first);
            }
            let next = self.fat_entry(card, cluster)?;
            if next < 2 || next >= END_OF_CHAIN {
                let extra = self.allocate_chain(card, needed - count)?;
                self.set_fat_entry(card, cluster, extra)?;
                return Ok(first);
            }
            cluster = next;
            count += 1;
        }
    }

    /// Creates or overwrites a root-directory file by 8.3 name.
    /// Allocates (or resizes) its cluster chain to fit `data` exactly,
    /// writes the data, and creates/updates its directory entry --
    /// extending the root directory with a new cluster if every
    /// existing entry slot is taken.
    pub fn write_file(&self, card: &Card, name_8_3: &[u8; 11], data: &[u8]) -> Result<(), Fat32Error> {
        let clusters_needed = data.len().div_ceil(self.cluster_size_bytes()).max(1) as u32;

        let mut found: Option<(u32, usize, u32)> = None; // (sector lba, offset, old first cluster)
        let mut free_slot: Option<(u32, usize)> = None;
        let mut dir_cluster = self.root_cluster;
        let mut last_dir_cluster;
        let mut exhausted_without_terminator = true;

        'search: loop {
            last_dir_cluster = dir_cluster;
            let lba = self.cluster_to_lba(dir_cluster);
            for s in 0..self.sectors_per_cluster as u32 {
                let sec_lba = lba + s;
                let mut sector = [0u8; SECTOR_SIZE];
                card.read_block(sec_lba, &mut sector)?;
                for (i, entry) in sector.chunks_exact(32).enumerate() {
                    let off = i * 32;
                    if entry[0] == 0x00 {
                        if free_slot.is_none() {
                            free_slot = Some((sec_lba, off));
                        }
                        exhausted_without_terminator = false;
                        break 'search;
                    }
                    if entry[0] == 0xE5 {
                        if free_slot.is_none() {
                            free_slot = Some((sec_lba, off));
                        }
                        continue;
                    }
                    if entry[11] == 0x0F || entry[11] & 0x08 != 0 {
                        continue; // long-name or volume-label entry
                    }
                    if &entry[0..11] == name_8_3 {
                        let hi = u16::from_le_bytes([entry[20], entry[21]]) as u32;
                        let lo = u16::from_le_bytes([entry[26], entry[27]]) as u32;
                        found = Some((sec_lba, off, (hi << 16) | lo));
                    }
                }
            }
            let next = self.fat_entry(card, dir_cluster)?;
            if next >= END_OF_CHAIN {
                break;
            }
            dir_cluster = next;
        }

        if found.is_none() && free_slot.is_none() && exhausted_without_terminator {
            // Root directory is completely full (no 0x00 terminator
            // and no deleted-entry slot anywhere) -- give it one more
            // cluster and use its first entry.
            let new_cluster = self.allocate_chain(card, 1)?;
            self.set_fat_entry(card, last_dir_cluster, new_cluster)?;
            free_slot = Some((self.cluster_to_lba(new_cluster), 0));
        }

        let first_cluster = match found {
            Some((_, _, old_first)) if old_first >= 2 => self.resize_chain(card, old_first, clusters_needed)?,
            _ => self.allocate_chain(card, clusters_needed)?,
        };

        let mut written = 0usize;
        let mut cluster = first_cluster;
        while written < data.len() {
            let lba = self.cluster_to_lba(cluster);
            for s in 0..self.sectors_per_cluster as u32 {
                if written >= data.len() {
                    break;
                }
                let mut sector = [0u8; SECTOR_SIZE];
                let take = (data.len() - written).min(SECTOR_SIZE);
                sector[..take].copy_from_slice(&data[written..written + take]);
                card.write_block(lba + s, &sector)?;
                written += take;
            }
            if written >= data.len() {
                break;
            }
            cluster = self.fat_entry(card, cluster)?;
        }

        let (entry_lba, entry_off) = found.map(|(lba, off, _)| (lba, off)).or(free_slot).unwrap();
        let mut sector = [0u8; SECTOR_SIZE];
        card.read_block(entry_lba, &mut sector)?;
        let entry = &mut sector[entry_off..entry_off + 32];
        entry[0..11].copy_from_slice(name_8_3);
        entry[11] = ATTR_ARCHIVE;
        entry[12..20].fill(0); // reserved + creation/access timestamps -- untracked
        entry[20..22].copy_from_slice(&(((first_cluster >> 16) & 0xFFFF) as u16).to_le_bytes());
        entry[22..26].fill(0); // write timestamp -- untracked
        entry[26..28].copy_from_slice(&((first_cluster & 0xFFFF) as u16).to_le_bytes());
        entry[28..32].copy_from_slice(&(data.len() as u32).to_le_bytes());
        card.write_block(entry_lba, &sector)?;

        Ok(())
    }

    /// Lists root-directory entries (8.3 names, directories included)
    /// into `names`/`sizes`, returning how many were found (capped at
    /// the slices' length).
    pub fn list_root(&self, card: &Card, names: &mut [[u8; 11]], sizes: &mut [u32]) -> Result<usize, Fat32Error> {
        self.list_dir(card, self.root_cluster, names, sizes)
    }

    /// Lists any directory's entries (8.3 names, subdirectories
    /// included) by its first cluster -- `list_root` is just this
    /// called with `self.root_cluster`. Get a subdirectory's cluster
    /// via `resolve`.
    pub fn list_dir(
        &self,
        card: &Card,
        dir_cluster: u32,
        names: &mut [[u8; 11]],
        sizes: &mut [u32],
    ) -> Result<usize, Fat32Error> {
        let mut cluster = dir_cluster;
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

    /// Searches one directory's cluster chain for `name_8_3`,
    /// returning (first_cluster, size, is_directory) if found. The
    /// shared primitive both `resolve` (path walking) and the
    /// existing root-only `write_file`'s search logic build on.
    fn find_in_dir(&self, card: &Card, dir_cluster: u32, name_8_3: &[u8; 11]) -> Result<(u32, u32, bool), Fat32Error> {
        let mut cluster = dir_cluster;
        loop {
            let lba = self.cluster_to_lba(cluster);
            for s in 0..self.sectors_per_cluster as u32 {
                let mut sector = [0u8; SECTOR_SIZE];
                card.read_block(lba + s, &mut sector)?;
                for entry in sector.chunks_exact(32) {
                    if entry[0] == 0x00 {
                        return Err(Fat32Error::NotFound);
                    }
                    if entry[0] == 0xE5 || entry[11] == 0x0F || entry[11] & 0x08 != 0 {
                        continue;
                    }
                    if &entry[0..11] == name_8_3 {
                        let hi = u16::from_le_bytes([entry[20], entry[21]]) as u32;
                        let lo = u16::from_le_bytes([entry[26], entry[27]]) as u32;
                        let first_cluster = (hi << 16) | lo;
                        let size = u32::from_le_bytes([entry[28], entry[29], entry[30], entry[31]]);
                        let is_dir = entry[11] & ATTR_DIRECTORY != 0;
                        return Ok((first_cluster, size, is_dir));
                    }
                }
            }
            let next = self.fat_entry(card, cluster)?;
            if next >= END_OF_CHAIN {
                return Err(Fat32Error::NotFound);
            }
            cluster = next;
        }
    }

    /// The root cluster, for callers (efi::file_protocol) that need
    /// to tell "the root directory" apart from an arbitrary
    /// subdirectory cluster without their own separate bookkeeping.
    pub fn root_cluster(&self) -> u32 {
        self.root_cluster
    }

    /// Resolves a `\`-separated path (e.g. `EFI\BOOT\BOOTAA64.EFI`,
    /// no leading slash, each component an 8.3 name) starting from
    /// `start_cluster` (the root cluster, or any subdirectory's, for
    /// EFI_FILE_PROTOCOL.Open's "relative to `this`" semantics),
    /// walking into subdirectories as needed. Returns the final
    /// component's (first_cluster, size, is_directory); an empty path
    /// resolves to `start_cluster` itself.
    pub fn resolve_from(&self, card: &Card, start_cluster: u32, path: &[u8]) -> Result<(u32, u32, bool), Fat32Error> {
        let mut dir_cluster = start_cluster;
        let mut result = (start_cluster, 0u32, true);
        for component in path.split(|&b| b == b'\\').filter(|c| !c.is_empty()) {
            let name = to_8_3(component).ok_or(Fat32Error::NotFound)?;
            let (first_cluster, size, is_dir) = self.find_in_dir(card, dir_cluster, &name)?;
            result = (first_cluster, size, is_dir);
            if is_dir {
                dir_cluster = if first_cluster == 0 { self.root_cluster } else { first_cluster };
            }
        }
        Ok(result)
    }

    /// Reads up to `out.len()` bytes of a file whose first cluster
    /// and size are already known (from `resolve` or `find_in_dir`) --
    /// decouples "find the file" from "read its data".
    pub fn read_from(&self, card: &Card, first_cluster: u32, file_size: u32, out: &mut [u8]) -> Result<usize, Fat32Error> {
        if first_cluster == 0 {
            return Ok(0);
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

    /// Reads a root-directory file's contents into `out`, matched by
    /// raw 8.3 name (e.g. `b"README  TXT"`, space-padded). Returns
    /// the byte count actually written (truncated to `out.len()` if
    /// the file is larger). For files in a subdirectory, use
    /// `resolve` + `read_from` instead.
    pub fn read_file(&self, card: &Card, name_8_3: &[u8; 11], out: &mut [u8]) -> Result<usize, Fat32Error> {
        let (first_cluster, size, _) = self.find_in_dir(card, self.root_cluster, name_8_3)?;
        self.read_from(card, first_cluster, size, out)
    }
}

/// Converts a plain-ASCII path component (e.g. `b"BOOTAA64.EFI"`) into
/// an 8.3 short name -- uppercased, space-padded, `None` if it
/// doesn't fit in 8+3 characters.
fn to_8_3(component: &[u8]) -> Option<[u8; 11]> {
    let mut name = [b' '; 11];
    let (base, ext) = match component.iter().position(|&b| b == b'.') {
        Some(dot) => (&component[..dot], &component[dot + 1..]),
        None => (component, &[][..]),
    };
    if base.is_empty() || base.len() > 8 || ext.len() > 3 {
        return None;
    }
    for (i, &b) in base.iter().enumerate() {
        name[i] = b.to_ascii_uppercase();
    }
    for (i, &b) in ext.iter().enumerate() {
        name[8 + i] = b.to_ascii_uppercase();
    }
    Some(name)
}
