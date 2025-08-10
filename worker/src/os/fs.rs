use std::{collections::HashMap, rc::Rc};

use thiserror::Error;
use tracing::warn;

pub type Inode = u64;

#[derive(Clone, Debug)]
pub enum FsEntry {
    Dir(HashMap<Vec<u8>, Inode>),
    File(Rc<Vec<u8>>),
}

impl FsEntry {
    pub fn as_file(&self) -> Option<&Rc<Vec<u8>>> {
        match self {
            FsEntry::File(data) => Some(data),
            _ => None,
        }
    }

    pub fn as_dir(&self) -> Option<&HashMap<Vec<u8>, Inode>> {
        match self {
            FsEntry::Dir(dir) => Some(dir),
            _ => None,
        }
    }
}

#[derive(Clone)]
pub struct Fs {
    pub entries: Vec<FsEntry>,
    pub parent_pointers: Vec<Inode>,
}

#[derive(Debug, Error)]
pub enum FsError {
    #[error("Not a directory")]
    NotDir,
    #[error("Is a directory")]
    IsDir,
    #[error("No such file or directory")]
    DoesNotExist,
}

impl Fs {
    pub fn new() -> Fs {
        Fs {
            entries: vec![FsEntry::Dir(HashMap::new())],
            parent_pointers: vec![0],
        }
    }

    pub fn root(&self) -> Inode {
        0
    }

    pub fn add_file_with_path(&mut self, path: &[u8], data: Rc<Vec<u8>>) {
        let components: Vec<_> = path.split(|x| *x == b'/').map(|x| x.to_vec()).collect();
        let mut cur = self.root();
        for c in &components[..components.len() - 1] {
            if c.is_empty() {
                continue;
            }
            let FsEntry::Dir(dir) = &mut self.entries[cur as usize] else {
                warn!("invalid file set");
                panic!("invalid files");
            };
            if let Some(e) = dir.get(c) {
                cur = *e;
            } else {
                cur = self.add_entry(cur, c, FsEntry::Dir(HashMap::new()));
            }
        }
        self.add_file(cur, components.last().unwrap(), data);
    }

    pub fn add_file(&mut self, parent: Inode, name: &[u8], data: Rc<Vec<u8>>) {
        self.add_entry(parent, name, FsEntry::File(data));
    }

    pub fn get_file_with_path(&self, path: &[u8]) -> Result<Rc<Vec<u8>>, FsError> {
        let root = self.root();
        let inode = self.get(root, path)?;
        let data = self.get_file(inode)?;
        Ok(data)
    }

    pub fn get(&self, parent: Inode, path: &[u8]) -> Result<Inode, FsError> {
        if path.is_empty() {
            return Ok(parent);
        }
        let FsEntry::Dir(dir) = &self.entries[parent as usize] else {
            return Err(FsError::NotDir);
        };
        let mut path = path.splitn(2, |x| *x == b'/');
        let cur = path.next().unwrap();
        let rest = path.next().unwrap_or(b"");
        if cur == b"." || cur == b"" {
            self.get(parent, rest)
        } else if cur == b".." {
            self.get(self.parent_pointers[parent as usize], rest)
        } else if let Some(child) = dir.get(cur) {
            self.get(*child, rest)
        } else {
            Err(FsError::DoesNotExist)
        }
    }

    pub fn get_file(&self, inode: Inode) -> Result<Rc<Vec<u8>>, FsError> {
        match &self.entries[inode as usize] {
            FsEntry::Dir(_) => Err(FsError::IsDir),
            FsEntry::File(f) => Ok(f.clone()),
        }
    }

    fn add_entry(&mut self, parent: Inode, name: &[u8], entry: FsEntry) -> Inode {
        let new_entry = self.entries.len() as Inode;
        self.entries.push(entry);
        self.parent_pointers.push(parent);
        let FsEntry::Dir(dir) = &mut self.entries[parent as usize] else {
            panic!("invalid call to add_entry");
        };
        dir.insert(name.to_vec(), new_entry);
        new_entry
    }
}

impl Default for Fs {
    fn default() -> Self {
        Self::new()
    }
}
