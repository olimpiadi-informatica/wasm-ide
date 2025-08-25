use std::{collections::HashMap, rc::Rc};

use enum_as_inner::EnumAsInner;
use thiserror::Error;
use tracing::warn;

use super::Pipe;

pub type Inode = u64;

#[derive(Clone, EnumAsInner)]
pub enum FsEntry {
    Dir(HashMap<Vec<u8>, Inode>),
    File(Rc<Vec<u8>>),
    Pipe(Rc<Pipe>),
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
    #[error("File exists")]
    Exist,
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
        self.add_entry_with_path(path, FsEntry::File(data));
    }

    pub fn add_entry_with_path(&mut self, path: &[u8], entry: FsEntry) {
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
        self.add_entry(cur, components.last().unwrap(), entry);
    }

    pub fn get_file_with_path(&self, path: &[u8]) -> Result<Rc<Vec<u8>>, FsError> {
        let root = self.root();
        let inode = self.get(root, path)?;
        let data = self.get_file(inode)?;
        Ok(data)
    }

    pub fn open(
        &mut self,
        mut parent: Inode,
        path: &[u8],
        creat: bool,
        excl: bool,
    ) -> Result<Inode, FsError> {
        let mut dirs: Vec<&[u8]> = path
            .split(|x| *x == b'/')
            .filter(|x| !x.is_empty())
            .collect();
        let name = dirs.pop();
        for cur in dirs {
            let FsEntry::Dir(dir) = &self.entries[parent as usize] else {
                return Err(FsError::NotDir);
            };
            if cur == b"." {
            } else if cur == b".." {
                parent = self.parent_pointers[parent as usize];
            } else if let Some(child) = dir.get(cur) {
                parent = *child;
            } else {
                return Err(FsError::DoesNotExist);
            };
        }
        let Some(name) = name else {
            return Ok(parent);
        };
        let FsEntry::Dir(dir) = &mut self.entries[parent as usize] else {
            return Err(FsError::NotDir);
        };
        if name == b"." {
            Ok(parent)
        } else if name == b".." {
            Ok(self.parent_pointers[parent as usize])
        } else if let Some(file) = dir.get(name) {
            if excl {
                return Err(FsError::Exist);
            }
            Ok(*file)
        } else if creat {
            Ok(self.add_entry(parent, name, FsEntry::File(Rc::new(Vec::new()))))
        } else {
            Err(FsError::DoesNotExist)
        }
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
            FsEntry::File(f) => Ok(f.clone()),
            FsEntry::Dir(_) => Err(FsError::IsDir),
            FsEntry::Pipe(_) => todo!(),
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
