// Codex assisted hard-link design, implementation, and host regression tests.
// Reference: LearningOS/rCore-Tutorial-Guide, source/chapter6/4exercise.rst.
use super::{
    block_cache_sync_all, get_block_cache, BlockDevice, DirEntry, DiskInode, DiskInodeType,
    EasyFileSystem, DIRENT_SZ,
};
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::{Mutex, MutexGuard};
/// Virtual filesystem layer over easy-fs
pub struct Inode {
    block_id: usize,
    block_offset: usize,
    fs: Arc<Mutex<EasyFileSystem>>,
    block_device: Arc<dyn BlockDevice>,
}

impl Inode {
    /// Create a vfs inode
    pub fn new(
        block_id: u32,
        block_offset: usize,
        fs: Arc<Mutex<EasyFileSystem>>,
        block_device: Arc<dyn BlockDevice>,
    ) -> Self {
        {
            let mut guard = fs.lock();
            let id = guard.inode_id(block_id as usize, block_offset);
            *guard.handles.entry(id).or_insert(0) += 1;
        }
        Self {
            block_id: block_id as usize,
            block_offset,
            fs,
            block_device,
        }
    }
    /// Call a function over a disk inode to read it
    fn read_disk_inode<V>(&self, f: impl FnOnce(&DiskInode) -> V) -> V {
        get_block_cache(self.block_id, Arc::clone(&self.block_device))
            .lock()
            .read(self.block_offset, f)
    }
    /// Call a function over a disk inode to modify it
    fn modify_disk_inode<V>(&self, f: impl FnOnce(&mut DiskInode) -> V) -> V {
        get_block_cache(self.block_id, Arc::clone(&self.block_device))
            .lock()
            .modify(self.block_offset, f)
    }
    /// Find inode under a disk inode by name
    fn find_inode_id(&self, name: &str, disk_inode: &DiskInode) -> Option<u32> {
        // assert it is a directory
        assert!(disk_inode.is_dir());
        let file_count = (disk_inode.size as usize) / DIRENT_SZ;
        let mut dirent = DirEntry::empty();
        for i in 0..file_count {
            assert_eq!(
                disk_inode.read_at(DIRENT_SZ * i, dirent.as_bytes_mut(), &self.block_device,),
                DIRENT_SZ,
            );
            if dirent.name() == name {
                return Some(dirent.inode_id() as u32);
            }
        }
        None
    }
    /// Find inode under current inode by name
    pub fn find(&self, name: &str) -> Option<Arc<Inode>> {
        if !Self::valid_name(name) {
            return None;
        }
        let mut fs = self.fs.lock();
        let id = self.read_disk_inode(|inode| self.find_inode_id(name, inode))?;
        Some(self.handle(id, &mut fs))
    }

    fn valid_name(name: &str) -> bool {
        !name.is_empty()
            && name.len() <= 27
            && !name.as_bytes().contains(&0)
            && !name.contains('/')
            && name != "."
            && name != ".."
    }

    // Called with the filesystem lock held, so lookup and reference acquisition
    // are atomic with respect to unlink and inode-number reuse.
    fn handle(&self, id: u32, fs: &mut EasyFileSystem) -> Arc<Inode> {
        let (block_id, block_offset) = fs.get_disk_inode_pos(id);
        *fs.handles.entry(id).or_insert(0) += 1;
        Arc::new(Self {
            block_id: block_id as usize,
            block_offset,
            fs: self.fs.clone(),
            block_device: self.block_device.clone(),
        })
    }

    /// Stable on-disk inode number.
    pub fn inode_id(&self) -> u32 {
        self.fs.lock().inode_id(self.block_id, self.block_offset)
    }

    /// Number of directory entries currently referring to this inode.
    pub fn link_count(&self) -> u32 {
        let fs = self.fs.lock();
        fs.link_count(fs.inode_id(self.block_id, self.block_offset))
    }

    /// Whether the inode represents a directory.
    pub fn is_dir(&self) -> bool {
        let _fs = self.fs.lock();
        self.read_disk_inode(|inode| inode.is_dir())
    }

    fn insert_entry(&self, name: &str, id: u32, fs: &mut MutexGuard<EasyFileSystem>) {
        self.modify_disk_inode(|root| {
            let count = root.size as usize / DIRENT_SZ;
            let mut slot = count;
            let mut entry = DirEntry::empty();
            for i in 0..count {
                root.read_at(i * DIRENT_SZ, entry.as_bytes_mut(), &self.block_device);
                if entry.name().is_empty() {
                    slot = i;
                    break;
                }
            }
            if slot == count {
                self.increase_size(((count + 1) * DIRENT_SZ) as u32, root, fs);
            }
            root.write_at(
                slot * DIRENT_SZ,
                DirEntry::new(name, id).as_bytes(),
                &self.block_device,
            );
        });
    }

    /// Add another name for a regular file in this flat directory.
    pub fn link(&self, old: &str, new: &str) -> bool {
        if !Self::valid_name(old) || !Self::valid_name(new) || old == new {
            return false;
        }
        let mut fs = self.fs.lock();
        let id = self.read_disk_inode(|root| {
            if self.find_inode_id(new, root).is_some() {
                None
            } else {
                self.find_inode_id(old, root)
            }
        });
        let id = match id {
            Some(id) => id,
            None => return false,
        };
        let (block, offset) = fs.get_disk_inode_pos(id);
        let is_file = get_block_cache(block as usize, self.block_device.clone())
            .lock()
            .read(offset, |inode: &DiskInode| inode.is_file());
        if !is_file {
            return false;
        }
        self.insert_entry(new, id, &mut fs);
        block_cache_sync_all();
        true
    }

    /// Remove one name, retaining an unlinked file until its last handle drops.
    pub fn unlink(&self, name: &str) -> bool {
        if !Self::valid_name(name) {
            return false;
        }
        let mut fs = self.fs.lock();
        let removed = self.modify_disk_inode(|root| {
            let mut entry = DirEntry::empty();
            for i in 0..root.size as usize / DIRENT_SZ {
                root.read_at(i * DIRENT_SZ, entry.as_bytes_mut(), &self.block_device);
                if entry.name() == name {
                    let id = entry.inode_id();
                    root.write_at(
                        i * DIRENT_SZ,
                        DirEntry::empty().as_bytes(),
                        &self.block_device,
                    );
                    return Some(id);
                }
            }
            None
        });
        match removed {
            Some(id) => {
                fs.reclaim_if_unused(id);
                block_cache_sync_all();
                true
            }
            None => false,
        }
    }
    /// Increase the size of a disk inode
    fn increase_size(
        &self,
        new_size: u32,
        disk_inode: &mut DiskInode,
        fs: &mut MutexGuard<EasyFileSystem>,
    ) {
        if new_size < disk_inode.size {
            return;
        }
        let blocks_needed = disk_inode.blocks_num_needed(new_size);
        let mut v: Vec<u32> = Vec::new();
        for _ in 0..blocks_needed {
            v.push(fs.alloc_data());
        }
        disk_inode.increase_size(new_size, v, &self.block_device);
    }
    /// Create inode under current inode by name
    pub fn create(&self, name: &str) -> Option<Arc<Inode>> {
        if !Self::valid_name(name) {
            return None;
        }
        let mut fs = self.fs.lock();
        let op = |root_inode: &DiskInode| {
            // assert it is a directory
            assert!(root_inode.is_dir());
            // has the file been created?
            self.find_inode_id(name, root_inode)
        };
        if self.read_disk_inode(op).is_some() {
            return None;
        }
        // create a new file
        // alloc a inode with an indirect block
        let new_inode_id = fs.alloc_inode();
        // initialize inode
        let (new_inode_block_id, new_inode_block_offset) = fs.get_disk_inode_pos(new_inode_id);
        get_block_cache(new_inode_block_id as usize, Arc::clone(&self.block_device))
            .lock()
            .modify(new_inode_block_offset, |new_inode: &mut DiskInode| {
                new_inode.initialize(DiskInodeType::File);
            });
        self.insert_entry(name, new_inode_id, &mut fs);

        block_cache_sync_all();
        // return inode
        Some(self.handle(new_inode_id, &mut fs))
        // release efs lock automatically by compiler
    }
    /// List inodes under current inode
    pub fn ls(&self) -> Vec<String> {
        let _fs = self.fs.lock();
        self.read_disk_inode(|disk_inode| {
            let file_count = (disk_inode.size as usize) / DIRENT_SZ;
            let mut v: Vec<String> = Vec::new();
            for i in 0..file_count {
                let mut dirent = DirEntry::empty();
                assert_eq!(
                    disk_inode.read_at(i * DIRENT_SZ, dirent.as_bytes_mut(), &self.block_device,),
                    DIRENT_SZ,
                );
                if !dirent.name().is_empty() {
                    v.push(String::from(dirent.name()));
                }
            }
            v
        })
    }
    /// Read data from current inode
    pub fn read_at(&self, offset: usize, buf: &mut [u8]) -> usize {
        let _fs = self.fs.lock();
        self.read_disk_inode(|disk_inode| disk_inode.read_at(offset, buf, &self.block_device))
    }
    /// Write data to current inode
    pub fn write_at(&self, offset: usize, buf: &[u8]) -> usize {
        let mut fs = self.fs.lock();
        let size = self.modify_disk_inode(|disk_inode| {
            self.increase_size((offset + buf.len()) as u32, disk_inode, &mut fs);
            disk_inode.write_at(offset, buf, &self.block_device)
        });
        block_cache_sync_all();
        size
    }
    /// Clear the data in current inode
    pub fn clear(&self) {
        let mut fs = self.fs.lock();
        self.modify_disk_inode(|disk_inode| {
            let size = disk_inode.size;
            let data_blocks_dealloc = disk_inode.clear_size(&self.block_device);
            assert!(data_blocks_dealloc.len() == DiskInode::total_blocks(size) as usize);
            for data_block in data_blocks_dealloc.into_iter() {
                fs.dealloc_data(data_block);
            }
        });
        block_cache_sync_all();
    }
}

impl Drop for Inode {
    fn drop(&mut self) {
        let mut fs = self.fs.lock();
        let id = fs.inode_id(self.block_id, self.block_offset);
        let count = fs.handles.get_mut(&id).expect("unregistered inode handle");
        *count -= 1;
        if *count == 0 {
            fs.handles.remove(&id);
        }
        fs.reclaim_if_unused(id);
        block_cache_sync_all();
    }
}
